import {Net, Packet, Xash3D, Xash3DOptions} from "xash3d-fwgs";

export interface Xash3DWebRTCOptions extends Xash3DOptions {
    proxyHost: string;
    proxyPort: number;
}

export class Xash3DWebRTC extends Xash3D {
    private channel?: RTCDataChannel
    private resolve?: (value?: unknown) => void
    private ws?: WebSocket
    private peer?: RTCPeerConnection
    private remoteDescription?: RTCSessionDescription
    private candidates: RTCIceCandidateInit[] = []
    private wasRemote = false
    private reconnectTimer?: ReturnType<typeof setTimeout>
    private reconnectDelay = 1000
    private keepaliveTimer?: ReturnType<typeof setInterval>
    private proxyHost: string
    private proxyPort: number
    private proxyIp: [number, number, number, number]
    /** Timestamp of the last game packet received from the proxy (stall watchdog). */
    lastPacketAt = 0
    /** How many packets the RollingBuffer dropped because it overflowed (diagnostics). */
    overflowDrops = 0
    private maxPackets = 16384
    /** True once the initial WebSocket/peer pair has fully opened its channels;
     *  a later channelsCount===2 means a transport rebuild, not the first connect. */
    private wasConnected = false
    /** Called when a rebuilt WebSocket/peer pair opens its channels. The engine's
     *  netchan baseline is stale after a transport reset even though packets flow
     *  again, so main.ts rejoins to re-sync the HLTV delta chain. */
    onReconnected?: () => void

    constructor(opts: Xash3DWebRTCOptions) {
        super(opts);
        // Large packet backlog so the engine never drops packets while the tab
        // is hidden (rAF throttled ~1fps). Dropping the oldest packets breaks the
        // HLTV delta chain and permanently freezes the renderer ("delta frame is
        // too old" -> validsequence=0). The relay also produces faster than the
        // engine consumes (1 packet per frame via recvfrom), so the buffer fills
        // up over a few minutes even with an active tab — the main.ts watchdog
        // rejoins at 80% full to reset the chain before any drop.
        this.net = new Net(this, { maxPackets: this.maxPackets })
        this.proxyHost = opts.proxyHost
        this.proxyPort = opts.proxyPort
        this.proxyIp = this.parseIp(opts.proxyHost)
    }

    /** Current number of queued incoming packets (used by the backlog watchdog). */
    netBacklog(): number {
        return (this.net as Net).incoming.size()
    }

    /** Capacity of the incoming packet buffer (netBacklog() / netCapacity()). */
    netCapacity(): number {
        return this.maxPackets
    }

    /** Drop every queued incoming packet (called on rejoin to clear stale session data). */
    netClear(): void {
        (this.net as Net).incoming.clear()
    }

    private enqueueIncoming(packet: Packet) {
        const incoming = (this.net as Net).incoming
        if (incoming.isFull()) {
            // The RollingBuffer push() would drop the OLDEST packet, which breaks
            // the HLTV delta chain and permanently freezes the renderer. main.ts
            // should rejoin before this ever happens; log anyway for diagnosis.
            this.overflowDrops += 1
            console.warn(`[net] buffer cheio (${incoming.size()}/${this.maxPackets}), pacote antigo descartado — cadeia delta quebrada (drop #${this.overflowDrops})`)
        }
        incoming.enqueue(packet)
    }

    private parseIp(host: string): [number, number, number, number] {
        const parts = host.split('.').map(Number)
        if (parts.length === 4 && parts.every(p => p >= 0 && p <= 255)) {
            return parts as [number, number, number, number]
        }
        // Default to 127.0.0.1 for non-IP hostnames
        return [127, 0, 0, 1]
    }

    async init() {
        await Promise.all([
            super.init(),
            this.connect()
        ]);
    }

    startConnection() {
        this.peer = new RTCPeerConnection({
            iceServers: [{ urls: 'stun:stun.l.google.com:19302' }]
        })
        this.peer.onicecandidate = e => {
            if (!e.candidate) {
                return
            }
            this.wsSend('candidate', e.candidate.toJSON())
        }
        let el: HTMLAudioElement | undefined
        this.peer.ontrack = (e) => {
            el = document.createElement(e.track.kind) as HTMLAudioElement
            el.srcObject = e.streams[0]
            el.autoplay = true
            el.setAttribute('playsinline', '')
            el.controls = true
            document.body.appendChild(el)

            e.track.onmute = () => {
                el?.play()
            }

            e.streams[0].onremovetrack = () => {
                if (el?.parentNode) {
                    el?.parentNode?.removeChild(el)
                    el = undefined
                }
            }
        }
        this.peer.onconnectionstatechange = () => {
            if (el?.parentNode) {
                el.parentNode.removeChild(el)
                el = undefined
            }
            if (this.peer?.connectionState === 'failed') {
                this.peer.close()
                this.peer = undefined
                this.remoteDescription = undefined
                this.candidates = []
                this.wasRemote = false
                this.scheduleReconnect()
            }
        }
        let channelsCount = 0
        this.peer.ondatachannel = (e) => {
            if (e.channel.label === 'write') {
                e.channel.onmessage = (ee) => {
                    this.lastPacketAt = Date.now()
                    const packet: Packet = {
                        ip: this.proxyIp,
                        port: this.proxyPort,
                        data: ee.data
                    }
                    if (ee.data.arrayBuffer) {
                        ee.data.arrayBuffer().then((data: Int8Array) => {
                            packet.data = data;
                            this.enqueueIncoming(packet)
                        })
                    } else {
                        this.enqueueIncoming(packet)
                    }
                }
            }
            e.channel.onclose = () => {
                // The proxy tears down the channels when the upstream (HLTV /
                // game server) dies; the WebRTC peer otherwise stays "connected"
                // via SCTP keepalives and the client would freeze forever.
                if (e.channel.label === 'read') {
                    this.channel = undefined
                }
                this.scheduleReconnect()
            }
            e.channel.onerror = () => {
                if (e.channel.label === 'read') {
                    this.channel = undefined
                }
                this.scheduleReconnect()
            }
            e.channel.onopen = () => {
                channelsCount += 1
                if (e.channel.label === 'read') {
                    this.channel = e.channel
                }
                if (channelsCount === 2) {
                    if (this.wasConnected) {
                        // Transport rebuilt after a reset: the engine's delta
                        // chain is stale even if packets flow again, so signal
                        // main.ts to rejoin instead of freezing on a bad baseline.
                        this.onReconnected?.()
                    } else if (this.resolve) {
                        this.wasConnected = true
                        const r = this.resolve
                        this.resolve = undefined
                        r()
                    }
                }
            }
        }
        this.handleDescription()
    }

    private wsSend(event: string, data: unknown) {
        const msg = JSON.stringify({
            event,
            data
        })
        this.ws?.send(msg)
    }

    private async handleDescription() {
        if (!this.remoteDescription || !this.peer) return

        await this.peer!.setRemoteDescription(this.remoteDescription)
        this.remoteDescription = undefined
        const answer = await this.peer!.createAnswer()
        await this.peer!.setLocalDescription(answer)
        this.wsSend('answer', answer)
        this.wasRemote = true
        this.handleCandidates()
    }

    private handleCandidates() {
        if (!this.candidates.length || !this.peer) return

        const candidates = this.candidates
        this.candidates = []
        candidates.forEach(c => {
            this.peer!.addIceCandidate(c).catch(() => {
                this.candidates.push(c)
            })
        })
    }

    private scheduleReconnect() {
        if (this.reconnectTimer) {
            clearTimeout(this.reconnectTimer)
        }
        const delay = this.reconnectDelay
        this.reconnectDelay = Math.min(this.reconnectDelay * 2, 30000)
        this.reconnectTimer = setTimeout(() => {
            this.reconnectTimer = undefined
            this.connectWs()
        }, delay)
    }

    /** Immediately rebuild the WebSocket + WebRTC peer (watchdog escalation for
     *  when scheduleReconnect isn't firing and a page reload is guard-blocked).
     *  The rebuilt pair triggers onReconnected, which rejoins the game. */
    forceReconnect(): void {
        if (this.reconnectTimer) {
            clearTimeout(this.reconnectTimer)
            this.reconnectTimer = undefined
        }
        this.reconnectDelay = 1000
        this.connectWs()
    }

    private connectWs() {
        if (this.reconnectTimer) {
            clearTimeout(this.reconnectTimer)
            this.reconnectTimer = undefined
        }
        this.stopKeepalive()
        if (this.ws) {
            const old = this.ws
            old.onopen = null
            old.onerror = null
            old.onclose = null
            old.close()
        }
        const protocol = window.location.protocol === "https:" ? "wss" : "ws";
        const host = window.location.host;
        const handler = async (e: MessageEvent) => {
            const parsed = JSON.parse(e.data)
            switch (parsed.event) {
                case 'offer':
                    this.remoteDescription = parsed.data
                    await this.handleDescription()
                    break
                case 'candidate':
                    this.candidates.push(parsed.data)
                    if (this.wasRemote) {
                        this.handleCandidates()
                    }
                    break
            }
        }
        this.ws = new WebSocket(`${protocol}://${host}/websocket`);
        this.ws.onerror = () => {
            this.stopKeepalive()
            this.scheduleReconnect()
        }
        this.ws.onclose = () => {
            this.stopKeepalive()
        }
        this.ws.addEventListener('message', handler)
        this.ws.onopen = () => {
            this.reconnectDelay = 1000
            this.startKeepalive()
            this.startConnection()
        }
    }

    /** Keep the signaling WebSocket busy so intermediate proxies/NAT (swag,
     *  router) don't idle-timeout it. The game data flows over WebRTC/UDP, so
     *  the WS goes quiet after the handshake; a 240s idle reset used to force a
     *  full WebRTC rebuild (~2-3s stream gap) every 4 minutes. */
    private startKeepalive() {
        this.stopKeepalive()
        this.keepaliveTimer = setInterval(() => {
            if (this.ws && this.ws.readyState === WebSocket.OPEN) {
                this.ws.send(JSON.stringify({ event: 'ping' }))
            }
        }, 60000)
    }

    private stopKeepalive() {
        if (this.keepaliveTimer) {
            clearInterval(this.keepaliveTimer)
            this.keepaliveTimer = undefined
        }
    }

    async connect() {
        return new Promise(resolve => {
            this.resolve = resolve;
            this.connectWs()
        })
    }

    sendto(packet: Packet) {
        // The channel may be mid-teardown when the engine calls sendto (the
        // proxy closes the channels when the upstream dies); sending on a
        // closed channel throws InvalidStateError which aborts the WASM frame.
        if (!this.channel || this.channel.readyState !== 'open') return
        try {
            this.channel.send(packet.data)
        } catch (err) {
            // SCTP buffer full (QuotaExceededError) or channel raced to close:
            // drop the packet instead of crashing the engine.
            console.warn('[net] sendto falhou, descartando pacote', err)
        }
    }
}