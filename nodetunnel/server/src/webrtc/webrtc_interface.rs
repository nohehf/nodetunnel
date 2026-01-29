use crate::transport::r#trait::Transport;
use crate::udp::common::{ServerEvent, TransferChannel};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, trace, warn};
use webrtc::data_channel::RTCDataChannel;

pub struct WebRTCInterface {
    // Map from data channel ID to client ID
    data_channels: Arc<tokio::sync::RwLock<HashMap<u16, (u64, Arc<RTCDataChannel>)>>>,
    // Event channel for sending events to the relay server
    event_tx: mpsc::UnboundedSender<ServerEvent>,
    // Next client ID to assign
    next_client_id: Arc<std::sync::Mutex<u64>>,
}

impl WebRTCInterface {
    pub fn new(event_tx: mpsc::UnboundedSender<ServerEvent>) -> Self {
        Self {
            data_channels: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            event_tx,
            next_client_id: Arc::new(std::sync::Mutex::new(1)),
        }
    }

    /// Register a data channel with a new client ID
    pub fn register_data_channel(&self, dc_id: u16, data_channel: Arc<RTCDataChannel>) {
        let span = tracing::span!(tracing::Level::INFO, "transport", transport = "WebRTC");
        let _enter = span.enter();
        let client_id = {
            let mut next_id = self.next_client_id.lock().unwrap();
            let id = *next_id;
            *next_id += 1;
            id
        };

        info!(
            "Registering data channel {} with client_id: {}",
            dc_id, client_id
        );

        // Use tokio::spawn to handle async operations
        let channels = Arc::clone(&self.data_channels);
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let mut channels = channels.write().await;
            channels.insert(dc_id, (client_id, data_channel.clone()));

            // Send client connected event
            debug!("Sending ClientConnected event for client {}", client_id);
            let _ = event_tx.send(ServerEvent::ClientConnected { client_id });
        });
    }

    /// Unregister a data channel
    pub fn unregister_data_channel(&self, dc_id: u16) {
        let span = tracing::span!(tracing::Level::INFO, "transport", transport = "WebRTC");
        let channels = Arc::clone(&self.data_channels);
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let _enter = span.enter();
            let mut channels = channels.write().await;

            if let Some((client_id, _)) = channels.remove(&dc_id) {
                info!(
                    "Unregistering data channel {} (client_id: {})",
                    dc_id, client_id
                );
                // Send client disconnected event
                debug!("Sending ClientDisconnected event for client {}", client_id);
                let _ = event_tx.send(ServerEvent::ClientDisconnected { client_id });
            }
        });
    }

    /// Handle a message received on a data channel
    pub fn handle_data_channel_message(&self, dc_id: u16, data: Vec<u8>) {
        let span = tracing::span!(tracing::Level::TRACE, "transport", transport = "WebRTC");
        let channels = Arc::clone(&self.data_channels);
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let _enter = span.enter();
            let channels = channels.read().await;

            if let Some((client_id, _)) = channels.get(&dc_id) {
                trace!(
                    "Received packet from client {} via data channel {}: {} bytes",
                    client_id,
                    dc_id,
                    data.len()
                );

                // Skip keepalive packets (0xFF 0xFF)
                if data.len() == 2 && data[0] == 0xFF && data[1] == 0xFF {
                    trace!("Skipping keepalive packet from client {}", client_id);
                    return;
                }

                debug!(
                    "Processing packet from client {}: {} bytes",
                    client_id,
                    data.len()
                );
                // Send packet received event
                let _ = event_tx.send(ServerEvent::PacketReceived {
                    client_id: *client_id,
                    data,
                    channel: TransferChannel::Reliable, // WebRTC data channels are reliable
                });
            } else {
                warn!("Received message on unknown data channel: {}", dc_id);
            }
        });
    }

    /// Remove a client (called on disconnect)
    pub fn remove_client(&self, client_id: &u64) {
        let span = tracing::span!(tracing::Level::INFO, "transport", transport = "WebRTC");
        let channels = Arc::clone(&self.data_channels);
        let client_id = *client_id;
        tokio::spawn(async move {
            let _enter = span.enter();
            let mut channels = channels.write().await;

            // Find and remove the data channel for this client
            let mut to_remove = None;
            for (dc_id, (id, _)) in channels.iter() {
                if id == &client_id {
                    to_remove = Some(*dc_id);
                    break;
                }
            }

            if let Some(dc_id) = to_remove {
                channels.remove(&dc_id);
                info!("Removed client {} (data channel {})", client_id, dc_id);
            }
        });
    }
}

impl Transport for WebRTCInterface {
    async fn send(
        &mut self,
        client_id: u64,
        data: Vec<u8>,
        _channel: TransferChannel,
    ) -> Result<(), std::io::Error> {
        let span = tracing::span!(tracing::Level::TRACE, "transport", transport = "WebRTC");
        let _enter = span.enter();
        let channels = self.data_channels.read().await;

        // Find the data channel for this client
        for (dc_id, (target_client_id, dc)) in channels.iter() {
            if *target_client_id == client_id {
                trace!(
                    "Sending {} bytes to client {} via data channel {}",
                    data.len(),
                    client_id,
                    dc_id
                );

                // Check if data channel is open
                use webrtc::data_channel::data_channel_state::RTCDataChannelState;
                let data_len = data.len();
                if dc.ready_state() == RTCDataChannelState::Open {
                    dc.send(&data.into()).await.map_err(|e| {
                        warn!(
                            "Failed to send {} bytes to client {} via data channel {}: {}",
                            data_len, client_id, dc_id, e
                        );
                        std::io::Error::new(
                            std::io::ErrorKind::Other,
                            format!("Failed to send: {}", e),
                        )
                    })?;
                    debug!(
                        "Successfully sent {} bytes to client {} via data channel {}",
                        data_len, client_id, dc_id
                    );
                    return Ok(());
                } else {
                    warn!(
                        "Data channel {} for client {} is not open (state: {:?})",
                        dc_id,
                        client_id,
                        dc.ready_state()
                    );
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        format!("Data channel {} not open", dc_id),
                    ));
                }
            }
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("No data channel found for client {}", client_id),
        ))
    }

    fn remove_client(&mut self, client_id: &u64) {
        // WebRTC doesn't need mutability, delegate to the existing method
        WebRTCInterface::remove_client(self, client_id);
    }
}

impl Clone for WebRTCInterface {
    fn clone(&self) -> Self {
        Self {
            data_channels: Arc::clone(&self.data_channels),
            event_tx: self.event_tx.clone(),
            next_client_id: Arc::clone(&self.next_client_id),
        }
    }
}
