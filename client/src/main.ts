import {loadAsync} from 'jszip'
import xashURL from 'xash3d-fwgs/xash.wasm?url'
import gl4esURL from 'xash3d-fwgs/libref_webgl2.wasm?url'
import {Xash3DWebRTC} from "./webrtc";

let touchControls = document.getElementById('touchControls') as HTMLInputElement
touchControls.addEventListener('change', () => {
    localStorage.setItem('touchControls', String(touchControls.checked))
})

let x: Xash3DWebRTC | undefined
let started = false
let configHost = ''
let configPort = 0

const stageEl = document.getElementById('stage') as HTMLElement
const percentEl = document.getElementById('percent') as HTMLElement
const fillEl = document.getElementById('barFill') as HTMLElement
const loadingEl = document.getElementById('loading') as HTMLElement
const errorEl = document.getElementById('error') as HTMLElement

function setStage(text: string, fraction?: number) {
    stageEl.textContent = text
    if (typeof fraction === 'number') {
        fillEl.classList.remove('indeterminate')
        const pct = Math.max(0, Math.min(100, Math.round(fraction * 100)))
        fillEl.style.width = pct + '%'
        percentEl.textContent = pct + '%'
    } else {
        fillEl.classList.add('indeterminate')
        fillEl.style.width = '100%'
        percentEl.textContent = '…'
    }
}

function showError(message: string) {
    loadingEl.classList.add('error')
    stageEl.textContent = 'Ocorreu um erro'
    percentEl.textContent = ''
    errorEl.style.display = 'block'
    errorEl.textContent = message
}

window.addEventListener('error', (e) => {
    const msg = e.message || 'Erro desconhecido'
    if (msg !== 'Script error.') {
        showError(msg)
    }
})
window.addEventListener('unhandledrejection', (e) => {
    showError(e.reason instanceof Error ? e.reason.message : String(e.reason))
})

async function fetchWithProgress(url: string) {
    const res = await fetch(url);

    const contentLength = res.headers.get('Content-Length');

    const total = contentLength ? parseInt(contentLength, 10) : null;
    const reader = res.body!.getReader();
    const chunks = [];
    let received = 0;

    setStage('Baixando arquivos…', 0)
    while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        chunks.push(value);
        received += value.length;

        if (total !== null) {
            setStage('Baixando arquivos…', received / total)
        } else {
            setStage('Baixando arquivos…')
        }
    }

    const blob = new Blob(chunks);
    return blob.arrayBuffer()
}

async function main() {
    // Load dynamic configuration from server (environment variables)
    const config = await fetch("/config").then(res => res.json()) as Awaited<{
        arguments: string[];
        console: string[];
        game_dir: string;
        spectator_name: string;
        libraries: {
            client: string;
            server: string;
            extras: string;
            menu: string;
            filesystem: string;
        };
        dynamic_libraries: string[];
        files_map: Record<string, string>;
        proxy_host: string;
        proxy_port: number;
    }>

    const canvas = document.getElementById('canvas') as HTMLCanvasElement
    const probe = document.createElement('canvas')
    if (!window.WebGL2RenderingContext || !probe.getContext('webgl2')) {
        showError('Seu navegador não suporta WebGL2, necessário para o espectador.\nAtualize o navegador ou tente em outro dispositivo.')
        return
    }

    // Use URLs directly from server config (no imports needed)
    x = new Xash3DWebRTC({
        canvas,
        arguments: config.arguments || ['-windowed'],
        libraries: {
            filesystem: config.libraries.filesystem,
            xash: xashURL,
            menu: config.libraries.menu,
            server: config.libraries.server,
            client: config.libraries.client,
            render: {
                gl4es: gl4esURL,
            }
        },
        dynamicLibraries: config.dynamic_libraries,
        filesMap: config.files_map,
        proxyHost: config.proxy_host,
        proxyPort: config.proxy_port,
        module: {
            // Route the engine main loop through the visibility-aware shim so the
            // game keeps simulating (and the netchan stays alive) while the tab is
            // hidden. The global window shim covers Emscripten's window.* path.
            requestAnimationFrame: (cb: FrameRequestCallback) => window.requestAnimationFrame(cb),
            cancelAnimationFrame: (id: number) => window.cancelAnimationFrame(id),
        },
    });

    configHost = config.proxy_host
    configPort = config.proxy_port

    // Transport rebuilds (WS/peer reset) deixam o engine com a baseline netchan
    // velha: os pacotes voltam a fluir (o watchdog de stall dorme) mas a cadeia
    // delta quebrou. Rejoin re-sincroniza a baseline — sem isso um reset de
    // transporte congela a tela para sempre mesmo com stream saudável.
    x.onReconnected = () => { if (started) rejoin() }

    let bootDone = false
    const [zip, extras] = await Promise.all([
        (async () => {
            const res = await fetchWithProgress('valve.zip')
            setStage(bootDone ? 'Preparando arquivos…' : 'Carregando, aguarde…')
            return await loadAsync(res);
        })(),
        (async () => {
            const res = await fetch(config.libraries.extras)
            return await res.arrayBuffer();
        })(),
        (async () => {
            const r = await x!.init()
            bootDone = true
            return r
        })(),
    ])

    const files = Object.entries(zip.files).filter(([, file]) => !file.dir)
    setStage('Descompactando…', 0)
    let written = 0
    for (const [filename, file] of files) {
        const path = '/rodir/' + filename;
        const dir = path.split('/').slice(0, -1).join('/');

        x.em.FS.mkdirTree(dir);
        x.em.FS.writeFile(path, await file.async("uint8array"));
        written += 1
        setStage('Descompactando…', written / files.length)
    }

    x.em.FS.writeFile(`/rodir/${config.game_dir}/extras.pk3`, new Uint8Array(extras))
    x.em.FS.chdir('/rodir')

    const form = document.getElementById('form') as HTMLFormElement
    form.style.display = 'none'

    const username = config.spectator_name
    setStage('Conectando…', 1)
    loadingEl.classList.add('fade-out')
    started = true
    x.main()
    if (touchControls.checked) {
        x.Cmd_ExecuteString('touch_enable 1')
    }
    x.Cmd_ExecuteString(`name "${username}"`)

    // Execute custom server commands
    if (config.console && Array.isArray(config.console)) {
        config.console.forEach((cmd: string) => {
            x!.Cmd_ExecuteString(cmd)
        })
    }

    x.Cmd_ExecuteString(`connect ${configHost}:${configPort}`)
}

// Game-level rejoin: drop the HLTV spectator session and reconnect over the
// (still-alive) WebRTC channel. Used for long hidden tabs, stream stalls and
// for the backlog high-watermark, avoiding a full reload (which would
// re-download valve.zip).
let rejoinInFlight = false
function rejoin() {
    if (rejoinInFlight || !x || !started) return
    rejoinInFlight = true
    try {
        // Descarta o backlog da sessão anterior: pacotes velhos enfileirados
        // seriam entregues à nova conexão e quebrariam a cadeia delta dela.
        x.netClear()
    } catch {}
    try {
        x.Cmd_ExecuteString('disconnect')
    } catch {}
    setTimeout(() => {
        try {
            // Limpa de novo o que chegou durante o desconecte (a fila do relay
            // antigo ainda drena por alguns instantes) antes de abrir a nova
            // sessão.
            x!.netClear()
        } catch {}
        try {
            x!.Cmd_ExecuteString(`connect ${configHost}:${configPort}`)
        } catch {}
        rejoinInFlight = false
    }, 300)
}

// The net.incoming backlog (maxPackets 16384, ~11min at 24fps) keeps the delta
// chain intact for hides below that: on refocus the engine drains it in order and
// fast-forwards to live instead of freezing. For hides long enough that the
// buffer could overflow (or cl_timeout), rejoin over the still-alive WebRTC
// channel instead of reloading the page (avoids re-downloading valve.zip).
let hiddenSince = 0
document.addEventListener('visibilitychange', () => {
    if (document.hidden) {
        hiddenSince = Date.now()
        return
    }
    const hiddenFor = Date.now() - hiddenSince
    if (started && x && hiddenFor > 60000) {
        rejoin()
    }
})

// Stream stall watchdog (active tab): the game/HLTV upstream can die without
// the WebRTC connection noticing (SCTP keepalives keep the peer "connected"
// while packets just stop arriving). If no game packet arrives for STALL_MS,
// force a rejoin; escalate to a single full reload so a wedged bridge/peer
// never leaves a frozen screen. The reload guard (sessionStorage) prevents a
// reload loop while the game server is genuinely down.
const STALL_MS = 15000
const REJOIN_ATTEMPTS_BEFORE_RELOAD = 6
// Overflow do buffer é fatal: o RollingBuffer descarta o pacote MAIS ANTIGO
// quando enche (quebra a cadeia delta do HLTV e congela o renderer para
// sempre), e o watchdog de stall NÃO age porque os pacotes continuam chegando
// (lastPacketAt segue atualizando). O relay produz mais rápido que o engine
// consome (1 pacote/frame via recvfrom), então o backlog cresce por alguns
// minutos mesmo com a aba ativa. Rejoin proativo ao atingir o teto, para
// resetar a cadeia delta (via reconexão no canal WebRTC vivo) antes de
// qualquer descarte.
const BACKLOG_HIGH_WATERMARK = 0.8
let stallCount = 0
let backlogRejoins = 0
setInterval(() => {
    if (!started || !x || document.hidden) return
    const lastPacketAt = x.lastPacketAt
    if (!lastPacketAt) return

    const backlog = x.netBacklog()
    const capacity = x.netCapacity()
    if (backlog >= capacity * BACKLOG_HIGH_WATERMARK) {
        backlogRejoins += 1
        console.warn(`[backlog] buffer em ${backlog}/${capacity} (${Math.round((backlog / capacity) * 100)}%) — rejoin para resetar a cadeia delta (${backlogRejoins}º; drops: ${x.overflowDrops})`)
        rejoin()
        return
    }

    const idleMs = Date.now() - lastPacketAt
    if (idleMs <= STALL_MS) {
        stallCount = 0
        return
    }
    stallCount += 1
    console.warn(`[watchdog] stream sem dados há ${Math.round(idleMs / 1000)}s (tentativa ${stallCount})`)
    if (stallCount >= REJOIN_ATTEMPTS_BEFORE_RELOAD) {
        if (!sessionStorage.getItem('watchStallReloaded')) {
            sessionStorage.setItem('watchStallReloaded', '1')
            console.error('[watchdog] rejoin falhou, recarregando a página')
            location.reload()
            return
        }
        // Guard já consumido (a página já recarregou): em vez de rejoin infinito
        // sobre um canal possivelmente morto, reconstrói o transporte WebRTC —
        // o onReconnected re-sincroniza a baseline via rejoin. Só a cada 12
        // stalls (~24s) para não virar churn.
        if (stallCount % 12 === 0) {
            console.error('[watchdog] canal morto após reload — forçando reconnect WebRTC')
            x.forceReconnect()
            return
        }
    }
    rejoin()
}, 2000)

const enableTouch = localStorage.getItem('touchControls')
if (enableTouch === null) {
    const isMobile = !window.matchMedia('(hover: hover)').matches;
    touchControls.checked = isMobile
    localStorage.setItem('touchControls', String(isMobile))
} else {
    touchControls.checked = enableTouch === 'true'
}

main()
