use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TLS error: {0}")]
    Tls(#[from] rustls::Error),

    #[error("Invalid host: {0}")]
    InvalidHost(String),

    #[error("Protocol framing error: {0}")]
    Protocol(String),

    #[error("Connection closed by peer")]
    ConnectionClosed,

    #[error("Crypto error: {0}")]
    Crypto(String),

    #[error("Frame too large: {0} bytes (max {1})")]
    FrameTooLarge(usize, usize),

    #[error("Replay detected: counter {0} already seen")]
    ReplayDetected(u64),

    #[error("Rekey needed: counter {0} reached threshold {1}")]
    RekeyNeeded(u64, u64),

    #[error("Authentication failed")]
    AuthFailed,

    #[error("Unknown key id: {0}")]
    UnknownKeyId(u8),

    #[error("Session error: {0}")]
    Session(String),
}
