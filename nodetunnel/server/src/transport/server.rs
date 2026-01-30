use crate::transport::r#trait::Transport;
use crate::udp::common::TransferChannel;
use crate::udp::paper_interface::PaperInterface;
use crate::webrtc::webrtc_interface::WebRTCInterface;
use std::sync::Arc;

/// Transport instance for a client
/// Uses a trait object to support multiple transport types
pub enum ClientTransport {
    Udp,
    WebRTC(Arc<WebRTCInterface>),
}

/// Transport registry - maps client IDs to their transport instances
pub struct TransportRegistry {
    transports: std::collections::HashMap<u64, ClientTransport>,
    udp: PaperInterface,
}

impl TransportRegistry {
    pub fn new(udp: PaperInterface) -> Self {
        Self {
            transports: std::collections::HashMap::new(),
            udp,
        }
    }

    /// Register a UDP client's transport
    pub fn register_udp_client(&mut self, client_id: u64) {
        self.transports.insert(client_id, ClientTransport::Udp);
    }

    /// Register a WebRTC client's transport
    pub fn register_webrtc_client(&mut self, client_id: u64, transport: Arc<WebRTCInterface>) {
        self.transports
            .insert(client_id, ClientTransport::WebRTC(transport));
    }

    /// Send to a client using their registered transport
    pub async fn send(
        &mut self,
        client_id: u64,
        data: Vec<u8>,
        channel: TransferChannel,
    ) -> Result<(), std::io::Error> {
        use tracing::{debug, warn};
        
        debug!(
            "TransportRegistry::send() called for client_id={}, data_size={} bytes, registered_transports={}",
            client_id,
            data.len(),
            self.transports.len()
        );
        
        match self.transports.get(&client_id) {
            Some(ClientTransport::Udp) => {
                debug!("Routing packet to UDP transport for client {}", client_id);
                Transport::send(&mut self.udp, client_id, data, channel).await
            }
            Some(ClientTransport::WebRTC(webrtc)) => {
                debug!("Routing packet to WebRTC transport for client {}", client_id);
                // WebRTC is wrapped in Arc, so we can't get &mut self
                // Since WebRTC's send implementation uses &self internally (via Arc),
                // we call it directly. The trait is still implemented for consistency.
                // Note: This is a limitation of Arc - we can't use the trait here directly.
                let webrtc_clone = Arc::clone(webrtc);
                async move {
                    // Create a temporary owned value to satisfy &mut self requirement
                    // This is safe because WebRTC's send doesn't actually mutate
                    let mut owned = (*webrtc_clone).clone();
                    Transport::send(&mut owned, client_id, data, channel).await
                }
                .await
            }
            None => {
                warn!(
                    "Client {} not found in transport registry, defaulting to UDP. Registered clients: {:?}",
                    client_id,
                    self.transports.keys().collect::<Vec<_>>()
                );
                // Default to UDP if not registered (backward compatibility)
                Transport::send(&mut self.udp, client_id, data, channel).await
            }
        }
    }

    /// Remove a client's transport
    pub fn remove_client(&mut self, client_id: &u64) {
        use tracing::info;
        info!("TransportRegistry::remove_client() called for client_id={}", client_id);
        match self.transports.remove(client_id) {
            Some(ClientTransport::WebRTC(webrtc)) => {
                info!("Removing WebRTC client {}", client_id);
                // WebRTC uses Arc, so we call the method directly
                webrtc.remove_client(client_id);
            }
            Some(ClientTransport::Udp) => {
                info!("Removing UDP client {}", client_id);
                // UDP removal handled by interface
                Transport::remove_client(&mut self.udp, client_id);
            }
            None => {
                info!("Client {} not found in transport registry (already removed?)", client_id);
            }
        }
        info!("TransportRegistry now has {} registered clients", self.transports.len());
    }

    /// Get mutable reference to UDP interface (for UDP-specific operations)
    pub fn udp_mut(&mut self) -> &mut PaperInterface {
        &mut self.udp
    }

    /// Check if a client is registered in the transport registry
    pub fn is_client_registered(&self, client_id: &u64) -> bool {
        self.transports.contains_key(client_id)
    }

    /// Get WebRTC interface (for WebRTC-specific operations)
    pub fn webrtc(&self) -> Option<&Arc<WebRTCInterface>> {
        // Return first WebRTC transport found (for signaling server)
        self.transports.values().find_map(|t| match t {
            ClientTransport::WebRTC(w) => Some(w),
            _ => None,
        })
    }
}
