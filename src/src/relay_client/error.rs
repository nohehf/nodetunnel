use crate::transport::error::TransportError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RelayClientError {
    #[error("Transport not initialized")]
    TransportNotInitialized,

    #[error("Transport error: {0}")]
    TransportError(#[from] TransportError),

    #[error("Invalid packet type")]
    InvalidPacketType,

    #[error("Packet parsing error")]
    PacketParsingError,
}
