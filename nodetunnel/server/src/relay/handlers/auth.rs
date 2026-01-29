use crate::config::loader::Config;
use crate::relay::apps::Apps;
use crate::relay::clients::{ClientState, Clients};
use crate::transport::server::TransportRegistry;
use crate::udp::common::TransferChannel;
use nodetunnel_core::protocol::packet::PacketType;
use reqwest::StatusCode;
use std::error::Error;
use tracing::warn;

pub struct AuthHandler<'a> {
    transport: &'a mut TransportRegistry,
    http: &'a reqwest::Client,
    clients: &'a mut Clients,
    apps: &'a mut Apps,
    config: &'a Config,
}

impl<'a> AuthHandler<'a> {
    pub fn new(
        transport: &'a mut TransportRegistry,
        http: &'a reqwest::Client,
        clients: &'a mut Clients,
        apps: &'a mut Apps,
        config: &'a Config,
    ) -> Self {
        Self {
            transport,
            http,
            clients,
            apps,
            config,
        }
    }

    pub async fn authenticate_client(&mut self, sender_id: u64, app_token: &str, version: &str) {
        // Check version
        if !self.is_version_allowed(version) {
            let msg = format!("Version {version} is not allowed.");
            self.send_err(sender_id, &msg).await;
            self.force_disconnect(sender_id).await;
            return;
        }

        // Check app whitelist
        if !self.app_allowed(app_token).await {
            let msg = format!("App token {app_token} is not allowed.");
            self.send_err(sender_id, &msg).await;
            self.force_disconnect(sender_id).await;
            return;
        }

        let Some(client) = self.clients.get_mut(sender_id) else {
            warn!("attempted to authenticate a missing client {}", sender_id);
            return;
        };

        let app_id = match self.apps.get_by_token(app_token) {
            Some(app) => app.id,
            None => self.apps.create(app_token.to_string()),
        };

        client.state = ClientState::Authenticated { app_id };
        self.send_packet(
            sender_id,
            &PacketType::ClientAuthenticated,
            TransferChannel::Reliable,
        )
        .await;
    }

    fn is_version_allowed(&self, version: &str) -> bool {
        let versions = &self.config.allowed_versions;
        versions.contains(&version.to_string())
    }

    async fn app_allowed(&mut self, app: &str) -> bool {
        let remote = &self.config.remote_whitelist_endpoint;
        let token = &self.config.remote_whitelist_token;

        if remote.is_empty() || token.is_empty() {
            self.check_local_whitelist(app)
        } else {
            match self.check_remote_whitelist(remote, app, token).await {
                Ok(res) => res,
                Err(e) => {
                    warn!(
                        "failed to check remote whitelist, defaulting to local: {}",
                        e
                    );
                    self.check_local_whitelist(app)
                }
            }
        }
    }

    fn check_local_whitelist(&self, app: &str) -> bool {
        let whitelist = &self.config.whitelist;

        if whitelist.is_empty() {
            true
        } else {
            whitelist.contains(&app.to_string())
        }
    }

    async fn check_remote_whitelist(
        &self,
        endpoint: &str,
        app: &str,
        relay_token: &str,
    ) -> Result<bool, Box<dyn Error>> {
        let url = format!("{endpoint}/{app}");

        let res = self
            .http
            .get(&url)
            .header("X-Relay-Token", relay_token)
            .send()
            .await?;

        match res.status() {
            StatusCode::OK => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            s => Err(format!("unexpected status from endpoint: {s}").into()),
        }
    }

    async fn send_packet(&mut self, target: u64, packet: &PacketType, channel: TransferChannel) {
        if let Err(e) = self
            .transport
            .send(target, packet.to_bytes(), channel)
            .await
        {
            warn!("failed to send packet: {}", e);
        }
    }

    async fn send_err(&mut self, target: u64, msg: &str) {
        self.send_packet(
            target,
            &PacketType::Error {
                error_code: 401,
                error_message: msg.to_string(),
            },
            TransferChannel::Reliable,
        )
        .await;
    }

    async fn force_disconnect(&mut self, target: u64) {
        self.send_packet(
            target,
            &PacketType::ForceDisconnect,
            TransferChannel::Reliable,
        )
        .await;
        self.transport.remove_client(&target);
    }
}
