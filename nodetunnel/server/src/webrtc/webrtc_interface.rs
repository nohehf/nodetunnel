use crate::transport::r#trait::Transport;
use crate::udp::common::{ServerEvent, TransferChannel};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, trace, warn};
use webrtc::data_channel::RTCDataChannel;

pub struct WebRTCInterface {
    // Map from client ID to data channel (for sending and receiving)
    client_channels: Arc<tokio::sync::RwLock<HashMap<u64, Arc<RTCDataChannel>>>>,
    // Event channel for sending events to the relay server
    event_tx: mpsc::UnboundedSender<ServerEvent>,
    // Next client ID to assign
    next_client_id: Arc<std::sync::Mutex<u64>>,
}

impl WebRTCInterface {
    pub fn new(event_tx: mpsc::UnboundedSender<ServerEvent>) -> Self {
        Self {
            client_channels: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
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
            "Registering data channel {} (dc_id) with client_id: {}",
            dc_id, client_id
        );

        // Use tokio::spawn to handle async operations
        let client_channels = Arc::clone(&self.client_channels);
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let mut client_map = client_channels.write().await;
            
            // Register the data channel (dc_id is just for logging, not used as key)
            client_map.insert(client_id, data_channel.clone());

            info!(
                "Data channel {} (dc_id) successfully registered for client {} (total clients: {})",
                dc_id,
                client_id,
                client_map.len()
            );

            // Send client connected event
            debug!("Sending ClientConnected event for client {}", client_id);
            let _ = event_tx.send(ServerEvent::ClientConnected { client_id });
        });
    }

    /// Unregister a data channel by finding it in the client_channels map
    pub fn unregister_data_channel(&self, dc_id: u16) {
        let span = tracing::span!(tracing::Level::INFO, "transport", transport = "WebRTC");
        let client_channels = Arc::clone(&self.client_channels);
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let _enter = span.enter();
            let mut client_map = client_channels.write().await;

            // Find the client_id by matching the dc_id (we need to iterate since dc_id can collide)
            // This is a bit inefficient but necessary since dc_id is not unique
            let mut client_to_remove = None;
            for (client_id, dc) in client_map.iter() {
                if dc.id() == dc_id {
                    client_to_remove = Some(*client_id);
                    break;
                }
            }

            if let Some(client_id) = client_to_remove {
                client_map.remove(&client_id);
                info!(
                    "Unregistering data channel {} (dc_id) for client_id: {}. Remaining clients: {}",
                    dc_id, client_id, client_map.len()
                );
                if !client_map.is_empty() {
                    info!("Remaining clients:");
                    for (remaining_client_id, _) in client_map.iter() {
                        info!("  client_id={}", remaining_client_id);
                    }
                }
                // Send client disconnected event
                debug!("Sending ClientDisconnected event for client {}", client_id);
                let _ = event_tx.send(ServerEvent::ClientDisconnected { client_id });
            } else {
                warn!("Attempted to unregister unknown data channel: {}", dc_id);
            }
        });
    }

    /// Handle a message received on a data channel
    /// We receive the data channel Arc and look up which client it belongs to
    pub fn handle_data_channel_message(&self, data_channel: Arc<RTCDataChannel>, data: Vec<u8>) {
        let span = tracing::span!(tracing::Level::TRACE, "transport", transport = "WebRTC");
        let client_channels = Arc::clone(&self.client_channels);
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let _enter = span.enter();
            let client_map = client_channels.read().await;

            // Find the client_id by matching the data channel Arc pointer
            // We compare Arc pointers using Arc::ptr_eq for efficiency
            let mut found_client_id = None;
            for (client_id, dc) in client_map.iter() {
                if Arc::ptr_eq(dc, &data_channel) {
                    found_client_id = Some(*client_id);
                    break;
                }
            }

            if let Some(client_id) = found_client_id {
                trace!(
                    "Received packet from client {} via data channel (dc_id: {}): {} bytes",
                    client_id,
                    data_channel.id(),
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
                    client_id,
                    data,
                    channel: TransferChannel::Reliable, // WebRTC data channels are reliable
                });
            } else {
                warn!("Received message on unknown data channel (dc_id: {})", data_channel.id());
            }
        });
    }

    /// Remove a client (called on disconnect)
    pub fn remove_client(&self, client_id: &u64) {
        let span = tracing::span!(tracing::Level::INFO, "transport", transport = "WebRTC");
        let client_channels = Arc::clone(&self.client_channels);
        let client_id = *client_id;
        tokio::spawn(async move {
            let _enter = span.enter();
            let mut client_map = client_channels.write().await;

            info!(
                "remove_client() called for client_id={}, current clients: {}",
                client_id,
                client_map.len()
            );

            // Remove from client map
            if let Some(dc) = client_map.remove(&client_id) {
                info!(
                    "Removed client {} (dc_id: {}). Remaining clients: {}",
                    client_id,
                    dc.id(),
                    client_map.len()
                );
            } else {
                warn!(
                    "remove_client() called for client_id={} but no data channel found. Available clients: {:?}",
                    client_id,
                    client_map.keys().collect::<Vec<_>>()
                );
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
        let client_channels = self.client_channels.read().await;

        debug!(
            "Attempting to send {} bytes to client {} (total clients: {})",
            data.len(),
            client_id,
            client_channels.len()
        );

        // Log all registered clients for debugging
        if client_channels.is_empty() {
            warn!("No clients registered at all!");
        } else {
            debug!("Registered clients:");
            for (registered_client_id, dc) in client_channels.iter() {
                use webrtc::data_channel::data_channel_state::RTCDataChannelState;
                debug!(
                    "  client_id={}, dc_id={}, state={:?}",
                    registered_client_id,
                    dc.id(),
                    dc.ready_state()
                );
            }
        }

        // Find the data channel for this client
        if let Some(dc) = client_channels.get(&client_id) {
            let dc_id = dc.id();
            trace!(
                "Found data channel {} (dc_id) for client {}, sending {} bytes",
                dc_id,
                client_id,
                data.len()
            );

            // Check if data channel is open
            use webrtc::data_channel::data_channel_state::RTCDataChannelState;
            let data_len = data.len();
            let channel_state = dc.ready_state();
            if channel_state == RTCDataChannelState::Open {
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
                    "Data channel {} (dc_id) for client {} is not open (state: {:?})",
                    dc_id,
                    client_id,
                    channel_state
                );
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    format!("Data channel {} not open (state: {:?})", dc_id, channel_state),
                ));
            }
        }

        warn!(
            "No data channel found for client {}. Available clients: {:?}",
            client_id,
            client_channels.keys().collect::<Vec<_>>()
        );
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
            client_channels: Arc::clone(&self.client_channels),
            event_tx: self.event_tx.clone(),
            next_client_id: Arc::clone(&self.next_client_id),
        }
    }
}
