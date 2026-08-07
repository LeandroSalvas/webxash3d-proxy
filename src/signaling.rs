//! WebRTC signaling over WebSocket for game client connections.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Weak};

use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc_ice::udp_network::{EphemeralUDP, UDPNetwork};
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

use crate::bridge::Bridge;
use crate::config::Config;

/// Signal event type constants
mod events {
    pub const OFFER: &str = "offer";
    pub const ANSWER: &str = "answer";
    pub const CANDIDATE: &str = "candidate";
}

/// WebSocket signaling message
#[derive(Debug, Serialize, Deserialize)]
struct SignalMessage {
    event: String,
    data: serde_json::Value,
}

/// Type alias for the WebSocket sender wrapped in `Arc<Mutex>`
type WsSender = Arc<Mutex<futures::stream::SplitSink<WebSocket, Message>>>;

/// Type alias for the bridge holder
type BridgeHolder = Arc<Mutex<Option<Arc<Bridge>>>>;

/// Handle a new WebSocket connection for WebRTC signaling
pub async fn handle_websocket(socket: WebSocket, config: Arc<Config>, client_id: String) {
    info!(client_id = %client_id, "New WebSocket connection");

    let (ws_sender, ws_receiver) = socket.split();
    let ws_sender: WsSender = Arc::new(Mutex::new(ws_sender));

    // Create WebRTC peer connection
    let peer = match create_peer_connection(config.public_ip.clone(), &config).await {
        Ok(p) => Arc::new(p),
        Err(e) => {
            error!(client_id = %client_id, error = %e, "Failed to create peer connection");
            return;
        }
    };

    // Create data channels
    let Some((write_channel, read_channel)) = create_data_channels(&peer, &client_id).await else {
        close_peer(&peer, &client_id).await;
        return;
    };

    info!(client_id = %client_id, "Created write and read data channels");

    // Setup bridge management
    let bridge: BridgeHolder = Arc::new(Mutex::new(None));

    // Setup callbacks
    setup_bridge_callbacks(
        &write_channel,
        &read_channel,
        config.clone(),
        client_id.clone(),
        bridge.clone(),
    );

    setup_ice_handler(&peer, ws_sender.clone(), client_id.clone());
    setup_connection_monitor(&peer, bridge.clone(), client_id.clone());

    // Send offer to client
    if !send_offer(&peer, &ws_sender, &client_id).await {
        close_peer(&peer, &client_id).await;
        return;
    }

    // Handle incoming WebSocket messages
    handle_ws_messages(ws_receiver, &peer, client_id.clone()).await;

    // Cleanup
    if let Some(b) = bridge.lock().await.take() {
        b.shutdown();
    }

    close_peer(&peer, &client_id).await;

    info!(client_id = %client_id, "WebSocket connection closed");
}

/// Create write and read data channels for game communication
async fn create_data_channels(
    peer: &Arc<RTCPeerConnection>,
    client_id: &str,
) -> Option<(Arc<RTCDataChannel>, Arc<RTCDataChannel>)> {
    let dc_options = RTCDataChannelInit {
        ordered: Some(true),
        ..Default::default()
    };

    // Create "write" channel - for sending data TO the browser
    let write_channel = match peer
        .create_data_channel("write", Some(dc_options.clone()))
        .await
    {
        Ok(dc) => dc,
        Err(e) => {
            error!(client_id = %client_id, error = %e, "Failed to create write channel");
            return None;
        }
    };

    // Create "read" channel - for receiving data FROM the browser
    let read_channel = match peer.create_data_channel("read", Some(dc_options)).await {
        Ok(dc) => dc,
        Err(e) => {
            error!(client_id = %client_id, error = %e, "Failed to create read channel");
            return None;
        }
    };

    Some((write_channel, read_channel))
}

/// Setup callbacks to start the bridge when both channels are open
fn setup_bridge_callbacks(
    write_channel: &Arc<RTCDataChannel>,
    read_channel: &Arc<RTCDataChannel>,
    config: Arc<Config>,
    client_id: String,
    bridge: BridgeHolder,
) {
    let channels_open = Arc::new(AtomicU8::new(0));

    // Use Weak references so the on_open callbacks (stored inside the channels)
    // do not keep the channels alive after the peer connection is dropped.
    let write_weak = Arc::downgrade(write_channel);
    let read_weak = Arc::downgrade(read_channel);

    // Setup write channel on_open callback
    setup_channel_on_open(
        write_channel,
        channels_open.clone(),
        config.clone(),
        client_id.clone(),
        bridge.clone(),
        write_weak.clone(),
        read_weak.clone(),
    );

    // Setup read channel on_open callback
    setup_channel_on_open(
        read_channel,
        channels_open,
        config,
        client_id,
        bridge,
        write_weak,
        read_weak,
    );
}

/// Setup the `on_open` callback for a data channel
fn setup_channel_on_open(
    channel: &Arc<RTCDataChannel>,
    channels_open: Arc<AtomicU8>,
    config: Arc<Config>,
    client_id: String,
    bridge: BridgeHolder,
    write_channel: Weak<RTCDataChannel>,
    read_channel: Weak<RTCDataChannel>,
) {
    channel.on_open(Box::new(move || {
        let channels_open = channels_open.clone();
        let config = config.clone();
        let client_id = client_id.clone();
        let bridge = bridge.clone();
        let write_channel = write_channel.clone();
        let read_channel = read_channel.clone();

        Box::pin(async move {
            let count = channels_open.fetch_add(1, Ordering::SeqCst) + 1;
            if count == 2 {
                let Some(write_channel) = write_channel.upgrade() else {
                    return;
                };
                let Some(read_channel) = read_channel.upgrade() else {
                    return;
                };
                start_bridge(config, client_id, bridge, write_channel, read_channel).await;
            }
        })
    }));
}

/// Start the UDP bridge when both channels are ready
async fn start_bridge(
    config: Arc<Config>,
    client_id: String,
    bridge: BridgeHolder,
    write_channel: Arc<RTCDataChannel>,
    read_channel: Arc<RTCDataChannel>,
) {
    info!(client_id = %client_id, "Both channels open, starting bridge");

    match Bridge::new(
        write_channel,
        read_channel,
        &config.server,
        client_id.clone(),
    )
    .await
    {
        Ok(b) => {
            let b = Arc::new(b);
            *bridge.lock().await = Some(b.clone());
            tokio::spawn(async move {
                b.start().await;
            });
        }
        Err(e) => {
            error!(client_id = %client_id, error = %e, "Failed to create bridge");
        }
    }
}

/// Setup ICE candidate handler to send candidates to the client
fn setup_ice_handler(peer: &Arc<RTCPeerConnection>, ws_sender: WsSender, client_id: String) {
    peer.on_ice_candidate(Box::new(move |candidate| {
        let ws_sender = ws_sender.clone();
        let client_id = client_id.clone();

        Box::pin(async move {
            let Some(c) = candidate else {
                return;
            };

            match c.to_json() {
                Ok(json) => {
                    // webrtc-rs hardcodes an empty sdpMid; the offer uses
                    // a=mid:0 (single data-channel m-line), so browsers
                    // reject candidates with sdpMid:"".
                    let mut json = json;
                    json.sdp_mid = Some("0".to_string());
                    json.sdp_mline_index = Some(0u16);

                    let msg = SignalMessage {
                        event: events::CANDIDATE.to_string(),
                        data: serde_json::to_value(json).unwrap_or_default(),
                    };

                    debug!(client_id = %client_id, "Sending ICE candidate");

                    let json_str = serde_json::to_string(&msg).unwrap_or_default();
                    let mut sender = ws_sender.lock().await;
                    if let Err(e) = sender.send(Message::Text(json_str)).await {
                        error!(client_id = %client_id, error = %e, "Failed to send ICE candidate");
                    }
                }
                Err(e) => {
                    error!(client_id = %client_id, error = %e, "Failed to serialize ICE candidate");
                }
            }
        })
    }));
}

/// Setup connection state change handler
fn setup_connection_monitor(
    peer: &Arc<RTCPeerConnection>,
    bridge: BridgeHolder,
    client_id: String,
) {
    let monitor_peer = Arc::downgrade(peer);
    peer.on_peer_connection_state_change(Box::new(move |state| {
        let client_id = client_id.clone();
        let bridge = bridge.clone();
        let peer = monitor_peer.clone();

        Box::pin(async move {
            info!(client_id = %client_id, state = ?state, "Peer connection state changed");

            match state {
                RTCPeerConnectionState::Failed
                | RTCPeerConnectionState::Disconnected
                | RTCPeerConnectionState::Closed => {
                    if let Some(b) = bridge.lock().await.take() {
                        b.shutdown();
                    }
                    // Disconnected is transient in WebRTC; only close on
                    // terminal states so recoverable connections survive.
                    if matches!(
                        state,
                        RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed
                    ) {
                        if let Some(peer) = peer.upgrade() {
                            close_peer(&peer, &client_id).await;
                        }
                    }
                }
                _ => {}
            }
        })
    }));
}

/// Close a peer connection and log any errors
async fn close_peer(peer: &Arc<RTCPeerConnection>, client_id: &str) {
    if let Err(e) = peer.close().await {
        error!(client_id = %client_id, error = %e, "Failed to close peer connection");
    }
}

/// Create and send WebRTC offer to client
async fn send_offer(peer: &Arc<RTCPeerConnection>, ws_sender: &WsSender, client_id: &str) -> bool {
    let offer = match peer.create_offer(None).await {
        Ok(o) => o,
        Err(e) => {
            error!(client_id = %client_id, error = %e, "Failed to create offer");
            return false;
        }
    };

    if let Err(e) = peer.set_local_description(offer.clone()).await {
        error!(client_id = %client_id, error = %e, "Failed to set local description");
        return false;
    }

    let offer_msg = SignalMessage {
        event: events::OFFER.to_string(),
        data: serde_json::json!({
            "type": events::OFFER,
            "sdp": offer.sdp
        }),
    };

    let json_str = serde_json::to_string(&offer_msg).unwrap_or_default();
    let mut sender = ws_sender.lock().await;
    if let Err(e) = sender.send(Message::Text(json_str)).await {
        error!(client_id = %client_id, error = %e, "Failed to send offer");
        return false;
    }

    info!(client_id = %client_id, "Sent WebRTC offer");
    true
}

/// Handle incoming WebSocket messages (answer, candidates)
async fn handle_ws_messages(
    mut receiver: futures::stream::SplitStream<WebSocket>,
    peer: &Arc<RTCPeerConnection>,
    client_id: String,
) {
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                let signal: SignalMessage = match serde_json::from_str(&text) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(client_id = %client_id, error = %e, "Invalid signal message");
                        continue;
                    }
                };

                match signal.event.as_str() {
                    events::ANSWER => {
                        handle_answer(peer, &signal, &client_id).await;
                    }
                    events::CANDIDATE => {
                        handle_candidate(peer, signal.data, &client_id).await;
                    }
                    _ => {
                        warn!(client_id = %client_id, event = %signal.event, "Unknown signal event");
                    }
                }
            }
            Ok(Message::Close(_)) => {
                info!(client_id = %client_id, "WebSocket close received");
                break;
            }
            Ok(Message::Ping(_)) => {
                debug!(client_id = %client_id, "Ping received");
                // Pong is handled automatically by axum
            }
            Ok(_) => {}
            Err(e) => {
                error!(client_id = %client_id, error = %e, "WebSocket error");
                break;
            }
        }
    }
}

/// Handle SDP answer from client
async fn handle_answer(peer: &Arc<RTCPeerConnection>, signal: &SignalMessage, client_id: &str) {
    debug!(client_id = %client_id, "Received answer");

    let sdp = signal
        .data
        .get("sdp")
        .and_then(|s| s.as_str())
        .unwrap_or("");

    let answer = match RTCSessionDescription::answer(sdp.to_string()) {
        Ok(a) => a,
        Err(e) => {
            error!(client_id = %client_id, error = %e, "Failed to parse SDP answer");
            return;
        }
    };

    if let Err(e) = peer.set_remote_description(answer).await {
        error!(client_id = %client_id, error = %e, "Failed to set remote description");
    }
}

/// Handle ICE candidate from client
async fn handle_candidate(peer: &Arc<RTCPeerConnection>, data: serde_json::Value, client_id: &str) {
    debug!(client_id = %client_id, data = %data, "Received ICE candidate");

    let candidate: RTCIceCandidateInit = match serde_json::from_value(data) {
        Ok(c) => c,
        Err(e) => {
            warn!(client_id = %client_id, error = %e, "Invalid ICE candidate");
            return;
        }
    };

    if let Err(e) = peer.add_ice_candidate(candidate).await {
        error!(client_id = %client_id, error = %e, "Failed to add ICE candidate");
    }
}

/// Create a new WebRTC peer connection
async fn create_peer_connection(
    public_ip: Option<String>,
    config: &Config,
) -> Result<RTCPeerConnection, Box<dyn std::error::Error + Send + Sync>> {
    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs()?;

    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media_engine)?;

    let mut setting_engine = SettingEngine::default();

    // Fixed UDP port range for WebRTC ICE (helps firewalls/NAT)
    if let Some((start, end)) = config.udp_port_range_parsed() {
        let mut ephemeral_udp = EphemeralUDP::default();
        if let Err(e) = ephemeral_udp.set_ports(start, end) {
            error!(error = %e, "Invalid UDP port range");
            return Err(Box::new(e));
        }
        setting_engine.set_udp_network(UDPNetwork::Ephemeral(ephemeral_udp));
    }

    // Set public IP for NAT traversal if provided.
    // Srflx type keeps the LAN host candidates AND adds public-IP
    // candidates, so same-LAN viewers still work without hairpin NAT.
    if let Some(ref ip) = public_ip {
        setting_engine.set_nat_1to1_ips(
            vec![ip.clone()],
            webrtc::ice_transport::ice_candidate_type::RTCIceCandidateType::Srflx,
        );
    }

    let api = APIBuilder::new()
        .with_media_engine(media_engine)
        .with_interceptor_registry(registry)
        .with_setting_engine(setting_engine)
        .build();

    // When a public IP is provided via nat_1to1 (Srflx), a STUN server must
    // NOT be configured (webrtc-ice errors out otherwise). STUN is only used
    // when there is no known public IP.
    let ice_servers = if public_ip.is_some() {
        vec![]
    } else {
        vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            ..Default::default()
        }]
    };

    let config = RTCConfiguration {
        ice_servers,
        ..Default::default()
    };

    let peer = api.new_peer_connection(config).await?;

    Ok(peer)
}
