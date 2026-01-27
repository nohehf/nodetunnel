use axum::{
    http::{Method, Request, StatusCode},
    response::Json,
    routing::{get, post},
    Router,
};
use tower_http::cors::{CorsLayer, Any};
use tower_http::trace::TraceLayer;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info, warn, error};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SignalingRequest {
    #[serde(rename = "type")]
    sdp_type: String,
    sdp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SignalingResponse {
    #[serde(rename = "type")]
    sdp_type: String,
    sdp: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    info!("Starting NodeTunnel test server...");
    
    // Enable CORS for all origins
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers(Any);
    
    // Add request tracing/logging
    let trace_layer = TraceLayer::new_for_http()
        .on_request(|request: &Request<_>, _span: &tracing::Span| {
            info!("🔵 Incoming request: {} {}", request.method(), request.uri());
            if let Some(host) = request.headers().get("host") {
                debug!("   Host: {:?}", host);
            }
            if let Some(user_agent) = request.headers().get("user-agent") {
                debug!("   User-Agent: {:?}", user_agent);
            }
        })
        .on_response(|_response: &axum::response::Response, latency: std::time::Duration, _span: &tracing::Span| {
            info!("🟢 Response sent (latency: {:?})", latency);
        });
    
    let app = Router::new()
        .route("/", get(|| async { "NodeTunnel Test Server - POST /signaling" }))
        .route("/signaling", post(handle_signaling))
        .layer(trace_layer)
        .layer(cors);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:5344").await?;
    info!("✓ Test server listening on http://0.0.0.0:5344");
    info!("✓ Endpoint: POST http://127.0.0.1:5344/signaling");
    info!("Ready to accept WebRTC connections");
    info!("🔌 Waiting for connections...");
    axum::serve(listener, app).await?;

    Ok(())
}

async fn handle_signaling(
    Json(request): Json<SignalingRequest>,
) -> Result<Json<SignalingResponse>, StatusCode> {
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("📨 Received signaling request");
    info!("   Type: {}", request.sdp_type);
    info!("   SDP length: {} bytes", request.sdp.len());
    debug!("   SDP preview: {}", request.sdp.lines().take(3).collect::<Vec<_>>().join(" "));
    
    // Log first few lines of SDP for debugging
    let sdp_preview: Vec<&str> = request.sdp.lines().take(5).collect();
    debug!("   SDP first lines: {:?}", sdp_preview);

    if request.sdp_type != "offer" {
        warn!("❌ Expected offer, got: {}", request.sdp_type);
        return Err(StatusCode::BAD_REQUEST);
    }

    info!("🔧 Initializing WebRTC components...");
    
    // Create WebRTC peer connection
    let mut m = MediaEngine::default();
    m.register_default_codecs().map_err(|e| {
        error!("❌ Failed to register codecs: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    debug!("✓ Codecs registered");
    
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m).map_err(|e| {
        error!("❌ Failed to register interceptors: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    debug!("✓ Interceptors registered");

    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();
    debug!("✓ WebRTC API created");

    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
            ..Default::default()
        }],
        ..Default::default()
    };
    debug!("✓ ICE configuration set (STUN: stun.l.google.com:19302)");

    info!("🔗 Creating peer connection...");
    let peer_connection = Arc::new(
        api.new_peer_connection(config)
            .await
            .map_err(|e| {
                error!("❌ Failed to create peer connection: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
    );
    info!("✓ Peer connection created");

    // Create data channel
    info!("📡 Creating data channel 'game'...");
    let data_channel = Arc::new(
        peer_connection
            .create_data_channel("game", None)
            .await
            .map_err(|e| {
                error!("❌ Failed to create data channel: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    );
    info!("✓ Data channel created (ID: {}, Label: '{}')", data_channel.id(), data_channel.label());

    // Set up data channel handlers
    let dc_label = data_channel.label().to_string();
    let dc_id = data_channel.id();
    
    let dc_label_open = dc_label.clone();
    data_channel.on_open(Box::new(move || {
        info!("✅ Data channel opened: '{}' (ID: {})", dc_label_open, dc_id);
        Box::pin(async {})
    }));

    let dc_label_msg = dc_label.clone();
    data_channel.on_message(Box::new(move |msg: DataChannelMessage| {
        info!("📩 Received message on data channel '{}': {} bytes", dc_label_msg, msg.data.len());
        if msg.data.len() <= 100 {
            info!("   Data (hex): {:02x?}", msg.data);
            // Check if it's a keepalive packet
            if msg.data.len() == 2 && msg.data[0] == 0xFF && msg.data[1] == 0xFF {
                info!("   ✅ Keepalive packet received!");
            }
        } else {
            info!("   Data (hex, first 100 bytes): {:02x?}...", &msg.data[..100.min(msg.data.len())]);
        }
        Box::pin(async {})
    }));

    let dc_label_close = dc_label.clone();
    data_channel.on_close(Box::new(move || {
        info!("🔌 Data channel closed: '{}' (ID: {})", dc_label_close, dc_id);
        Box::pin(async {})
    }));

    // Set remote description (the offer)
    info!("📥 Setting remote description (offer)...");
    let offer = RTCSessionDescription::offer(request.sdp).map_err(|e| {
        error!("❌ Failed to parse offer: {}", e);
        StatusCode::BAD_REQUEST
    })?;
    debug!("   Offer type: {:?}", offer.sdp_type);
    
    peer_connection
        .set_remote_description(offer)
        .await
        .map_err(|e| {
            error!("❌ Failed to set remote description: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    info!("✓ Remote description set");

    // Create answer
    info!("📤 Creating answer...");
    let answer = peer_connection
        .create_answer(None)
        .await
        .map_err(|e| {
            error!("❌ Failed to create answer: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    let answer_sdp = answer.sdp.clone();
    debug!("   Answer SDP length: {} bytes", answer_sdp.len());

    // Set local description
    info!("📥 Setting local description (answer)...");
    peer_connection
        .set_local_description(answer)
        .await
        .map_err(|e| {
            error!("❌ Failed to set local description: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    info!("✓ Local description set");

    // Wait for ICE gathering to complete
    info!("🧊 Waiting for ICE gathering...");
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // Get the final SDP with ICE candidates
    let local_desc = peer_connection.local_description().await;
    let final_sdp = match local_desc {
        Some(desc) => {
            debug!("✓ Got final SDP with ICE candidates (length: {} bytes)", desc.sdp.len());
            desc.sdp
        }
        None => {
            warn!("⚠️  No local description available, using initial answer");
            answer_sdp
        }
    };

    // Monitor connection state
    info!("👀 Setting up connection state monitoring...");
    let _pc_clone = peer_connection.clone();
    peer_connection.on_peer_connection_state_change(Box::new(
        move |s: RTCPeerConnectionState| {
            Box::pin(async move {
                match s {
                    RTCPeerConnectionState::New => {
                        info!("🆕 Peer connection state: New");
                    }
                    RTCPeerConnectionState::Connecting => {
                        info!("🔄 Peer connection state: Connecting...");
                    }
                    RTCPeerConnectionState::Connected => {
                        info!("✅ WebRTC connection established!");
                    }
                    RTCPeerConnectionState::Disconnected => {
                        warn!("⚠️  Peer connection state: Disconnected");
                    }
                    RTCPeerConnectionState::Failed => {
                        error!("❌ Peer connection state: Failed");
                    }
                    RTCPeerConnectionState::Closed => {
                        info!("🔒 Peer connection state: Closed");
                    }
                    RTCPeerConnectionState::Unspecified => {
                        debug!("❓ Peer connection state: Unspecified");
                    }
                }
            })
        },
    ));

    // Monitor ICE connection state
    peer_connection.on_ice_connection_state_change(Box::new(
        move |s| {
            info!("🧊 ICE connection state: {:?}", s);
            Box::pin(async {})
        },
    ));

    // Monitor ICE gathering state
    peer_connection.on_ice_gathering_state_change(Box::new(
        move |s| {
            debug!("🧊 ICE gathering state: {:?}", s);
            Box::pin(async {})
        },
    ));

    // Keep the peer connection alive (don't drop it)
    let pc_monitor = peer_connection.clone();
    tokio::spawn(async move {
        info!("🔄 Starting connection monitor loop...");
        let mut last_state = pc_monitor.connection_state();
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            let state = pc_monitor.connection_state();
            if state != last_state {
                debug!("   Connection state changed: {:?} -> {:?}", last_state, state);
                last_state = state;
            }
            if state == RTCPeerConnectionState::Closed
                || state == RTCPeerConnectionState::Failed
            {
                info!("🛑 Connection monitor stopping (state: {:?})", state);
                break;
            }
        }
    });

    info!("✅ WebRTC connection setup complete");
    info!("📤 Returning answer SDP ({} bytes)", final_sdp.len());
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    Ok(Json(SignalingResponse {
        sdp_type: "answer".to_string(),
        sdp: final_sdp,
    }))
}
