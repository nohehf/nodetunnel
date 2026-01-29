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
}
