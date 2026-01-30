use crate::webrtc::webrtc_interface::WebRTCInterface;
use axum::{extract::State, http::StatusCode, response::Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info, trace, warn};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalingRequest {
    #[serde(rename = "type")]
    pub sdp_type: String,
    pub sdp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalingResponse {
    #[serde(rename = "type")]
    pub sdp_type: String,
    pub sdp: String,
}

pub async fn handle_signaling(
    State(webrtc_interface): State<Arc<WebRTCInterface>>,
    Json(request): Json<SignalingRequest>,
) -> Result<Json<SignalingResponse>, StatusCode> {
    let span = tracing::span!(tracing::Level::INFO, "http", context = "signaling");
    let _enter = span.enter();
    info!(
        "Received WebRTC signaling request - type: {}, SDP length: {} bytes",
        request.sdp_type,
        request.sdp.len()
    );
    trace!("Signaling request SDP: {}", request.sdp);

    if request.sdp_type != "offer" {
        warn!(
            "Invalid signaling request - expected 'offer', got: {}",
            request.sdp_type
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    debug!("Initializing WebRTC components...");

    // Create WebRTC peer connection
    let mut m = MediaEngine::default();
    m.register_default_codecs().map_err(|e| {
        error!("Failed to register codecs: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m).map_err(|e| {
        error!("Failed to register interceptors: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();

    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
            ..Default::default()
        }],
        ..Default::default()
    };

    debug!("Creating peer connection...");
    let peer_connection = Arc::new(api.new_peer_connection(config).await.map_err(|e| {
        error!("Failed to create peer connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?);

    // Set up data channel handler BEFORE setting remote description
    let webrtc_clone = webrtc_interface.clone();
    peer_connection.on_data_channel(Box::new(
        move |data_channel: Arc<webrtc::data_channel::RTCDataChannel>| {
            let span = tracing::span!(tracing::Level::INFO, "transport", transport = "WebRTC");
            let _enter = span.enter();
            let label = data_channel.label().to_string();
            let dc_id = data_channel.id();

            info!(
                "Data channel received from client: '{}' (ID: {})",
                label, dc_id
            );

            // Register the data channel with the WebRTC interface
            let webrtc_for_register = webrtc_clone.clone();
            use webrtc::data_channel::data_channel_state::RTCDataChannelState;
            let channel_state = data_channel.ready_state();
            info!(
                "Data channel '{}' (ID: {}) state before registration: {:?}",
                label, dc_id, channel_state
            );
            webrtc_for_register.register_data_channel(dc_id, data_channel.clone());

            let dc_label_open = label.clone();
            data_channel.on_open(Box::new(move || {
                let span = tracing::span!(tracing::Level::INFO, "transport", transport = "WebRTC");
                let _enter = span.enter();
                info!("Data channel opened: '{}' (ID: {})", dc_label_open, dc_id);
                Box::pin(async {})
            }));

            let dc_label_msg = label.clone();
            let webrtc_for_msg = webrtc_clone.clone();
            let data_channel_for_msg = data_channel.clone();
            data_channel.on_message(Box::new(move |msg: DataChannelMessage| {
                let span = tracing::span!(tracing::Level::TRACE, "transport", transport = "WebRTC");
                let _enter = span.enter();
                let webrtc_clone = webrtc_for_msg.clone();
                let label = dc_label_msg.clone();
                let dc = data_channel_for_msg.clone();

                trace!(
                    "Message received on data channel '{}': {} bytes",
                    label,
                    msg.data.len()
                );

                // Forward the message to the WebRTC interface with the data channel Arc
                webrtc_clone.handle_data_channel_message(dc, msg.data.to_vec());

                Box::pin(async {})
            }));

            let dc_label_close = label.clone();
            let webrtc_for_close = webrtc_clone.clone();
            data_channel.on_close(Box::new(move || {
                let span = tracing::span!(tracing::Level::INFO, "transport", transport = "WebRTC");
                let _enter = span.enter();
                warn!("Data channel closed: '{}' (ID: {})", dc_label_close, dc_id);
                webrtc_for_close.unregister_data_channel(dc_id);
                Box::pin(async {})
            }));

            Box::pin(async {})
        },
    ));

    // Set remote description (the offer)
    debug!("Setting remote description (offer)...");
    let offer = RTCSessionDescription::offer(request.sdp).map_err(|e| {
        error!("Failed to parse offer: {}", e);
        StatusCode::BAD_REQUEST
    })?;

    peer_connection
        .set_remote_description(offer)
        .await
        .map_err(|e| {
            error!("Failed to set remote description: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Create answer
    debug!("Creating answer...");
    let answer = peer_connection.create_answer(None).await.map_err(|e| {
        error!("Failed to create answer: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let answer_sdp = answer.sdp.clone();

    // Set local description
    debug!("Setting local description (answer)...");
    peer_connection
        .set_local_description(answer)
        .await
        .map_err(|e| {
            error!("Failed to set local description: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Wait for ICE gathering to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Get the final SDP with ICE candidates
    let local_desc = peer_connection.local_description().await;
    let final_sdp = match local_desc {
        Some(desc) => {
            debug!(
                "Retrieved final SDP with ICE candidates (length: {} bytes)",
                desc.sdp.len()
            );
            desc.sdp
        }
        None => {
            warn!("No local description available, using initial answer");
            answer_sdp
        }
    };

    // Monitor connection state
    let _pc_clone = peer_connection.clone();
    let webrtc_monitor = webrtc_interface.clone();
    peer_connection.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
        let span = tracing::span!(tracing::Level::INFO, "transport", transport = "WebRTC");
        let _enter = span.enter();
        let _webrtc = webrtc_monitor.clone();
        Box::pin(async move {
            match s {
                RTCPeerConnectionState::Unspecified => {
                    debug!("Peer connection state: Unspecified");
                }
                RTCPeerConnectionState::New => {
                    debug!("Peer connection state: New");
                }
                RTCPeerConnectionState::Connecting => {
                    info!("Peer connection state: Connecting");
                }
                RTCPeerConnectionState::Connected => {
                    info!("Peer connection established!");
                }
                RTCPeerConnectionState::Disconnected => {
                    warn!("Peer connection disconnected");
                }
                RTCPeerConnectionState::Failed => {
                    error!("Peer connection failed");
                }
                RTCPeerConnectionState::Closed => {
                    info!("Peer connection closed");
                }
            }
        })
    }));

    // Keep the peer connection alive
    let pc_monitor = peer_connection.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            let state = pc_monitor.connection_state();
            if state == RTCPeerConnectionState::Closed || state == RTCPeerConnectionState::Failed {
                break;
            }
        }
    });

    info!(
        "WebRTC signaling complete - returning answer (SDP length: {} bytes)",
        final_sdp.len()
    );
    trace!("Signaling response SDP: {}", final_sdp);
    Ok(Json(SignalingResponse {
        sdp_type: "answer".to_string(),
        sdp: final_sdp,
    }))
}
