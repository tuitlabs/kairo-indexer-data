use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};
use crate::solana::error::{SolanaError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum BanterMarketStatus {
    Active = 0,
    Expired = 1,
    Settled = 2,
}

impl BanterMarketStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            BanterMarketStatus::Active => "ACTIVE",
            BanterMarketStatus::Expired => "EXPIRED",
            BanterMarketStatus::Settled => "SETTLED",
        }
    }

    pub fn from_u8(val: u8) -> Self {
        match val {
            0 => BanterMarketStatus::Active,
            1 => BanterMarketStatus::Expired,
            2 => BanterMarketStatus::Settled,
            _ => BanterMarketStatus::Active,
        }
    }
}

/// Anchor Account representation for `kairo_banter`::BanterMarket PDA.
/// Seeds: `[b"banter_market", curator.key().as_ref(), market_id.as_ref()]`
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BanterMarket {
    pub curator: [u8; 32],
    pub market_id: [u8; 32],
    pub stance_uri: String,
    pub status: BanterMarketStatus,
    pub total_raw_capital: u64,
    pub total_effective_stake: u64,
    pub created_at_slot: u64,
    pub created_at_timestamp: i64,
    pub bump: u8,
}

impl BanterMarket {
    /// Compute Anchor 8-byte account discriminator: sha256("account:BanterMarket")[0..8]
    pub fn discriminator() -> [u8; 8] {
        let mut hasher = Sha256::new();
        hasher.update(b"account:BanterMarket");
        let hash = hasher.finalize();
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&hash[..8]);
        disc
    }

    /// Deserialize from raw Solana account data (discriminator + Borsh payload)
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < 8 {
            return Err(SolanaError::Borsh(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Account data shorter than 8-byte Anchor discriminator",
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
        let account = Self::deserialize_reader(&mut slice)?;
        Ok(account)
    }

    /// Serialize into Anchor account format (discriminator + Borsh payload)
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&Self::discriminator());
        self.serialize(&mut buf)?;
        Ok(buf)
    }

    pub fn curator_base58(&self) -> String {
        bs58::encode(&self.curator).into_string()
    }

    pub fn market_id_hex(&self) -> String {
        format!("0x{}", hex::encode(self.market_id))
    }

    /// Derive expected BanterMarket PDA address
    /// Seeds: `[b"banter_market", curator, market_id]`
    pub fn derive_pda_address(
        program_id: &[u8; 32],
        curator: &[u8; 32],
        market_id: &[u8; 32],
        bump: u8,
    ) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"banter_market");
        hasher.update(curator);
        hasher.update(market_id);
        hasher.update(&[bump]);
        hasher.update(program_id);
        hasher.update(b"ProgramDerivedAddress");
        let hash = hasher.finalize();
        let mut pda = [0u8; 32];
        pda.copy_from_slice(&hash);
        pda
    }

    /// Verify this account's bump and keys against the provided pubkey and program ID
    pub fn verify_pda(&self, expected_account_pubkey: &[u8; 32], program_id: &[u8; 32]) -> Result<()> {
        let derived = Self::derive_pda_address(program_id, &self.curator, &self.market_id, self.bump);
        if &derived != expected_account_pubkey {
            return Err(SolanaError::PdaMismatch {
                expected: bs58::encode(expected_account_pubkey).into_string(),
                actual: bs58::encode(derived).into_string(),
            });
        }
        Ok(())
    }
}
