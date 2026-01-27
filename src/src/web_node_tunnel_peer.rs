use crate::relay_client::client::RelayClient;
use crate::relay_client::events::RelayEvent;
use crate::transport::common::Channel;
use crate::transport::webrtc::WebRTCClientTransport;
use godot::builtin::{Array, Callable, Dictionary, GString, PackedByteArray, Variant};
use godot::classes::multiplayer_peer::{ConnectionStatus, TransferMode};
use godot::classes::{IMultiplayerPeerExtension, MultiplayerPeerExtension, Node};
use godot::global::{Error, godot_error, godot_print, godot_warn};
use godot::meta::ToGodot;
use godot::obj::{Base, Gd, NewAlloc, WithUserSignals};
use godot::prelude::{GodotClass, godot_api};
use std::time::{Duration, Instant};

struct GamePacket {
    from_peer: i32,
    data: Vec<u8>,
    transfer_mode: TransferMode,
}

// TODO: Merge this with NodeTunnelPeer and swap the transport based on the platform
#[derive(GodotClass)]
#[class(tool, base=MultiplayerPeerExtension)]
struct WebNodeTunnelPeer {
    app_id: String,
    unique_id: i32,
    #[var]
    room_id: GString,
    #[var]
    join_validation: Callable,
    connection_status: ConnectionStatus,
    target_peer: i32,
    transfer_mode: TransferMode,
    incoming_packets: Vec<GamePacket>,
    relay_client: RelayClient<WebRTCClientTransport>,
    outgoing_queue: Vec<(i32, Vec<u8>, Channel)>,
    last_poll_time: Option<Instant>,
    // Minimal node ONLY for WebRTC signals (unavoidable - Godot requires nodes for signals)
    signal_node: Option<Gd<Node>>,
    // Callable to poll() method (created in init when we have access)
    poll_callable: Callable,
    // Callable to set_pending_offer() method (created in init when we have access)
    offer_callable: Callable,
    base: Base<MultiplayerPeerExtension>,
}

#[godot_api]
impl WebNodeTunnelPeer {
    #[signal]
    fn authenticated();

    #[signal]
    fn error(error_message: String);

    #[signal]
    fn room_connected();

    #[signal]
    fn forced_disconnect();

    #[signal]
    fn rooms_received(rooms: Array<Variant>);

    /// Internal poll method - exposed as func so SignalHandler can call it
    #[func]
    fn _internal_poll(&mut self) {
        // Call the actual poll() implementation
        self.poll();
    }

    /// Set pending offer in transport (called from SignalHandler callback)
    #[func]
    fn _set_pending_offer(&mut self, type_: GString, sdp: GString) {
        if let Some(transport) = self.relay_client.transport_mut() {
            transport.set_pending_offer(type_, sdp);
        }
    }

    #[func]
    fn connect_to_relay(&mut self, relay_address: String, app_id: String) -> Error {
        godot_print!("[WebNodeTunnelPeer] Connecting to relay: {}", relay_address);
        self.app_id = app_id;

        // Create minimal node ONLY for WebRTC signals (must be in scene tree)
        let signal_node = if let Some(ref node) = self.signal_node {
            node.clone()
        } else {
            let mut node = Node::new_alloc();
            node.set_name("WebRTCSignalNode");

            // Add to scene tree (deferred to avoid "busy" error)
            use godot::builtin::{StringName, Variant};
            use godot::classes::Engine;
            use godot::obj::Singleton;

            let engine = Engine::singleton();
            if let Some(main_loop) = engine.get_main_loop() {
                let mut main_loop_obj = main_loop.upcast::<godot::classes::Object>();
                let get_root_name = StringName::from("get_root");
                let root_result: Variant = main_loop_obj.call(&get_root_name, &[]);

                if let Ok(root_gd) = root_result.try_to::<Gd<Node>>() {
                    let mut root_obj = root_gd.upcast::<godot::classes::Object>();
                    let call_deferred_name = StringName::from("call_deferred");
                    let add_child_name = StringName::from("add_child");
                    let node_variant = node.clone().upcast::<godot::classes::Object>().to_variant();
                    let _ = root_obj.call(
                        &call_deferred_name,
                        &[add_child_name.to_variant(), node_variant],
                    );
                    godot_print!(
                        "[WebNodeTunnelPeer] Added signal node to scene tree (only for WebRTC signals)"
                    );
                }
            }

            self.signal_node = Some(node.clone());
            node
        };

        godot_print!("Creating WebRTC transport");
        // Use the poll callable and offer callable created in init() for automatic polling and offer handling
        let transport = match WebRTCClientTransport::new(
            relay_address,
            signal_node,
            self.poll_callable.clone(),
            self.offer_callable.clone(),
        ) {
            Ok(t) => t,
            Err(e) => {
                godot_error!("[NodeTunnel] Failed to create transport: {}", e);
                return Error::ERR_CANT_CREATE;
            }
        };

        godot_print!("Connecting to relay");
        self.relay_client.connect(transport);
        self.connection_status = ConnectionStatus::CONNECTING;

        Error::OK
    }

    #[func]
    fn host_room(&mut self, public: bool, metadata: String) -> Error {
        match self.relay_client.req_create_room(public, metadata) {
            Ok(_) => Error::OK,
            Err(e) => {
                godot_error!("[NodeTunnel] Failed to create room: {}", e);
                Error::ERR_CANT_CREATE
            }
        }
    }

    #[func]
    fn get_rooms(&mut self) -> Error {
        match self.relay_client.req_rooms() {
            Ok(_) => Error::OK,
            Err(e) => {
                godot_error!("[NodeTunnel] Failed to get rooms: {}", e);
                Error::ERR_CANT_CREATE
            }
        }
    }

    #[func]
    fn join_room(&mut self, host_id: String, #[opt(default = "")] metadata: GString) -> Error {
        match self
            .relay_client
            .req_join_room(host_id, metadata.to_string())
        {
            Ok(_) => Error::OK,
            Err(e) => {
                godot_error!("[NodeTunnel] Failed to join room: {}", e);
                Error::ERR_CANT_CREATE
            }
        }
    }

    #[func]
    fn update_room(&mut self, metadata: String) -> Error {
        match self
            .relay_client
            .req_update_room(&self.room_id.to_string(), &metadata)
        {
            Ok(_) => Error::OK,
            Err(e) => {
                godot_error!("[NodeTunnel] Failed to update room: {}", e);
                Error::ERR_CANT_CREATE
            }
        }
    }

    fn handle_relay_event(&mut self, event: RelayEvent) {
        match event {
            RelayEvent::ConnectedToServer => {
                match self.relay_client.req_auth(self.app_id.clone()) {
                    Ok(_) => {}
                    Err(e) => {
                        godot_error!("[NodeTunnel] Failed to authenticate: {}", e);
                        self.signals().error().emit(e.to_string());
                    }
                }
            }
            RelayEvent::Authenticated => {
                self.signals().authenticated().emit();
            }
            RelayEvent::RoomsReceived { rooms } => {
                let mut room_array = Array::new();

                for room in rooms {
                    let mut room_dict = Dictionary::new();
                    room_dict.set("id", room.id.clone());
                    room_dict.set("metadata", room.metadata.clone());

                    room_array.push(&room_dict.to_variant());
                }

                self.signals().rooms_received().emit(&room_array)
            }
            RelayEvent::RoomJoined { room_id, peer_id } => {
                self.connection_status = ConnectionStatus::CONNECTED;
                self.unique_id = peer_id;
                self.room_id = room_id.to_godot();

                if !self.is_server() {
                    self.signals().peer_connected().emit(1);
                }

                self.signals().room_connected().emit();
            }
            RelayEvent::PeerJoinAttempt {
                client_id,
                metadata,
            } => {
                if self.is_server() {
                    let mut allowed = true;

                    if self.join_validation.is_valid() {
                        allowed = self
                            .join_validation
                            .call(&[metadata.to_variant()])
                            .booleanize()
                    }

                    self.relay_client
                        .send_join_response(self.room_id.to_string(), client_id, allowed)
                        .expect("todo");
                }
            }
            RelayEvent::PeerJoinedRoom { peer_id } => {
                if self.is_server() {
                    self.signals().peer_connected().emit(peer_id as i64);
                }
            }
            RelayEvent::PeerLeftRoom { peer_id } => {
                self.signals().peer_disconnected().emit(peer_id as i64);
            }
            RelayEvent::GameDataReceived {
                channel,
                from_peer,
                data,
            } => {
                let transfer_mode = match channel {
                    Channel::Reliable => TransferMode::RELIABLE,
                    Channel::Unreliable => TransferMode::UNRELIABLE,
                };

                self.incoming_packets.push(GamePacket {
                    transfer_mode,
                    from_peer,
                    data,
                });
            }
            RelayEvent::ForceDisconnect => {
                if self.connection_status == ConnectionStatus::CONNECTED {
                    godot_warn!("[NodeTunnel] Client was forcibly disconnected from relay");
                    self.close();
                    self.signals().forced_disconnect().emit();
                }
            }
            RelayEvent::Error {
                error_code,
                error_message,
            } => {
                godot_error!("[NodeTunnel] Relay error {}: {}", error_code, error_message);
                self.signals().error().emit(error_message);
            }
        }
    }
}

#[godot_api]
impl IMultiplayerPeerExtension for WebNodeTunnelPeer {
    fn init(base: Base<Self::Base>) -> Self {
        godot_print!("Initializing WebNodeTunnelPeer");

        // Create callables to methods (can only do this in init)
        let self_gd = base.to_init_gd();
        let self_obj = self_gd.upcast::<godot::classes::Object>();
        let poll_callable = self_obj.callable("_internal_poll");
        let offer_callable = self_obj.callable("_set_pending_offer");

        Self {
            app_id: "".to_string(),
            room_id: "".to_godot(),
            join_validation: Callable::invalid(),
            unique_id: 0,
            connection_status: ConnectionStatus::DISCONNECTED,
            target_peer: 0,
            transfer_mode: TransferMode::UNRELIABLE,
            incoming_packets: vec![],
            relay_client: RelayClient::<WebRTCClientTransport>::new(),
            outgoing_queue: vec![],
            last_poll_time: None,
            signal_node: None,
            poll_callable,
            offer_callable,
            base,
        }
    }

    fn get_available_packet_count(&self) -> i32 {
        self.incoming_packets.len() as i32
    }

    fn get_max_packet_size(&self) -> i32 {
        1 << 24
    }

    fn get_packet_script(&mut self) -> PackedByteArray {
        if !self.incoming_packets.is_empty() {
            let packet = self.incoming_packets.remove(0);
            PackedByteArray::from(packet.data.as_slice())
        } else {
            PackedByteArray::new()
        }
    }

    fn put_packet_script(&mut self, p_buffer: PackedByteArray) -> Error {
        let data: Vec<u8> = p_buffer.to_vec();

        let channel = match self.transfer_mode {
            TransferMode::RELIABLE => Channel::Reliable,
            _ => Channel::Unreliable,
        };

        self.outgoing_queue.push((self.target_peer, data, channel));

        Error::OK
    }

    fn get_packet_channel(&self) -> i32 {
        0
    }

    fn get_packet_mode(&self) -> TransferMode {
        self.incoming_packets
            .first()
            .map(|p| p.transfer_mode)
            .unwrap_or(TransferMode::UNRELIABLE)
    }

    fn set_transfer_channel(&mut self, p_channel: i32) {
        if p_channel != 0 {
            godot_warn!("[NodeTunnel] Set to invalid channel: {}", p_channel);
        }
    }

    fn get_transfer_channel(&self) -> i32 {
        0
    }

    fn set_transfer_mode(&mut self, p_mode: TransferMode) {
        self.transfer_mode = p_mode;
    }

    fn get_transfer_mode(&self) -> TransferMode {
        self.transfer_mode
    }

    fn set_target_peer(&mut self, p_peer: i32) {
        self.target_peer = p_peer;
    }

    fn get_packet_peer(&self) -> i32 {
        self.incoming_packets
            .first()
            .map(|p| p.from_peer)
            .unwrap_or(0)
    }

    fn is_server(&self) -> bool {
        self.unique_id == 1
    }

    fn poll(&mut self) {
        // This is called automatically by Godot for MultiplayerPeerExtension
        // But only when the peer is set as the multiplayer peer!
        // If poll() isn't being called, set this peer as the multiplayer peer:
        // get_multiplayer().multiplayer_peer = peer
        let now = Instant::now();
        let delta = match self.last_poll_time {
            Some(last) => now.duration_since(last),
            None => Duration::ZERO,
        };
        self.last_poll_time = Some(now);

        match self.relay_client.update(delta) {
            Ok(events) => {
                for event in events {
                    self.handle_relay_event(event)
                }
            }
            Err(e) => {
                godot_error!("[NodeTunnel] Relay error: {}", e);
            }
        }

        for (peer, data, channel) in self.outgoing_queue.drain(..) {
            match self.relay_client.send_game_data(peer, data, channel) {
                Ok(_) => {}
                Err(e) => {
                    godot_error!("[NodeTunnel] Failed to send game data: {}", e);
                }
            }
        }
    }

    fn close(&mut self) {
        if self.connection_status == ConnectionStatus::DISCONNECTED
            || !self.relay_client.is_connected()
        {
            godot_warn!("[NodeTunnel] Attempted to close connection while disconnected");
            return;
        }

        self.unique_id = 0;
        self.connection_status = ConnectionStatus::DISCONNECTED;

        // Clean up signal node if it exists
        if let Some(mut node) = self.signal_node.take() {
            node.queue_free();
        }
    }

    fn disconnect_peer(&mut self, _p_peer: i32, _p_force: bool) {}

    fn get_unique_id(&self) -> i32 {
        self.unique_id
    }

    fn is_server_relay_supported(&self) -> bool {
        true
    }

    fn get_connection_status(&self) -> ConnectionStatus {
        self.connection_status
    }
}
