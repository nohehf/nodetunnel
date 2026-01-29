use crate::relay::apps::Apps;
use crate::relay::clients::{ClientState, Clients};
use crate::relay::handlers::room::RoomHandler;
use crate::udp::common::TransferChannel;
use crate::udp::paper_interface::PaperInterface;
use nodetunnel_core::protocol::packet::PacketType;
use tracing::{info, warn};

struct DisconnectInfo {
    is_host: bool,
    godot_id: i32,
    other_peers: Vec<u64>,
}

pub struct DisconnectHandler<'a> {
    transport: &'a mut PaperInterface,
    clients: &'a mut Clients,
    apps: &'a mut Apps,
}

impl<'a> DisconnectHandler<'a> {
    pub fn new(
        transport: &'a mut PaperInterface,
        clients: &'a mut Clients,
        apps: &'a mut Apps,
    ) -> Self {
        Self {
            transport,
            clients,
            apps,
        }
    }

    pub async fn handle_disconnect(&mut self, client_id: u64) {
        let Some(client) = self.clients.remove(client_id) else {
            warn!("unregistered client disconnected");
            return;
        };

        if let ClientState::InRoom { app_id, room_id } = client.state {
            self.handle_room_disconnect(client_id, app_id, room_id)
                .await;
        }
    }

    async fn handle_room_disconnect(&mut self, sender_id: u64, app_id: u64, room_id: u64) {
        let disconnect_info = {
            let Some(app) = self.apps.get_mut(app_id) else {
                warn!("{} had invalid app_id on disconnect", sender_id);
                return;
            };

            let Some(room) = app.rooms.get(room_id) else {
                warn!("{} had invalid room_id on disconnect", sender_id);
                return;
            };

            let Some(godot_id) = room.client_to_gd(sender_id) else {
                warn!("{} not found in their room on disconnect", sender_id);
                return;
            };

            DisconnectInfo {
                is_host: room.get_host() == sender_id,
                godot_id,
                other_peers: room
                    .get_clients()
                    .into_iter()
                    .filter(|&id| id != sender_id)
                    .collect(),
            }
        };

        if disconnect_info.is_host {
            self.handle_host_disconnect(app_id, room_id, disconnect_info.other_peers)
                .await;
        } else {
            self.handle_peer_disconnect(
                app_id,
                room_id,
                sender_id,
                disconnect_info.godot_id,
                disconnect_info.other_peers,
            )
            .await;
        }
    }

    async fn handle_host_disconnect(&mut self, app_id: u64, room_id: u64, peers_to_kick: Vec<u64>) {
        info!("host disconnected");
        RoomHandler::new(&mut self.transport, self.apps, self.clients).remove_room(app_id, room_id);

        for peer_id in peers_to_kick {
            self.clients.remove(peer_id);
            self.force_disconnect(peer_id).await;
        }
    }

    async fn handle_peer_disconnect(
        &mut self,
        app_id: u64,
        room_id: u64,
        client_id: u64,
        peer_godot_id: i32,
        other_peers: Vec<u64>,
    ) {
        info!("peer disconnected");
        if let Some(app) = self.apps.get_mut(app_id) {
            if let Some(room) = app.rooms.get_mut(room_id) {
                room.remove_peer(client_id);
            }
        }

        for peer_id in other_peers {
            self.send_packet(
                peer_id,
                &PacketType::PeerLeftRoom {
                    peer_id: peer_godot_id,
                },
                TransferChannel::Reliable,
            )
            .await;
        }
    }

    pub async fn force_disconnect(&mut self, target_client: u64) {
        self.send_packet(
            target_client,
            &PacketType::ForceDisconnect,
            TransferChannel::Reliable,
        )
        .await;
        self.transport.remove_client(&target_client);
    }

    async fn send_packet(
        &mut self,
        target_client: u64,
        packet: &PacketType,
        channel: TransferChannel,
    ) {
        if let Err(e) = self
            .transport
            .send(target_client, packet.to_bytes(), channel)
            .await
        {
            warn!("failed to send packet: {}", e);
        }
    }
}
