use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};
use crate::solana::error::{SolanaError, Result};
use crate::config::{
    PROVISIONAL_INNER_CIRCLE_MIN_WEIGHT,
    PROVISIONAL_SOCIAL_RING_MIN_WEIGHT,
    PROVISIONAL_PUBLIC_OUTER_MIN_WEIGHT,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum RingTier {
    Public = 0,
    SocialRing = 1,
    InnerCircle = 2,
}

impl RingTier {
    pub fn from_u8(val: u8) -> Result<Self> {
        match val {
            0 => Ok(RingTier::Public),
            1 => Ok(RingTier::SocialRing),
            2 => Ok(RingTier::InnerCircle),
            other => Err(SolanaError::InvalidTier(other)),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            RingTier::Public => "PUBLIC",
            RingTier::SocialRing => "SOCIAL_RING",
            RingTier::InnerCircle => "INNER_CIRCLE",
        }
    }
}

/// Anchor Account representation for `kairo_social`::SocialEdge PDA.
/// Seeds: `[b"social_edge", authority.key().as_ref(), peer.key().as_ref()]`
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SocialEdge {
    pub authority: [u8; 32],
    pub peer: [u8; 32],
    pub ring_tier: RingTier,
    pub weight_bps: u16,
    pub interaction_count: u64,
    pub last_updated_slot: u64,
    pub bump: u8,
}

impl SocialEdge {
    /// Compute Anchor 8-byte account discriminator: sha256("account:SocialEdge")[0..8]
    pub fn discriminator() -> [u8; 8] {
        let mut hasher = Sha256::new();
        hasher.update(b"account:SocialEdge");
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
        let mut buf = Vec::with_capacity(8 + 32 + 32 + 1 + 2 + 8 + 8 + 1);
        buf.extend_from_slice(&Self::discriminator());
        self.serialize(&mut buf)?;
        Ok(buf)
    }

    pub fn authority_base58(&self) -> String {
        bs58::encode(&self.authority).into_string()
    }

    pub fn peer_base58(&self) -> String {
        bs58::encode(&self.peer).into_string()
    }

    /// Derive the expected PDA address given seeds, bump, and program ID
    /// Solana PDA hash formula: SHA-256(seed_1 || seed_2 || ... || bump || program_id || "ProgramDerivedAddress")
    pub fn derive_pda_address(
        program_id: &[u8; 32],
        authority: &[u8; 32],
        peer: &[u8; 32],
        bump: u8,
    ) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"social_edge");
        hasher.update(authority);
        hasher.update(peer);
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
        let derived = Self::derive_pda_address(program_id, &self.authority, &self.peer, self.bump);
        if &derived != expected_account_pubkey {
            return Err(SolanaError::PdaMismatch {
                expected: bs58::encode(expected_account_pubkey).into_string(),
                actual: bs58::encode(derived).into_string(),
            });
        }
        Ok(())
    }

    /// Compute provisional clamped weight based on ring tier and weight_bps (0..=10000).
    ///
    /// Provisional governance defaults (unvalidated):
    /// - INNER_CIRCLE: Clamped to [0.90, 1.00] using 0.90 + 0.10 * (weight_bps / 10000.0)
    /// - SOCIAL_RING: Clamped to [0.30, 0.50) using 0.30 + 0.20 * (weight_bps / 10000.0)
    /// - PUBLIC: 0.00 (implicit network baseline, not materialized as RING_EDGE in Neo4j)
    pub fn clamped_weight(&self) -> f64 {
        let bps_ratio = (self.weight_bps as f64) / 10000.0;
        let clamped_ratio = bps_ratio.clamp(0.0, 1.0);

        match self.ring_tier {
            RingTier::InnerCircle => {
                let range = 1.00 - PROVISIONAL_INNER_CIRCLE_MIN_WEIGHT;
                (PROVISIONAL_INNER_CIRCLE_MIN_WEIGHT + range * clamped_ratio).clamp(0.90, 1.00)
            }
            RingTier::SocialRing => {
                let range = 0.50 - PROVISIONAL_SOCIAL_RING_MIN_WEIGHT;
                (PROVISIONAL_SOCIAL_RING_MIN_WEIGHT + range * clamped_ratio).clamp(0.30, 0.499999)
            }
            RingTier::Public => PROVISIONAL_PUBLIC_OUTER_MIN_WEIGHT,
        }
    }

    /// Should this edge be materialized into Neo4j as an explicit `:RING_EDGE`?
    /// Returns false for Public tier (implicit baseline).
    pub fn should_materialize_in_graph(&self) -> bool {
        match self.ring_tier {
            RingTier::InnerCircle | RingTier::SocialRing => true,
            RingTier::Public => false,
        }
    }
}
