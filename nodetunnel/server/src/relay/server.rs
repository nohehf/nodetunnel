use crate::config::loader::Config;
use crate::relay::apps::Apps;
use crate::relay::clients::{ClientState, Clients};
use crate::relay::handlers::auth::AuthHandler;
use crate::relay::handlers::disconnect::DisconnectHandler;
use crate::relay::handlers::game_data::GameDataHandler;
use crate::relay::handlers::room::RoomHandler;
use crate::transport::server::TransportRegistry;
use crate::udp::common::{ServerEvent, TransferChannel};
use crate::udp::paper_interface::PaperInterface;
use crate::webrtc::signaling::handle_signaling;
use crate::webrtc::webrtc_interface::WebRTCInterface;
use axum::{routing::post, Router};
use nodetunnel_core::protocol::packet::PacketType;
use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tower::ServiceBuilder;
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, error, info, trace, warn};

pub struct RelayServer {
    transport: TransportRegistry,
    webrtc: Arc<WebRTCInterface>,
    webrtc_event_rx: mpsc::UnboundedReceiver<ServerEvent>,
    http_client: reqwest::Client,

    config: Config,
    apps: Apps,
    clients: Clients,
}

impl RelayServer {
    pub fn new(udp_transport: PaperInterface, config: Config) -> (Self, Arc<WebRTCInterface>) {
        let (webrtc_event_tx, webrtc_event_rx) = mpsc::unbounded_channel();
        let webrtc = Arc::new(WebRTCInterface::new(webrtc_event_tx));

        let transport = TransportRegistry::new(udp_transport);

        let server = Self {
            transport,
            webrtc: webrtc.clone(),
            webrtc_event_rx,
            http_client: reqwest::Client::new(),
            config,
            apps: Apps::new(),
            clients: Clients::new(),
        };

        (server, webrtc)
    }

    /// Starts the server loop.
    pub async fn run(&mut self) -> Result<(), Box<dyn Error>> {
        // Start HTTP server for WebRTC signaling
        let http_addr: SocketAddr = self
            .config
            .http_bind_address
            .to_socket_addrs()?
            .next()
            .ok_or("Failed to resolve HTTP host name")?;

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        let app = Router::new()
            .route("/signaling", post(handle_signaling))
            .layer(ServiceBuilder::new().layer(cors))
            .with_state(self.webrtc.clone());

        let listener = tokio::net::TcpListener::bind(&http_addr).await?;

        let span = tracing::span!(tracing::Level::INFO, "http", context = "server");
        let _enter = span.enter();
        info!("Server listening on {}", http_addr);

        // Spawn HTTP server as a background task
        tokio::spawn(async move {
            let server = axum::serve(listener, app);
            if let Err(e) = server.await {
                let span = tracing::span!(tracing::Level::ERROR, "http", context = "server");
                let _enter = span.enter();
                error!("Server error: {}", e);
            }
        });

        // TODO: remove magic numbers
        let mut cleanup = tokio::time::interval(Duration::from_secs(1));
        // TODO: remove magic numbers
        let mut resend = tokio::time::interval(Duration::from_millis(50));

        cleanup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        resend.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                result = self.transport.udp_mut().recv_events() => {
                    let events = result?;
                    for event in events {
                        self.handle_event(event, true).await; // true = UDP
                    }
                }

                event = self.webrtc_event_rx.recv() => {
                    if let Some(event) = event {
                        self.handle_event(event, false).await; // false = WebRTC
                    }
                }

                _ = cleanup.tick() => {
                    // TODO: remove magic numbers
                    for client_id in self.transport.udp_mut().connection_manager.cleanup_sessions(Duration::from_secs(5)) {
                        self.handle_event(ServerEvent::ClientDisconnected { client_id }, true).await;
                    }
                }

                _ = resend.tick() => {
                    // TODO: remove magic numbers
                    self.transport.udp_mut().do_resends(Duration::from_millis(100)).await;
                }
            }
        }

        Ok(())
    }

    /// Handles an event from either UDP or WebRTC layer.
    async fn handle_event(&mut self, event: ServerEvent, is_udp: bool) {
        match event {
            ServerEvent::ClientConnected { client_id } => {
                let transport_type = if is_udp { "UDP" } else { "WebRTC" };
                let span = tracing::span!(
                    tracing::Level::INFO,
                    "transport",
                    transport = transport_type
                );
                let _enter = span.enter();
                info!("Client connected: {}", client_id);
                self.clients.create(client_id);
                // Register transport based on connection type
                if is_udp {
                    self.transport.register_udp_client(client_id);
                } else {
                    self.transport
                        .register_webrtc_client(client_id, self.webrtc.clone());
                }
            }
            ServerEvent::ClientDisconnected { client_id } => {
                let transport_type = if is_udp { "UDP" } else { "WebRTC" };
                let span = tracing::span!(
                    tracing::Level::INFO,
                    "transport",
                    transport = transport_type
                );
                let _enter = span.enter();
                info!("Client disconnected: {}", client_id);
                DisconnectHandler::new(&mut self.transport, &mut self.clients, &mut self.apps)
                    .handle_disconnect(client_id)
                    .await;
            }
            ServerEvent::PacketReceived {
                client_id,
                data,
                channel,
            } => {
                trace!(
                    "received packet from client {}: {} bytes via {:?}",
                    client_id,
                    data.len(),
                    channel
                );
                self.handle_packet(client_id, data, channel).await;
            }
        }
    }

    /// Handles a packet received from `PaperUDP`.
    /// This checks the state of the client and routes packets based on the state.
    async fn handle_packet(
        &mut self,
        from_client_id: u64,
        data: Vec<u8>,
        channel: TransferChannel,
    ) {
        let Some(client) = self.clients.get(from_client_id) else {
            // This means that the client is not in the list of connected clients.
            // Likely a bug in the client or a malicious client.
            warn!("received a packet from an invalid peer");
            return;
        };

        let Ok(packet) = PacketType::from_bytes(&data) else {
            warn!("received an invalid packet from {}", from_client_id);
            return;
        };

        match client.state {
            ClientState::Connected => {
                self.handle_unauthenticated_packet(from_client_id, &packet)
                    .await
            }
            ClientState::Authenticated { app_id } => {
                self.handle_authenticated_packet(from_client_id, app_id, &packet)
                    .await
            }
            ClientState::InRoom { app_id, room_id } => {
                self.handle_in_room_packet(from_client_id, app_id, room_id, &packet, &channel)
                    .await
            }
        }
    }

    /// Delegates packets to various handlers when the client has yet to authenticate.
    async fn handle_unauthenticated_packet(&mut self, from_client_id: u64, packet: &PacketType) {
        match packet {
            PacketType::Authenticate { app_id, version } => {
                debug!(
                    "client {} attempting authentication with app_id: {}, version: {}",
                    from_client_id, app_id, version
                );
                AuthHandler::new(
                    &mut self.transport,
                    &self.http_client,
                    &mut self.clients,
                    &mut self.apps,
                    &self.config,
                )
                .authenticate_client(from_client_id, app_id, version)
                .await;
            }
            _ => {
                // TODO: should probably alert the client that they need to authenticate first!
                warn!(
                    "unexpected packet type from {} in un-authenticated state: {:?}.",
                    from_client_id, packet
                );
            }
        }
    }

    /// Delegates packets to various handlers when the client is authenticated, but not in a room.
    async fn handle_authenticated_packet(
        &mut self,
        from_client_id: u64,
        client_app_id: u64,
        packet: &PacketType,
    ) {
        let mut rh = RoomHandler::new(&mut self.transport, &mut self.apps, &mut self.clients);

        match packet {
            PacketType::CreateRoom {
                is_public,
                metadata,
            } => {
                debug!(
                    "client {} creating room (public: {}, metadata: {})",
                    from_client_id, is_public, metadata
                );
                rh.create_room(from_client_id, client_app_id, *is_public, metadata)
                    .await;
            }
            PacketType::ReqJoin { room_id, metadata } => {
                debug!(
                    "client {} requesting to join room {} with metadata: {}",
                    from_client_id, room_id, metadata
                );
                rh.recv_join_req(from_client_id, client_app_id, room_id, metadata)
                    .await;
            }
            PacketType::ReqRooms => {
                debug!("client {} requesting room list", from_client_id);
                rh.send_rooms(from_client_id, client_app_id).await;
            }
            _ => {
                // TODO: should probably alert the client that they are in an unexpected state?
                warn!(
                    "unexpected packet type from {} in authenticated state: {:?}.",
                    from_client_id, packet
                );
            }
        }
    }

    /// Delegates packets to various handlers when the client is in a room.
    async fn handle_in_room_packet(
        &mut self,
        from_client_id: u64,
        client_app_id: u64,
        client_room_id: u64,
        packet: &PacketType,
        channel: &TransferChannel,
    ) {
        match packet {
            PacketType::UpdateRoom {
                metadata,
                room_id: _room_id,
            } => {
                debug!(
                    "client {} updating room {} with metadata: {}",
                    from_client_id, client_room_id, metadata
                );
                RoomHandler::new(&mut self.transport, &mut self.apps, &mut self.clients)
                    .update_room(from_client_id, client_app_id, client_room_id, metadata)
                    .await;
            }
            PacketType::JoinRes {
                target_id,
                allowed,
                room_id: _room_id,
            } => {
                debug!(
                    "client {} responding to join request for target {} (allowed: {})",
                    from_client_id, target_id, allowed
                );
                RoomHandler::new(&mut self.transport, &mut self.apps, &mut self.clients)
                    .recv_join_res(client_app_id, *target_id, client_room_id, allowed)
                    .await;
            }
            PacketType::GameData { from_peer, data } => {
                debug!(
                    "routing game data from client {} (peer {}) to peer {}: {} bytes",
                    from_client_id,
                    from_peer,
                    from_peer,
                    data.len()
                );
                GameDataHandler::new(&mut self.transport, &mut self.apps)
                    .route_game_data(
                        from_client_id,
                        client_app_id,
                        client_room_id,
                        *from_peer,
                        data,
                        channel,
                    )
                    .await;
            }
            _ => {
                // TODO: should probably alert the client that they are in an unexpected state?
                warn!(
                    "unexpected packet type from {} in room state: {:?}.",
                    from_client_id, packet
                );
            }
        }
    }

    /// Forcefully disconnects all clients from the server.
    /// Should be called when the server shuts down.
    pub async fn cleanup(&mut self) {
        let mut disconnects: Vec<u64> = Vec::new();
        let mut to_remove: Vec<(u64, u64)> = Vec::new();

        for app in self.apps.iter() {
            for room in app.rooms.iter() {
                disconnects.extend(room.get_clients().iter().copied());
                to_remove.push((app.id, room.id));
            }
        }

        info!("disconnecting {} peers", disconnects.len());

        let mut dh = DisconnectHandler::new(&mut self.transport, &mut self.clients, &mut self.apps);

        for id in disconnects {
            dh.force_disconnect(id).await;
        }

        let mut rh = RoomHandler::new(&mut self.transport, &mut self.apps, &mut self.clients);

        for (app_id, room_id) in to_remove {
            rh.remove_room(app_id, room_id);
        }
    }
}
