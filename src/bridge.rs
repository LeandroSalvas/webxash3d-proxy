use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;

/// Maximum packet size for `GoldSrc` protocol
const MAX_PACKET_SIZE: usize = 65536;

/// Bridge between WebRTC data channels and UDP socket to game server
///
/// Uses two channels to match the original client expectations:
/// - `write` channel: proxy sends TO browser (server → client)
/// - `read` channel: proxy receives FROM browser (client → server)
pub struct Bridge {
    /// Channel for sending data TO the browser (server responses)
    write_channel: Arc<RTCDataChannel>,
    /// Channel for receiving data FROM the browser (client commands)
    read_channel: Arc<RTCDataChannel>,
    /// UDP socket connected to game server
    udp_socket: Arc<UdpSocket>,
    /// Shutdown signal
    shutdown: Arc<Notify>,
    /// Client identifier for logging
    client_id: String,
}

impl Bridge {
    /// Create a new bridge connecting data channels to a game server
    pub async fn new(
        write_channel: Arc<RTCDataChannel>,
        read_channel: Arc<RTCDataChannel>,
        server_addr: &str,
        client_id: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Bind to random local port
        let udp_socket = UdpSocket::bind("0.0.0.0:0").await?;

        // Connect to game server (allows us to use send/recv instead of send_to/recv_from)
        udp_socket.connect(server_addr).await?;

        let local_addr = udp_socket.local_addr()?;
        info!(
            client_id = %client_id,
            local_port = %local_addr.port(),
            server = %server_addr,
            "UDP socket connected to game server"
        );

        Ok(Self {
            write_channel,
            read_channel,
            udp_socket: Arc::new(udp_socket),
            shutdown: Arc::new(Notify::new()),
            client_id,
        })
    }

    /// Start bidirectional forwarding
    pub async fn start(self: Arc<Self>) {
        // Set once the browser sends its first packet (past valve.zip download,
        // its connect handshake has started). Shared by the two forwarders so
        // the upstream idle timeout also fires for a bridge that never hears back.
        let browser_started = Arc::new(AtomicBool::new(false));

        // Spawn UDP → WebRTC forwarder (server responses to browser via write channel)
        let udp_to_webrtc = tokio::spawn({
            let bridge = self.clone();
            let browser_started = browser_started.clone();
            async move {
                bridge.forward_udp_to_webrtc(browser_started).await;
            }
        });

        // Setup WebRTC → UDP forwarder (browser commands to server via read channel)
        self.setup_webrtc_to_udp(browser_started);

        // Wait for shutdown signal
        self.shutdown.notified().await;

        // Cleanup
        udp_to_webrtc.abort();
        info!(client_id = %self.client_id, "Bridge shut down");
    }

    /// Forward packets from UDP (game server) to WebRTC write channel (browser)
    async fn forward_udp_to_webrtc(&self, browser_started: Arc<AtomicBool>) {
        let mut buf = vec![0u8; MAX_PACKET_SIZE];

        // The HLTV streams continuously once the spectator is connected, so a
        // long silence means the upstream (HLTV / game server) is dead. The
        // timeout arms once the upstream answers (saw_packet) OR the browser
        // starts its connect handshake (browser_started) — no timeout while
        // the browser is still downloading valve.zip and has sent nothing.
        const IDLE_TIMEOUT: Duration = Duration::from_secs(25);
        let mut saw_packet = false;

        loop {
            tokio::select! {
                result = self.udp_socket.recv(&mut buf) => {
                    match result {
                        Ok(n) if n > 0 => {
                            saw_packet = true;
                            let mut data = bytes::Bytes::copy_from_slice(&buf[..n]);

                            // The HLTV appends a 16-zero cookie to the GoldSrc
                            // connection approval ("B0000000000000000"), but the
                            // Xash3D client only accepts the literal "B" reply
                            // (real servers send a space-delimited "B <args>").
                            // Rewrite so the handshake completes on first try.
                            const HLTV_CONNECT_ACK: &[u8] = b"\xff\xff\xff\xffB0000000000000000";
                            if data.len() == HLTV_CONNECT_ACK.len()
                                && data.starts_with(HLTV_CONNECT_ACK)
                            {
                                data = bytes::Bytes::from_static(b"\xff\xff\xff\xffB\n");
                                info!(
                                    client_id = %self.client_id,
                                    "Rewrote HLTV connect ack 'B'+cookie -> 'B'"
                                );
                            }

                            debug!(
                                client_id = %self.client_id,
                                bytes = n,
                                "UDP → WebRTC (write channel)"
                            );

                            if let Err(e) = self.write_channel.send(&data).await {
                                error!(
                                    client_id = %self.client_id,
                                    error = %e,
                                    "Failed to send to write channel"
                                );
                                break;
                            }
                        }
                        Ok(_) => {
                            // Empty packet, continue
                        }
                        Err(e) => {
                            error!(
                                client_id = %self.client_id,
                                error = %e,
                                "UDP recv error, tearing down bridge (upstream lost)"
                            );
                            self.teardown().await;
                            break;
                        }
                    }
                }
                _ = tokio::time::sleep(IDLE_TIMEOUT), if saw_packet
                    || browser_started.load(Ordering::Relaxed) => {
                    warn!(
                        client_id = %self.client_id,
                        idle_seconds = IDLE_TIMEOUT.as_secs(),
                        "HLTV upstream stalled: no UDP data, tearing down bridge"
                    );
                    self.teardown().await;
                    break;
                }
                () = self.shutdown.notified() => {
                    break;
                }
            }
        }
    }

    /// Close the data channels and signal shutdown so the browser notices the
    /// connection is dead. SCTP keepalives keep the WebRTC connection "alive"
    /// (and the peer state Connected) even when the upstream is gone, so the
    /// client would otherwise freeze forever without any signal.
    async fn teardown(&self) {
        if let Err(e) = self.write_channel.close().await {
            error!(client_id = %self.client_id, error = %e, "Failed to close write channel");
        }
        if let Err(e) = self.read_channel.close().await {
            error!(client_id = %self.client_id, error = %e, "Failed to close read channel");
        }
        self.shutdown.notify_one();
    }

    /// Setup callback for WebRTC read channel → UDP forwarding (browser to game server)
    fn setup_webrtc_to_udp(&self, browser_started: Arc<AtomicBool>) {
        let udp_socket = self.udp_socket.clone();
        let client_id = self.client_id.clone();
        let shutdown = self.shutdown.clone();
        let browser_started = browser_started.clone();

        // Handle incoming messages on the read channel
        self.read_channel
            .on_message(Box::new(move |msg: DataChannelMessage| {
                let udp_socket = udp_socket.clone();
                let client_id = client_id.clone();
                let browser_started = browser_started.clone();

                Box::pin(async move {
                    let data = msg.data;
                    browser_started.store(true, Ordering::Relaxed);
                    debug!(
                        client_id = %client_id,
                        bytes = data.len(),
                        "WebRTC (read channel) → UDP"
                    );

                    if let Err(e) = udp_socket.send(&data).await {
                        error!(
                            client_id = %client_id,
                            error = %e,
                            "Failed to send to UDP"
                        );
                    }
                })
            }));

        // Handle read channel close
        let shutdown_clone = shutdown.clone();
        let client_id = self.client_id.clone();
        self.read_channel.on_close(Box::new(move || {
            info!(client_id = %client_id, "Read channel closed");
            shutdown_clone.notify_one();
            Box::pin(async {})
        }));

        // Handle read channel errors
        let shutdown_clone = shutdown;
        let client_id = self.client_id.clone();
        self.read_channel.on_error(Box::new(move |e| {
            error!(client_id = %client_id, error = %e, "Read channel error");
            shutdown_clone.notify_one();
            Box::pin(async {})
        }));
    }

    /// Shutdown the bridge
    pub fn shutdown(&self) {
        self.shutdown.notify_one();
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        info!(client_id = %self.client_id, "Bridge dropped");
    }
}
