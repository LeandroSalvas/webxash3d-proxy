# Patches aplicados sobre o upstream

Fork de `bordeux/webxash3d-proxy` (base: commit `9cb046f`, release 1.0.0) com
as seguintes modificações locais, necessárias para o espectador de CS 1.6 via
browser funcionar de forma estável.

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
- **Auto-reconexão em `visibilitychange`**: se a aba voltou após >15s oculta,
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

## Configuração de deploy

A config de deployment (hltv.cfg, start-hltv.sh) vive em `config/watch/` no
repositório principal; o fork fica genérico.
