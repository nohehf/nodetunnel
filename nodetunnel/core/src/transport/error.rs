use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("Failed to bind UDP socket: {0}")]
    BindError(std::io::Error),

    #[error("Failed to send packet: {0}")]
    SendError(std::io::Error),

    #[error("Failed to recv packet: {0}")]
    RecvError(std::io::Error),

    #[error("Clock may have gone backwards: {0}")]
    ClockError(#[from] std::time::SystemTimeError),

    #[error("Failed to create Netcode server udp: {0}")]
    NetcodeCreationFailed(std::io::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

// Manual From impls for types that don't implement std::error::Error
impl From<String> for TransportError {
    fn from(err: String) -> Self {
        TransportError::Other(err)
    }
}

impl From<&str> for TransportError {
    fn from(err: &str) -> Self {
        TransportError::Other(err.to_string())
    }
}
