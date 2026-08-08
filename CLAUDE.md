# CLAUDE.md

## Git Commits

**Do not include Claude/AI information in commits.** Specifically:
- No "Generated with Claude Code" footer
- No "Co-Authored-By: Claude" lines
- No AI-related mentions in commit messages

Use conventional commits format: `feat:`, `fix:`, `docs:`, `refactor:`, `chore:`, etc.

## Project Overview

Fork de `bordeux/webxash3d-proxy` (base `9cb046f`, release 1.0.0) usado como
**espectador web de CS 1.6** do projeto `csserver_wstats`: o browser roda o
engine Xash3D em WASM e se conecta a um **relay HLTV** via WebRTC, com o proxy
Rust fazendo a ponte WebRTC → UDP.

O fork adiciona (detalhes em `PATCHES.md`):

1. Reescrita do ack de conexão do HLTV (`src/bridge.rs`) — o relay responde
   `"B"+cookie` (21 bytes), o cliente Xash3D só aceita `"B"`.
2. Handshake DTLS com browsers modernos — crate `webrtc-dtls` **vendored** em
   `vendor-webrtc-dtls/` (curvas pós-quânticas do Chrome), via
   `[patch.crates-io]` no `Cargo.toml`.
3. Cliente espectador (`client/src`): auto-start sem formulário, auto-reconnect
   com backoff, STUN, `valve.zip` servido pelo proxy.
4. Configuração por ambiente: `PACKAGE_ZIP`, `UDP_PORT_RANGE` (faixa de portas
   ICE), `CONSOLE_COMMANDS`, nome do espectador.
5. Build multi-stage (Node/Vite + Rust) no `Dockerfile`.
6. Robustez do cliente: aba oculta não congela (shim rAF + `maxPackets: 8192`),
   loading por estágios PT-BR, erros visíveis, sem `getUserMedia`.
7. **Auto-recuperação**: watchdog de stall no cliente (rejoin + reload
   silencioso), teardown de idle na bridge, reconexão em close/error do canal.
8. Causa raiz conhecida: HLTV relay pode ficar "mudo" (veja PATCHES.md §8).

## Deploy real (csserver_wstats)

- `watch-main` (este proxy) roda com `network_mode: host`, porta `27018`
  (página + `/websocket`), UDP `27019-27050` (ICE) e `GAME_SERVER` apontando
  para o relay HLTV `127.0.0.1:27020`.
- O relay HLTV (`watch-hltv`, imagem `cs16_stats:local`) conecta no servidor
  primário (host `27015`). Config de deploy em `config/watch/` no repo principal
  (fora deste fork).
- Exposição TLS via swag: `https://zueiracstrike.duckdns.org:4445`.
- A porta **default do upstream é `27016`**; o deploy usa `27018` via
  `LISTEN_PORT`/`WATCH_LISTEN_PORT`.

## Project Structure

```
webxash3d-proxy/
├── src/                        # Rust proxy server
│   ├── main.rs                 # HTTP server, /config endpoint, static files
│   ├── config.rs               # CLI args (clap), env vars
│   ├── signaling.rs            # WebRTC peer connection, data channels, ICE
│   ├── bridge.rs               # UDP <-> WebRTC packet forwarding
│   └── assets.rs               # Embedded static files
├── client/                     # Web client (TypeScript)
│   ├── src/
│   │   ├── index.html          # UI, canvas, login form, rAF shim inline
│   │   ├── main.ts             # Game init, config loading, watchdog, rejoin
│   │   ├── webrtc.ts           # Xash3DWebRTC class, WebSocket signaling
│   │   ├── favicon.png
│   │   └── logo.png
│   ├── package.json            # Dependencies: xash3d-fwgs, cs16-client
│   ├── vite.config.ts          # Build config, static file copying
│   └── tsconfig.json
├── vendor-webrtc-dtls/         # webrtc-dtls vendored (patch de curvas DTLS)
├── dist/                       # Built client output
├── Cargo.toml
└── Dockerfile
```

## Architecture

```
┌─────────────────────────┐     ┌─────────────────────────┐     ┌─────────────────┐
│  Browser                │     │  Proxy (Rust)           │     │  HLTV Relay     │
│  ├─ index.html          │     │                         │     │  (watch-hltv)   │
│  ├─ main.ts             │     │  ┌─────────────────┐    │     │                 │
│  └─ webrtc.ts           │     │  │ Signaling       │    │     │                 │
│      │                  │     │  │ (WebSocket)     │    │     │                 │
│      │ WebSocket ───────┼────►│  └─────────────────┘    │     │                 │
│      │                  │     │                         │     │                 │
│      │ WebRTC           │     │  ┌─────────────────┐    │     │                 │
│      ├─ write channel ◄─┼─────┼──┤ Bridge          ├────┼────►│  UDP :27020     │
│      └─ read channel ───┼─────┼──┤ (per client)    │    │     │                 │
│                         │     │  └─────────────────┘    │     │                 │
│  Xash3D WASM Engine     │     │                         │     │                 │
└─────────────────────────┘     └─────────────────────────┘     └────────┬────────┘
                                                                          │ UDP
                                                                 ┌────────▼────────┐
                                                                 │ Game server 27015│
                                                                 └─────────────────┘
```

## Rust Proxy (src/)

### main.rs
- Axum HTTP server (porta `LISTEN_PORT`, default 27016; deploy 27018)
- Routes: `/ws`, `/websocket`, `/config`, `/health`
- Static file serving via `--static-dir`/embedded assets com `ServeDir`
- `/config` endpoint returns client configuration with library paths and files_map
- `index.html` é servido com cache desabilitado (cliente espectador deve pegar
  sempre o bundle novo; o bundle JS é versionado por hash)

### config.rs
- CLI args via clap + env vars: `--server`, `--port`, `--static-dir`,
  `--package-zip`, `--udp-port-range`, `--console-commands`, `--public-ip`, etc.
- `get_console_commands()` helper for parsing comma-separated commands

### signaling.rs
- Creates RTCPeerConnection with STUN server (stun.l.google.com)
- Two data channels: `write` (proxy→browser), `read` (browser→proxy)
- SDP offer/answer exchange over WebSocket; ICE candidate trickle
- Starts Bridge when both channels open
- `--public-ip` announced as host/ICE candidate (deploy: IP da LAN `192.168.15.54`
  ou público via `WATCH_PUBLIC_IP`); resolve candidatos mDNS `.local` do browser

### bridge.rs
- Per-client UDP socket connected to the HLTV relay
- `forward_udp_to_webrtc()`: UDP recv → write channel send; reescreve o ack de
  conexão do HLTV (`"B"+cookie` → `"B"`); **teardown de idle** após
  `IDLE_TIMEOUT = 25s` sem pacote do upstream (armado em `saw_packet ||
  browser_started`), que fecha os canais e sinaliza o cliente a reconectar
- `setup_webrtc_to_udp()`: read channel on_message → UDP send; marca
  `browser_started` no primeiro pacote do browser
- Handles channel close/error for cleanup

## Web Client (client/)

### Dependencies (package.json)
- `xash3d-fwgs`: Xash3D engine WASM (xash.wasm, libmenu.wasm, filesystem_stdio.wasm, renderers)
- `cs16-client`: CS 1.6 client WASM (client_emscripten_wasm32.wasm, extras.pk3)

### vite.config.ts
- Copies WASM files from node_modules to dist automatically
- Copies cstrike/ directory from cs16-client
- `valve.zip` NÃO entra no build (servido pelo proxy via `--package-zip`)
- Output to `../../dist` (project root)

### main.ts
- Fetches `/config` for server configuration
- Loads valve.zip (jszip) and extras.pk3
- Initializes Xash3DWebRTC with library paths from config
- `module.requestAnimationFrame/cancelAnimationFrame` usam o shim global
- **Watchdog de stall** (intervalo 2s): sem pacote por `STALL_MS = 15000` →
  `rejoin()`; após `REJOIN_ATTEMPTS_BEFORE_RELOAD = 6` → reload **silencioso**
  (guarda `sessionStorage.watchStallReloaded`)
- **visibilitychange**: aba oculta >60s → `rejoin()` (desconecta+conecta no
  mesmo canal WebRTC, sem redownload do valve.zip)
- Loading por estágios PT-BR com barra de progresso; `window.onerror`/WebGL2
  check mostram erros no overlay

### webrtc.ts
- `Xash3DWebRTC` class extends `Xash3D` from xash3d-fwgs
- WebSocket to `/websocket` for signaling
- `write` channel: receives server packets, `lastPacketAt = Date.now()`, enqueues to `net.incoming`
- `net.incoming` com `maxPackets: 8192` (~5,7 min a 24fps) — evita quebrar a
  cadeia delta do HLTV com a aba oculta
- `read` channel: sendto() sends packets to server (virtual address 127.0.0.1:8080)
- `onclose`/`onerror` do canal `write` e estado `failed` da peer → `scheduleReconnect()`
  (reconnect com backoff exponencial 1s→30s)

## Build Commands

```bash
# Rust proxy
cargo build --release

# Web client (valve.zip servido pelo proxy — não precisa estar em client/src)
cd client && npm install && npm run build

# No deploy (csserver_wstats): o build do cliente+proxy é feito pelo
# ./scripts/watch.sh build (imagem csserver_wstats-watch-main multi-stage).
```

## Key APIs

### GET /config
Returns configuration for the web client:
```json
{
  "arguments": ["-windowed", "-game", "cstrike"],
  "console": ["spec_autodirector 1"],
  "game_dir": "cstrike",
  "libraries": {
    "client": "/cstrike/cl_dlls/client_emscripten_wasm32.wasm",
    "server": "/cstrike/dlls/cs_emscripten_wasm32.wasm",
    "extras": "/cstrike/extras.pk3",
    "menu": "/cstrike/cl_dlls/menu_emscripten_wasm32.wasm",
    "filesystem": "/filesystem_stdio.wasm"
  },
  "dynamic_libraries": ["dlls/cs_emscripten_wasm32.so", "/rwdir/filesystem_stdio.wasm"],
  "files_map": {
    "dlls/cs_emscripten_wasm32.so": "/cstrike/dlls/cs_emscripten_wasm32.wasm",
    "dlls/hl_emscripten_wasm32.so": "/cstrike/dlls/cs_emscripten_wasm32.wasm",
    "/rwdir/filesystem_stdio.wasm": "/filesystem_stdio.wasm"
  },
  "proxy_host": "192.168.1.100",
  "proxy_port": 27016
}
```

Note: `proxy_host` uses `--public-ip` if provided, otherwise falls back to `--host`.

### WebSocket Signaling
```json
{"event": "offer", "data": {"type": "offer", "sdp": "..."}}
{"event": "answer", "data": {"type": "answer", "sdp": "..."}}
{"event": "candidate", "data": {"candidate": "...", "sdpMid": "...", "sdpMLineIndex": ...}}
```

## Required Assets

- `valve.zip` (Half-Life base assets: `valve/` com `.wad`, `.pak`) é **externo**
  — no deploy fica em `valve/valve.zip` no repo principal (gitignored, backup via
  `watch.sh backup`) e é montado read-only em `/valve/valve.zip` no container
  (`PACKAGE_ZIP`).
- Todo o resto é automático: WASM de `xash3d-fwgs`, arquivos CS 1.6 de `cs16-client`.

## Common Tasks

### Add new CLI option
1. Add field to `Config` struct in `config.rs`
2. Add `#[arg(...)]` attribute with clap options
3. Use in `main.rs` or pass to handlers

### Modify client UI / behavior
1. Edit `client/src/index.html`, `main.ts` ou `webrtc.ts`
2. `npm run build` em `client/` (ou `./scripts/watch.sh build` no deploy — o
   rebuild do cliente exige rebuild completo da imagem, ver README do repo
   principal)

### Change WebRTC settings
- Data channel options: `signaling.rs` RTCDataChannelInit
- ICE servers: `signaling.rs` RTCConfiguration
- NAT traversal: Use `--public-ip` flag
- Portas ICE fixas: `--udp-port-range` / `UDP_PORT_RANGE` (abrir no firewall/NAT)

### Change game directory
- Use `--game-dir valve` for Half-Life
- Use `--game-dir cstrike` for CS 1.6 (default)

## Server Requirements

### ReUnion Module
O servidor de jogo precisa de ReUnion para aceitar clientes não-Steam
(protocol 47/48).

### HLTV relay
Neste deploy o proxy aponta para o relay HLTV (`watch-hltv`, porta `27020`),
não direto para o servidor de jogo. O relay usa `hltv.cfg` + `start-hltv.sh`
em `config/watch/` (loop de auto-restart, pidfile p/ healthcheck, crash log em
`live/watch/last_hltv_crash.txt`).

## Debugging

### Proxy
```bash
cargo run -- --server 127.0.0.1:27020 -v --static-dir ./dist
```

### Client
- Browser DevTools Console for JS errors
- Network tab for WebSocket messages
- `chrome://webrtc-internals` for WebRTC debugging

### Common Issues
- "Unsupported Extension Type" warnings: Safe to ignore (WebRTC SCTP)
- Espectador congelado: o watchdog/rejoin/reload se auto-recuperam; se o relay
  HLTV ficou "mudo", reiniciar o servidor de jogo (HLTV reconecta em ~20s).
  Ver `live/watch/last_hltv_crash.txt` e `./scripts/watch.sh status`.
- Conexão falha: verificar ReUnion no servidor, portas UDP/TCP `27018` +
  `27019-27050` + `4445` abertas, e `WATCH_PUBLIC_IP` para acesso externo.
