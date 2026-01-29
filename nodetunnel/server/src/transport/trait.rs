use crate::udp::common::TransferChannel;
use std::io::Error;

/// Trait for transport implementations (UDP, WebRTC, etc.)
/// Note: Some transports (like WebRTC) may be wrapped in Arc and use &self,
/// while others (like UDP) may need &mut self. Implementations handle this.
pub trait Transport: Send + Sync {
    /// Send data to a client
    /// For UDP: requires &mut self
    /// For WebRTC: uses &self (handled via Arc internally)
    async fn send(
        &mut self,
        client_id: u64,
        data: Vec<u8>,
        channel: TransferChannel,
    ) -> Result<(), Error>;

    /// Remove a client from the transport
    fn remove_client(&mut self, client_id: &u64);
}
