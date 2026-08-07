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
    private proxyHost: string
    private proxyPort: number
    private proxyIp: [number, number, number, number]

    constructor(opts: Xash3DWebRTCOptions) {
        super(opts);
        this.net = new Net(this)
        this.proxyHost = opts.proxyHost
        this.proxyPort = opts.proxyPort
        this.proxyIp = this.parseIp(opts.proxyHost)
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
                    const packet: Packet = {
                        ip: this.proxyIp,
                        port: this.proxyPort,
                        data: ee.data
                    }
                    if (ee.data.arrayBuffer) {
                        ee.data.arrayBuffer().then((data: Int8Array) => {
                            packet.data = data;
                            (this.net as Net).incoming.enqueue(packet)
                        })
                    } else {
                        (this.net as Net).incoming.enqueue(packet)
                    }
                }
            }
            e.channel.onopen = () => {
                channelsCount += 1
                if (e.channel.label === 'read') {
                    this.channel = e.channel
                }
                if (channelsCount === 2) {
                    if (this.resolve) {
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

    private connectWs() {
        if (this.reconnectTimer) {
            clearTimeout(this.reconnectTimer)
            this.reconnectTimer = undefined
        }
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
            this.scheduleReconnect()
        }
        this.ws.addEventListener('message', handler)
        this.ws.onopen = () => {
            this.reconnectDelay = 1000
            this.startConnection()
        }
    }

    async connect() {
        return new Promise(resolve => {
            this.resolve = resolve;
            this.connectWs()
        })
    }

    sendto(packet: Packet) {
        if (!this.channel) return
        this.channel.send(packet.data)
    }
}