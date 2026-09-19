use crate::solana::error::{SolanaError, Result};

/// Canonical representation of a Solana program ID, containing both
/// its Base58 string representation and its 32-byte public key array.
/// Ensures zero drift between pipeline string filtering and processor byte verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolanaProgramId {
    base58: String,
    bytes: [u8; 32],
}

impl SolanaProgramId {
    pub fn from_base58(s: &str) -> Result<Self> {
        let decoded = bs58::decode(s).into_vec().map_err(SolanaError::Base58)?;
        if decoded.len() != 32 {
            return Err(SolanaError::InvalidPubkeyLength {
                address: s.to_string(),
                length: decoded.len(),
            });
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&decoded);
        Ok(Self {
            base58: s.to_string(),
            bytes,
        })
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        let base58 = bs58::encode(&bytes).into_string();
        Self { base58, bytes }
    }

    pub fn as_str(&self) -> &str {
        &self.base58
    }

    pub fn bytes(&self) -> &[u8; 32] {
        &self.bytes
    }
}

/// Unified, single source of truth for protocol program IDs on Solana.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SolanaProgramsConfig {
    pub kairo_social: Option<SolanaProgramId>,
    pub kairo_banter: Option<SolanaProgramId>,
}

impl SolanaProgramsConfig {
    pub fn new(
        kairo_social: Option<&str>,
        kairo_banter: Option<&str>,
    ) -> Result<Self> {
        let social = match kairo_social {
            Some(s) if !s.trim().is_empty() => Some(SolanaProgramId::from_base58(s.trim())?),
            _ => None,
        };
        let banter = match kairo_banter {
            Some(s) if !s.trim().is_empty() => Some(SolanaProgramId::from_base58(s.trim())?),
            _ => None,
        };
        Ok(Self {
            kairo_social: social,
            kairo_banter: banter,
        })
    }
}
