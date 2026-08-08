# Changelog

## Fork `LeandroSalvas` (espectador CS 1.6)

Base: upstream `9cb046f` (1.0.0). Alterações em `PATCHES.md`.

## 1.4.0 (2026-08-08)

* fix: rejoin after transport rebuild, force-reconnect escalation and WS keepalive — reset de transporte (WS/peer WebRTC) deixava o engine com a baseline netchan velha: os pacotes voltavam a fluir (watchdog de stall quieto, pois `lastPacketAt` seguia atualizando) mas a cadeia delta do HLTV quebrada congelava a tela **para sempre** mesmo com o stream saudável. `wasConnected`/`onReconnected` em `webrtc.ts` (rebuild de transporte → rejoin para re-sincronizar a baseline em ~2s), `forceReconnect()` no watchdog quando a guarda de reload já foi consumida (a cada 12 stalls, em vez de `rejoin()` infinito). Causa raiz dos resets periódicos: timeout de ociosidade de ~240s no WS de sinalização (dados vão por WebRTC/UDP) — keepalive `{"event":"ping"}` a cada 60s no cliente + handler silencioso no `signaling.rs` (veja `PATCHES.md` §12/§13)

## 1.3.0 (2026-08-08)

* fix: proactive rejoin on net.incoming overflow — active-tab freeze ([075ba61](https://github.com/LeandroSalvas/webxash3d-proxy/commit/075ba61)) — o relay produz mais rápido que o engine consome (1 pacote/frame via `recvfrom`); o `RollingBuffer` enchia em ~3-7 min e o descarte do pacote mais antigo quebrava a cadeia delta do HLTV (congelamento permanente com a aba ativa, watchdog de stall quieto pois pacotes continuavam chegando). Watchdog de backlog (`rejoin()` a 80% do buffer), `rejoin()` limpa o backlog da sessão anterior (`netClear()`), `maxPackets` 8192 → 16384, contadores `overflowDrops`/`backlogRejoins` + logs `[net]`/`[backlog]`, e stub de `getUserMedia` no `index.html` (o glue do engine pedia a permissão de microfone no boot)

## 1.2.0 (2026-08-07)

* feat: self-healing spectator with silent reload and upstream watchdog ([763895d](https://github.com/LeandroSalvas/webxash3d-proxy/commit/763895d)) — watchdog de stall no cliente (`STALL_MS=15000` → rejoin → reload silencioso após 6 tentativas, guarda `sessionStorage`), teardown de idle na bridge (`IDLE_TIMEOUT=25s`, armado em `saw_packet || browser_started`), reconexão em close/error do canal, `beforeunload` removido
* fix: keep HLTV delta chain intact while tab is hidden ([88d5901](https://github.com/LeandroSalvas/webxash3d-proxy/commit/88d5901)) — `maxPackets: 8192` no `net.incoming`
* fix: prevent ASI bug merging chdir with following statement ([4e6dab6](https://github.com/LeandroSalvas/webxash3d-proxy/commit/4e6dab6))
* feat: robust spectator client with staged loading UI ([4699baf](https://github.com/LeandroSalvas/webxash3d-proxy/commit/4699baf)) — loading PT-BR por estágios, `window.onerror`/WebGL2 check, `getUserMedia` removido, rAF shim ciente de visibilidade
* feat: spectator mode for CS 1.6 HLTV relay via browser ([49e6430](https://github.com/LeandroSalvas/webxash3d-proxy/commit/49e6430)) — ack de conexão do HLTV reescrito, `PACKAGE_ZIP`/`UDP_PORT_RANGE`/`CONSOLE_COMMANDS`, build Docker multi-stage

## 1.0.0 (2026-01-11)

* Merge pull request #1 from bordeux/feature/initial ([46e9466](https://github.com/bordeux/webxash3d-proxy/commit/46e9466)), closes [#1](https://github.com/bordeux/webxash3d-proxy/issues/1)
* Merge pull request #2 from bordeux/feature/initial ([22f5ca4](https://github.com/bordeux/webxash3d-proxy/commit/22f5ca4)), closes [#2](https://github.com/bordeux/webxash3d-proxy/issues/2)
* Merge remote-tracking branch 'origin/master' into feature/initial ([6b3bc9c](https://github.com/bordeux/webxash3d-proxy/commit/6b3bc9c))
* fix: fix repo url ([8af4370](https://github.com/bordeux/webxash3d-proxy/commit/8af4370))
* feat: add cargo-make for unified build orchestration ([143f514](https://github.com/bordeux/webxash3d-proxy/commit/143f514))
* feat: add dynamic proxy address configuration ([cb79c78](https://github.com/bordeux/webxash3d-proxy/commit/cb79c78))
* feat: add embedded assets, CI/CD workflows, and code quality tools ([3de23a8](https://github.com/bordeux/webxash3d-proxy/commit/3de23a8))
* feat: add nvm support for consistent Node.js version ([a1e3de8](https://github.com/bordeux/webxash3d-proxy/commit/a1e3de8))
* docs: fix ([da3b64d](https://github.com/bordeux/webxash3d-proxy/commit/da3b64d))
* docs: update README with Mermaid diagrams and architecture explanation ([5c50516](https://github.com/bordeux/webxash3d-proxy/commit/5c50516))
* chore: add fmt ([328d7f9](https://github.com/bordeux/webxash3d-proxy/commit/328d7f9))
* chore: initial commit ([faa4a2f](https://github.com/bordeux/webxash3d-proxy/commit/faa4a2f))
* chore: initial commit ([7ad9c77](https://github.com/bordeux/webxash3d-proxy/commit/7ad9c77))
* chore: refactoring ([c9255b4](https://github.com/bordeux/webxash3d-proxy/commit/c9255b4))
