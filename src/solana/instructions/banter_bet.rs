use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use crate::solana::error::{SolanaError, Result};

/// Decoded Anchor event emitted by `kairo_banter` on trade execution.
/// Emitted via Anchor's `emit!(BanterBetPlaced { ... })` into Solana transaction log as:
/// `Program data: <base64>`
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BanterBetPlaced {
    pub market_id: [u8; 32],
    pub agent: [u8; 32],
    pub trade_type: u8, // 0 = BUY, 1 = SELL
    pub raw_capital: u64,
    pub token_amount: u64,
    pub effective_stake: u64,
    pub credibility_score: u16, // Scaled 100-1000, genuine score emitted by on-chain program
    pub execution_price: u64,
    pub friction_tax: u64,
}

impl BanterBetPlaced {
    /// Compute Anchor 8-byte event discriminator: sha256("event:BanterBetPlaced")[0..8]
    pub fn discriminator() -> [u8; 8] {
        let mut hasher = Sha256::new();
        hasher.update(b"event:BanterBetPlaced");
        let hash = hasher.finalize();
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&hash[..8]);
        disc
    }

    /// Deserialize event from raw bytes (8-byte discriminator + Borsh payload)
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < 8 {
            return Err(SolanaError::Borsh(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Event data shorter than 8-byte Anchor discriminator",
            )));
        }

        let expected_disc = Self::discriminator();
        let actual_disc = &data[..8];

        if actual_disc != expected_disc {
            return Err(SolanaError::DiscriminatorMismatch {
                expected: hex::encode(expected_disc),
                actual: hex::encode(actual_disc),
            });
        }

        let mut slice = &data[8..];
        let event = Self::deserialize_reader(&mut slice)?;
        Ok(event)
    }

    /// Serialize event to raw bytes (8-byte discriminator + Borsh payload)
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&Self::discriminator());
        self.serialize(&mut buf)?;
        Ok(buf)
    }

    /// Format event to Anchor log string format: `Program data: <base64>`
    pub fn to_log_string(&self) -> Result<String> {
        let bytes = self.to_bytes()?;
        let b64 = BASE64_STANDARD.encode(&bytes);
        Ok(format!("Program data: {}", b64))
    }

    pub fn market_id_hex(&self) -> String {
        format!("0x{}", hex::encode(self.market_id))
    }

    pub fn agent_base58(&self) -> String {
        bs58::encode(&self.agent).into_string()
    }

    pub fn trade_type_str(&self) -> &'static str {
        if self.trade_type == 0 {
            "BUY"
        } else {
            "SELL"
        }
    }

    /// Parse a single log line if it contains an Anchor `Program data: ` emission.
    /// Returns `Ok(Some(event))` if it is a valid `BanterBetPlaced` event,
    /// `Ok(None)` if it is another event or non-event log,
    /// or `Err` if the base64 or Borsh parsing fails unexpectedly.
    pub fn from_log_line(line: &str) -> Result<Option<Self>> {
        let trimmed = line.trim();
        let prefix = "Program data: ";
        if !trimmed.starts_with(prefix) {
            return Ok(None);
        }

        let b64_payload = &trimmed[prefix.len()..];
        let decoded = BASE64_STANDARD.decode(b64_payload)?;

        if decoded.len() < 8 {
            return Ok(None);
        }

        let expected_disc = Self::discriminator();
        if decoded[..8] != expected_disc {
            // Different event emitted by Anchor
            return Ok(None);
        }

        let event = Self::from_bytes(&decoded)?;
        Ok(Some(event))
    }

    /// Extract all `BanterBetPlaced` events from a transaction's log messages
    pub fn extract_from_logs(logs: &[String]) -> Result<Vec<Self>> {
        let mut events = Vec::new();
        for line in logs {
            if let Some(event) = Self::from_log_line(line)? {
                events.push(event);
            }
        }
        Ok(events)
    }
}
