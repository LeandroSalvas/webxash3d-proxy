# Patches aplicados sobre o upstream

Fork de `bordeux/webxash3d-proxy` (base: commit `9cb046f`, release 1.0.0) com
as seguintes modificações locais, necessárias para o espectador de CS 1.6 via
browser funcionar de forma estável (incluindo auto-recuperação em quedas do
upstream).

## 1. Ack de conexão do HLTV (src/bridge.rs)

O relay HLTV (rehlds) responde ao `connect 48` com um pacote *out-of-band*
formado por `"B" + cookie` (16 zeros), ex.: `\xff\xff\xff\xffB0000000000000000`
(21 bytes). O servidor de jogo normal responde com o formato com espaços:
`\xff\xff\xff\xffB <playerid> "ip" <prot> <auth>`. O cliente Xash3D
(`CL_ConnectionlessPacket`, `cl_main.c`) faz comparação estrita do comando
contra `"B"`, ignora a forma fundida do HLTV e permanece em `ca_connecting`,
re-enviando `getchallenge` a cada `cl_resend`.

Correção: `forward_udp_to_webrtc` reescreve o ack exato de 21 bytes
`\xff\xff\xff\xffB0000000000000000` para o pacote estático
`\xff\xff\xff\xffB\n` (aceito pelo cliente). O log registra a reescrita.

## 2. Handshake DTLS com browsers modernos (vendor-webrtc-dtls)

O Chrome oferece curvas elípticas pós-quânticas (ex.: `X25519MLKEM768`) como
primeira escolha. O `webrtc-rs` 0.11 assumia a primeira curva oferecida pelo
cliente e falhava o handshake DTLS com `ErrInvalidNamedCurve`.

Correção: crate `webrtc-dtls` vendido em `vendor-webrtc-dtls/` com o servidor
selecionando uma curva suportada, aplicado via `[patch.crates-io]` no
`Cargo.toml`.

## 3. Cliente espectador (client/src)

- Auto-start sem formulário de nome (nome gerado pelo `/config`: `spectatorN`).
- Auto-reconnect com backoff exponencial (1s -> 30s) em falha de WS/ICE.
- `stun:stun.l.google.com:19302` nos ICE servers para NAT externo.
- Remoção do `valve.zip` do build Vite (servido pelo proxy via `--package-zip`).

## 4. Configuração por ambiente (src/config.rs, src/main.rs, src/signaling.rs)

- `PACKAGE_ZIP`: serve `valve.zip` de um caminho externo (sem embutir no build).
- `UDP_PORT_RANGE` (ex.: `27019-27050`): faixa fixa de portas ICE para abrir no
  firewall/NAT.
- `CONSOLE_COMMANDS`: comandos extras executados no cliente (ex.:
  `spec_autodirector 1`).
- `spectator_name` no `/config` e limpeza dos data channels no encerramento.

## 5. Build (Dockerfile)

- Build do cliente (Node/Vite) + proxy Rust em imagem multi-stage.
- Runtime instala `wget` (usado pelo healthcheck do compose).

## 6. Robustez do cliente espectador (client/src)

- **Aba oculta não congela**: shim de `requestAnimationFrame` ciente de
  visibilidade (script inline no `<head>`, antes do módulo) + `module: {
  requestAnimationFrame, cancelAnimationFrame }` em `main.ts`. Com a aba oculta,
  o loop vira `setTimeout` (~1fps), então o netchan do cliente não estoura o
  `cl_timeout` e a sessão com o relay HLTV sobrevive ao voltar.
- **Backlog do net.incoming elevado**: `maxPackets: 8192` (~5,7 min a 24fps;
  hoje 16384, ver §9). O padrão (128 pacotes, ~5,3s) descartava os pacotes mais
  antigos quando a aba ficava oculta, quebrava a cadeia delta do HLTV ("delta
  frame is too old" → `cl.validsequence=0`) e congelava o renderer
  permanentemente. Com o backlog maior, escondidas curtas drenam os pacotes em
  ordem (fast-forward até o ao-vivo) em vez de congelar.
- **Auto-reconexão em `visibilitychange`**: se a aba voltou após >60s oculta,
  `disconnect` + `connect` pelo canal WebRTC ainda vivo (sem recarregar o
  `valve.zip`).
- **Microfone não é solicitado pelo código do cliente**: o `client/src` nunca
  chama `getUserMedia` (espectador não fala). Ainda assim o glue do engine
  (captura SDL2/AL de voice) disparava o prompt no boot — resolvido com o stub
  de rejeição do §10.
- **Tela de loading com estágios**: rótulos PT-BR ("Baixando arquivos…",
  "Descompactando…", "Conectando…"), barra customizada (track+fill) com % real
  e animação indeterminada no boot do engine. Substitui a barra nativa de 8px.
- **Erros visíveis**: `window.onerror`/`unhandledrejection` e checagem de
  WebGL2 exibem a mensagem no overlay em vez de tela preta.
- **Imagem**: crosshair do portal (`loading.png` 256px) substitui o logo do
  xash3d no carregamento; `favicon.png` é o favicon do site; ícones sociais
  (GitHub/Discord) removidos.

## 7. Auto-recuperação (watchdog de stall, reload silencioso e teardown de idle)

O upstream (HLTV / servidor de jogo) pode morrer sem o WebRTC perceber: os
keepalives SCTP mantêm o peer "Connected" mesmo com os pacotes parando de
chegar. Para o espectador não ficar congelado para sempre:

- `client/src/webrtc.ts`: `lastPacketAt` registra o timestamp do último pacote
  de jogo recebido (canal `write`). `onclose`/`onerror` do canal `write` e o
  estado `failed` da peer connection chamam `scheduleReconnect()` — o proxy
  derruba os canais quando o upstream morre, então o `onclose` destrava o
  cliente que de outra forma ficaria "Connected" para sempre.
- `client/src/main.ts`: watchdog de stall do stream (intervalo 2s, aba ativa).
  Se nenhum pacote de jogo chegar em `STALL_MS = 15000`, força `rejoin()`
  (disconnect + connect no mesmo canal WebRTC). Após
  `REJOIN_ATTEMPTS_BEFORE_RELOAD = 6` tentativas, recarrega a página uma única
  vez, de forma **silenciosa** (guarda `sessionStorage.watchStallReloaded`
  impede loop de reload com o servidor genuinamente fora).
- `src/bridge.rs`: timeout de idle no `forward_udp_to_webrtc` —
  `IDLE_TIMEOUT = 25s` sem pacote do upstream derruba a bridge (`teardown`),
  fechando os canais e sinalizando o cliente a reconectar via
  `scheduleReconnect()`. O timeout só arma quando o upstream respondeu
  (`saw_packet`) **ou** o browser iniciou o handshake de conexão
  (`browser_started`), para não derrubar a bridge durante o download do
  `valve.zip` / handshake.
- **`beforeunload` removido**: o diálogo nativo "As alterações que você fez
  talvez não sejam salvas" bloqueava o reload automático do watchdog; removido
  para o reload ser silencioso.
- **Sessão sobrevive ao restart do servidor de jogo**: o HLTV renegocia o ack
  de conexão com o espectador existente e o stream retoma sem reload.

## 8. Causa raiz conhecida: HLTV relay "mudo"

O relay HLTV pode ficar sem enviar dados aos espectadores com o processo vivo e
conectado ao servidor de jogo (foi o que congelou o espectador em 01:12:10,
antes do deploy do watchdog). A auto-recuperação da seção 7 destrava a sessão,
mas o destravamento definitivo é reiniciar o servidor de jogo (o HLTV reconecta
sozinho em ~20s). Mitigação operacional sugerida (follow-up): alerta
Grafana/Prometheus se o write-channel ficar 0 por N minutos, ou auto-restart do
`watch-hltv`.

## 9. Overflow do buffer → rejoin proativo (congelamento com a aba ativa)

O backlog do §6 protegia apenas a aba oculta. Com a aba **ativa**, o relay
produz mais rápido do que o engine consome — o `recvfrom` do `net.js` puxa
**1 pacote por frame** (~24-60/s) enquanto o relay HLTV broadcasta na própria
taxa de ticks. O `RollingBuffer` enche em alguns minutos (3-7 no deploy real) e
o `push()` (rb.js) descarta o pacote **mais antigo**, quebrando a cadeia delta
do HLTV de novo ("delta frame is too old" → `cl.validsequence=0`): o renderer
congela **permanentemente** e o watchdog de stall **não** age, porque os
pacotes continuam chegando (`lastPacketAt` segue atualizando). Sintomas: freeze
determinístico com a janela ativa, sem reload automático, áudio para e input
não destrava.

- `client/src/webrtc.ts`: `maxPackets` elevado a **16384** (mais folga);
  `overflowDrops` conta descartes reais e loga `[net] buffer cheio…`; helpers
  `netBacklog()`/`netCapacity()`/`netClear()` expostos ao watchdog.
- `client/src/main.ts`: watchdog de backlog no mesmo `setInterval` de 2s — se
  `netBacklog() ≥ 80%` da capacidade, `rejoin()` **proativo** (conta
  `backlogRejoins`, log `[backlog]`) para resetar a cadeia delta ANTES de
  qualquer descarte. O rejoin é uma reconexão pelo canal WebRTC ainda vivo (o
  relay manda base nova de deltas), não um reload.
- `rejoin()` agora chama `netClear()` no início **e** imediatamente antes do
  `connect`, para pacotes velhos da sessão anterior (ainda drenando do relay)
  não envenenarem a cadeia da nova sessão.
- **Relay (deploy)**: `sys_ticrate 30` no `config/watch/hltv.cfg` limita a
  produção a ~30 ticks/s (mesmo teto do `sv_maxupdaterate` do servidor de
  jogo), tornando o overflow improvável; o rejoin proativo cobre picos/bursts.

## 10. Microfone (getUserMedia) — stub de rejeição

O prompt de permissão de microfone não vinha do código do cliente: o glue do
engine `xash3d-fwgs` (`dist/generated/xash.js`, ~linha 5653-5654) chama
`navigator.mediaDevices.getUserMedia({ audio: true })` no init da captura
SDL2/AL (voice), que dispara o prompt no boot do espectador (e no iOS
Safari/iframe um `getUserMedia` sem gesto do usuário pendura o `connect()`).

Correção: stub inline no `<head>` do `index.html` (antes do módulo) que
sobrescreve `navigator.mediaDevices.getUserMedia` para rejeitar
(`Promise.reject`) quando `constraints` pedir `audio`/`video`. O `onError` do
glue (que apenas registra `mediaStreamError`) lida com a rejeição e o voice cai
em silêncio — sem prompt e sem afetar o áudio de saída.

## 11. Blindagem do `sendto` (canal fechado nunca aborta o frame) + watchdog do relay mudo (deploy)

### sendto sem exceção

O engine chama `Net.sendto()` **dentro do frame WASM** (via `invoke_vd`). Quando
o canal `read` está fechado/mid-teardown (o proxy derruba os canais quando o
upstream morre), `RTCDataChannel.send()` lança `InvalidStateError: readyState is
not 'open'` — a exceção propaga pelo WASM e o Emscripten **aborta o frame**:
congelamento permanente, sem reload, mesmo com o stream retomando (observado em
produção: relay mudo por ~85s, stream voltou, página continuou congelada).

- `client/src/webrtc.ts` `sendto()`: guard `!this.channel ||
  this.channel.readyState !== 'open'` → descarta o pacote em silêncio;
  `try/catch` em volta do `send` (cobre também `QuotaExceededError` — buffer
  SCTP cheio); `this.channel = undefined` nos handlers `onclose`/`onerror` do
  canal `read`. O glue de rede nunca lança para o engine.

### Watchdog do relay mudo (scripts/watch-mudo.sh, repo principal)

Recuperação via **kick RCON** do HLTV no servidor de jogo — `docker restart
cs16-watch-hltv` foi **comprovadamente ineficaz**: o relay reiniciou, reconectou
no servidor (jogo via `status` mostrou novo userid HLTV) e continuou mudo; o
kick força um handshake netchan novo e o stream voltou em ~20s.

- Detecção: o proxy loga `HLTV upstream stalled: no UDP data, tearing down
  bridge` quando derruba a bridge por idle (25s sem dados, com browser
  conectado). O watchdog (cron a cada minuto) age se esse evento ocorreu nos
  últimos 90s **e** não há `recving` recente (30s) — se o stream voltou sozinho,
  não age. Keepalives de 48 bytes não disparam o teardown (resetam o timer) e
  correspondem a stream saudável em jogo quieto.
- Ação: `servers.sh rcon main "kick ZueiraHLTV"`; se o RCON não acusar o kick,
  escala para `docker restart cs16-watch-hltv`. Cooldown de 5min entre ações e
  aviso no log após 3 episódios seguidos (sugerindo reiniciar o servidor de
  jogo, o destravamento definitivo).

## Configuração de deploy

A config de deployment (hltv.cfg, start-hltv.sh) vive em `config/watch/` no
repositório principal; o fork fica genérico.
