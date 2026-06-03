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
}
