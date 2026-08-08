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
- **Backlog do net.incoming elevado**: `maxPackets: 8192` (~5,7 min a 24fps).
  O padrão (128 pacotes, ~5,3s) descartava os pacotes mais antigos quando a aba
  ficava oculta, quebrava a cadeia delta do HLTV ("delta frame is too old" →
  `cl.validsequence=0`) e congelava o renderer permanentemente. Com o backlog
  maior, escondidas curtas drenam os pacotes em ordem (fast-forward até o
  ao-vivo) em vez de congelar.
- **Auto-reconexão em `visibilitychange`**: se a aba voltou após >60s oculta,
  `disconnect` + `connect` pelo canal WebRTC ainda vivo (sem recarregar o
  `valve.zip`).
- **Microfone removido**: `getUserMedia` não é mais chamado (espectador não
  fala). Isso desbloqueia o iOS Safari/iframe, onde a permissão sem gesto do
  usuário travava o `connect()` e impedia o `x.main()`.
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

## Configuração de deploy

A config de deployment (hltv.cfg, start-hltv.sh) vive em `config/watch/` no
repositório principal; o fork fica genérico.
