use thiserror::Error;

#[derive(Error, Debug)]
pub enum IndexerError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Event decode error: {0}")]
    Decode(String),

    #[error("RPC/WebSocket error: {0}")]
    Network(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, IndexerError>;
