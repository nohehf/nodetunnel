use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Clock may have gone backwards: {0}")]
    ClockError(#[from] std::time::SystemTimeError),
}
