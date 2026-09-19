use thiserror::Error;
use crate::error::IndexerError;

#[derive(Error, Debug)]
pub enum SolanaError {
    #[error("Anchor discriminator mismatch: expected {expected}, got {actual}")]
    DiscriminatorMismatch {
        expected: String,
        actual: String,
    },

    #[error("Borsh deserialization error: {0}")]
    Borsh(#[from] std::io::Error),

    #[error("PDA derivation mismatch: expected {expected}, got {actual}")]
    PdaMismatch {
        expected: String,
        actual: String,
    },

    #[error("Base58 decode error: {0}")]
    Base58(#[from] bs58::decode::Error),

    #[error("Base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),

    #[error("Invalid ring tier byte: {0}")]
    InvalidTier(u8),

    #[error("Invalid banter market status byte: {0}")]
    InvalidMarketStatus(u8),

    #[error("Missing expected program ID for PDA verification ({0})")]
    MissingProgramId(&'static str),

    #[error("Invalid public key length for {address}: expected 32 bytes, got {length}")]
    InvalidPubkeyLength {
        address: String,
        length: usize,
    },

    #[error("Solana event not found in logs: {0}")]
    EventNotFound(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Graph database error: {0}")]
    Graph(String),

    #[error("Geyser streaming error: {0}")]
    Geyser(String),

    #[error("Solana RPC error: {0}")]
    Rpc(String),

    #[error("Configuration error: {0}")]
    Config(String),
}

impl From<SolanaError> for IndexerError {
    fn from(err: SolanaError) -> Self {
        match err {
            SolanaError::Database(msg) => IndexerError::Database(msg),
            SolanaError::Geyser(msg) => IndexerError::Network(msg),
            SolanaError::Config(msg) => IndexerError::Config(msg),
            other => IndexerError::Decode(other.to_string()),
        }
    }
}

pub type Result<T> = std::result::Result<T, SolanaError>;
