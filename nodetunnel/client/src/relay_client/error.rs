use crate::transport::error::TransportError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RelayClientError {
    #[error("Transport not initialized")]
    TransportNotInitialized,

    #[error("Failed to send packet: {0}")]
    SendPacketError(#[from] TransportError),

    #[error("Invalid packet type")]
    InvalidPacketType,

    #[error("Packet parsing error")]
    PacketParsingError,
}
