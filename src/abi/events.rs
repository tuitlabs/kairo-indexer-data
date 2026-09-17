use tiny_keccak::{Hasher, Keccak};
use ethnum::U256;
use serde::{Deserialize, Serialize};
use crate::error::{IndexerError, Result};

pub fn keccak256(input: &[u8]) -> [u8; 32] {
    let mut output = [0u8; 32];
    let mut hasher = Keccak::v256();
    hasher.update(input);
    hasher.finalize(&mut output);
    output
}

pub fn to_hex_topic(bytes: &[u8; 32]) -> String {
    format!("0x{}", hex::encode(bytes))
}

// Event Signatures & Topics
lazy_static_topics! {
    MARKET_CREATED_TOPIC: "MarketCreated(bytes32,string,address,address,uint256)",
    STANCE_BOUGHT_TOPIC: "StanceBought(bytes32,address,uint256,uint256,uint256,uint256,uint256,uint256)",
    STANCE_SOLD_TOPIC: "StanceSold(bytes32,address,uint256,uint256,uint256,uint256)",
    DISCLOSURE_ENFORCED_TOPIC: "DisclosureEnforced(bytes32,address,uint256,uint256)",
    EPOCH_FINALIZED_TOPIC: "EpochFinalized(bytes32,uint256,uint256,uint256,uint256,uint256)",
    YIELD_DISTRIBUTED_TOPIC: "YieldDistributed(bytes32,uint256,uint256,uint256,uint256,uint256,uint256,uint256)",
    COALITION_FORMED_TOPIC: "CoalitionFormed(bytes32,uint256[],uint16,uint256)",
    COALITION_MEMBER_JOINED_TOPIC: "CoalitionMemberJoined(bytes32,address,uint256,uint256,uint256)",
    DECEPTION_TAX_CHARGED_TOPIC: "DeceptionTaxCharged(bytes32,address,uint256,uint256,uint256,uint256)"
}

macro_rules! lazy_static_topics {
    ($($name:ident : $sig:expr),* $(,)?) => {
        $(
            #[allow(non_snake_case)]
            pub fn $name() -> &'static str {
                // Return cached hex string of Keccak-256
                static TOPIC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
                TOPIC.get_or_init(|| {
                    let hash = keccak256($sig.as_bytes());
                    format!("0x{}", hex::encode(hash))
                })
            }
        )*
    };
}
pub(crate) use lazy_static_topics;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketCreatedEvent {
    pub market_id: String,
    pub stance_uri: String,
    pub pool_address: String,
    pub curator: String,
    pub created_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StanceBoughtEvent {
    pub market_id: String,
    pub agent: String,
    pub deposit_amt: U256,
    pub tokens_received: U256,
    pub effective_stake: U256,
    pub credibility_score: u32,
    pub current_price: U256,
    pub timestamp: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StanceSoldEvent {
    pub market_id: String,
    pub agent: String,
    pub token_amt: U256,
    pub collateral_returned: U256,
    pub exit_price: U256,
    pub timestamp: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisclosureEnforcedEvent {
    pub market_id: String,
    pub coalition_id: String,
    pub supply_share_bps: u32,
    pub timestamp: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EpochFinalizedEvent {
    pub market_id: String,
    pub epoch_id: u64,
    pub consensus_ratio: U256,
    pub total_effective_stake: U256,
    pub total_raw_capital: U256,
    pub timestamp: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct YieldDistributedEvent {
    pub market_id: String,
    pub epoch_id: u64,
    pub total_yield: U256,
    pub agent_wallet_share: U256,
    pub agent_owner_share: U256,
    pub coalition_share: U256,
    pub treasury_share: U256,
    pub timestamp: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoalitionFormedEvent {
    pub coalition_id: String,
    pub agent_ids: Vec<u64>,
    pub yield_bps: u16,
    pub timestamp: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoalitionMemberJoinedEvent {
    pub coalition_id: String,
    pub agent_address: String,
    pub agent_id: u64,
    pub share_bps: u32,
    pub timestamp: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeceptionTaxChargedEvent {
    pub market_id: String,
    pub agent: String,
    pub tax_amount: U256,
    pub flip_velocity: u32,
    pub credibility_score: u32,
    pub timestamp: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ParsedKairoEvent {
    MarketCreated(MarketCreatedEvent),
    StanceBought(StanceBoughtEvent),
    StanceSold(StanceSoldEvent),
    DisclosureEnforced(DisclosureEnforcedEvent),
    EpochFinalized(EpochFinalizedEvent),
    YieldDistributed(YieldDistributedEvent),
    CoalitionFormed(CoalitionFormedEvent),
    CoalitionMemberJoined(CoalitionMemberJoinedEvent),
    DeceptionTaxCharged(DeceptionTaxChargedEvent),
}

// Helpers for decoding standard EVM ABI 32-byte chunks
pub fn parse_u256_slice(chunk: &[u8]) -> U256 {
    let mut arr = [0u8; 32];
    if chunk.len() >= 32 {
        arr.copy_from_slice(&chunk[..32]);
    } else {
        arr[32 - chunk.len()..].copy_from_slice(chunk);
    }
    U256::from_be_bytes(arr)
}

pub fn parse_address_slice(chunk: &[u8]) -> String {
    // Address is in the last 20 bytes of the 32-byte word
    let slice = if chunk.len() >= 32 {
        &chunk[12..32]
    } else if chunk.len() >= 20 {
        &chunk[chunk.len() - 20..]
    } else {
        chunk
    };
    format!("0x{}", hex::encode(slice))
}

pub fn parse_bytes32_slice(chunk: &[u8]) -> String {
    format!("0x{}", hex::encode(&chunk[..std::cmp::min(chunk.len(), 32)]))
}

pub fn strip_0x(s: &str) -> &str {
    s.strip_prefix("0x").unwrap_or(s)
}

impl ParsedKairoEvent {
    pub fn decode(topics: &[String], data_hex: &str) -> Result<Option<Self>> {
        if topics.is_empty() {
            return Ok(None);
        }

        let primary_topic = topics[0].to_lowercase();
        let data = hex::decode(strip_0x(data_hex))
            .map_err(|e| IndexerError::Decode(format!("Invalid data hex: {}", e)))?;

        if primary_topic == MARKET_CREATED_TOPIC() {
            // event MarketCreated(bytes32 indexed marketId, string stanceUri, address indexed poolAddress, address indexed curator, uint256 createdAt)
            // topics[1] = marketId
            // topics[2] = poolAddress
            // topics[3] = curator
            // data contains: offset to string stanceUri, createdAt
            if topics.len() < 4 {
                return Err(IndexerError::Decode("MarketCreated missing indexed topics".into()));
            }
            let market_id = topics[1].clone();
            let pool_address = format!("0x{}", &topics[2].to_lowercase()[topics[2].len().saturating_sub(40)..]);
            let curator = format!("0x{}", &topics[3].to_lowercase()[topics[3].len().saturating_sub(40)..]);

            // Decode dynamic string stanceUri and createdAt from data
            let mut stance_uri = String::new();
            let mut created_at = 0u64;

            if data.len() >= 64 {
                let offset = parse_u256_slice(&data[0..32]).as_usize();
                created_at = parse_u256_slice(&data[32..64]).as_u64();

                if offset < data.len() && offset + 32 <= data.len() {
                    let str_len = parse_u256_slice(&data[offset..offset + 32]).as_usize();
                    let str_start = offset + 32;
                    let str_end = std::cmp::min(data.len(), str_start + str_len);
                    stance_uri = String::from_utf8_lossy(&data[str_start..str_end]).to_string();
                }
            }

            return Ok(Some(Self::MarketCreated(MarketCreatedEvent {
                market_id,
                stance_uri,
                pool_address,
                curator,
                created_at,
            })));
        }

        if primary_topic == STANCE_BOUGHT_TOPIC() {
            // event StanceBought(bytes32 indexed marketId, address indexed agent, uint256 depositAmt, uint256 tokensReceived, uint256 effectiveStake, uint256 credibilityScore, uint256 currentPrice, uint256 timestamp)
            // topics[1] = marketId
            // topics[2] = agent
            if topics.len() < 3 {
                return Err(IndexerError::Decode("StanceBought missing indexed topics".into()));
            }
            let market_id = topics[1].clone();
            let agent = format!("0x{}", &topics[2].to_lowercase()[topics[2].len().saturating_sub(40)..]);

            if data.len() < 192 {
                return Err(IndexerError::Decode("StanceBought data payload too short".into()));
            }

            let deposit_amt = parse_u256_slice(&data[0..32]);
            let tokens_received = parse_u256_slice(&data[32..64]);
            let effective_stake = parse_u256_slice(&data[64..96]);
            let credibility_score = parse_u256_slice(&data[96..128]).as_u32();
            let current_price = parse_u256_slice(&data[128..160]);
            let timestamp = parse_u256_slice(&data[160..192]).as_u64();

            return Ok(Some(Self::StanceBought(StanceBoughtEvent {
                market_id,
                agent,
                deposit_amt,
                tokens_received,
                effective_stake,
                credibility_score,
                current_price,
                timestamp,
            })));
        }

        if primary_topic == STANCE_SOLD_TOPIC() {
            // event StanceSold(bytes32 indexed marketId, address indexed agent, uint256 tokenAmt, uint256 collateralReturned, uint256 exitPrice, uint256 timestamp)
            if topics.len() < 3 {
                return Err(IndexerError::Decode("StanceSold missing indexed topics".into()));
            }
            let market_id = topics[1].clone();
            let agent = format!("0x{}", &topics[2].to_lowercase()[topics[2].len().saturating_sub(40)..]);

            if data.len() < 128 {
                return Err(IndexerError::Decode("StanceSold data payload too short".into()));
            }

            let token_amt = parse_u256_slice(&data[0..32]);
            let collateral_returned = parse_u256_slice(&data[32..64]);
            let exit_price = parse_u256_slice(&data[64..96]);
            let timestamp = parse_u256_slice(&data[96..128]).as_u64();

            return Ok(Some(Self::StanceSold(StanceSoldEvent {
                market_id,
                agent,
                token_amt,
                collateral_returned,
                exit_price,
                timestamp,
            })));
        }

        if primary_topic == DISCLOSURE_ENFORCED_TOPIC() {
            // event DisclosureEnforced(bytes32 indexed marketId, address indexed coalitionId, uint256 supplyShareBps, uint256 timestamp)
            if topics.len() < 3 {
                return Err(IndexerError::Decode("DisclosureEnforced missing indexed topics".into()));
            }
            let market_id = topics[1].clone();
            let coalition_id = format!("0x{}", &topics[2].to_lowercase()[topics[2].len().saturating_sub(40)..]);

            let supply_share_bps = if data.len() >= 32 { parse_u256_slice(&data[0..32]).as_u32() } else { 0 };
            let timestamp = if data.len() >= 64 { parse_u256_slice(&data[32..64]).as_u64() } else { 0 };

            return Ok(Some(Self::DisclosureEnforced(DisclosureEnforcedEvent {
                market_id,
                coalition_id,
                supply_share_bps,
                timestamp,
            })));
        }

        if primary_topic == EPOCH_FINALIZED_TOPIC() {
            // event EpochFinalized(bytes32 indexed marketId, uint256 indexed epochId, uint256 consensusRatio, uint256 totalEffectiveStake, uint256 totalRawCapital, uint256 timestamp)
            if topics.len() < 3 {
                return Err(IndexerError::Decode("EpochFinalized missing indexed topics".into()));
            }
            let market_id = topics[1].clone();
            let epoch_id = {
                let bytes = hex::decode(strip_0x(&topics[2])).unwrap_or_default();
                parse_u256_slice(&bytes).as_u64()
            };

            if data.len() < 128 {
                return Err(IndexerError::Decode("EpochFinalized data payload too short".into()));
            }

            let consensus_ratio = parse_u256_slice(&data[0..32]);
            let total_effective_stake = parse_u256_slice(&data[32..64]);
            let total_raw_capital = parse_u256_slice(&data[64..96]);
            let timestamp = parse_u256_slice(&data[96..128]).as_u64();

            return Ok(Some(Self::EpochFinalized(EpochFinalizedEvent {
                market_id,
                epoch_id,
                consensus_ratio,
                total_effective_stake,
                total_raw_capital,
                timestamp,
            })));
        }

        if primary_topic == YIELD_DISTRIBUTED_TOPIC() {
            // event YieldDistributed(bytes32 indexed marketId, uint256 indexed epochId, uint256 totalYield, uint256 agentWalletShare, uint256 agentOwnerShare, uint256 coalitionShare, uint256 protocolTreasuryShare, uint256 timestamp)
            if topics.len() < 3 {
                return Err(IndexerError::Decode("YieldDistributed missing indexed topics".into()));
            }
            let market_id = topics[1].clone();
            let epoch_id = {
                let bytes = hex::decode(strip_0x(&topics[2])).unwrap_or_default();
                parse_u256_slice(&bytes).as_u64()
            };

            if data.len() < 192 {
                return Err(IndexerError::Decode("YieldDistributed data payload too short".into()));
            }

            let total_yield = parse_u256_slice(&data[0..32]);
            let agent_wallet_share = parse_u256_slice(&data[32..64]);
            let agent_owner_share = parse_u256_slice(&data[64..96]);
            let coalition_share = parse_u256_slice(&data[96..128]);
            let treasury_share = parse_u256_slice(&data[128..160]);
            let timestamp = parse_u256_slice(&data[160..192]).as_u64();

            return Ok(Some(Self::YieldDistributed(YieldDistributedEvent {
                market_id,
                epoch_id,
                total_yield,
                agent_wallet_share,
                agent_owner_share,
                coalition_share,
                treasury_share,
                timestamp,
            })));
        }

        if primary_topic == COALITION_FORMED_TOPIC() {
            // event CoalitionFormed(bytes32 indexed coalitionId, uint256[] agentIds, uint16 yieldBps, uint256 timestamp)
            if topics.len() < 2 {
                return Err(IndexerError::Decode("CoalitionFormed missing indexed topic".into()));
            }
            let coalition_id = topics[1].clone();

            let mut agent_ids = Vec::new();
            let mut yield_bps = 0u16;
            let mut timestamp = 0u64;

            if data.len() >= 96 {
                let offset = parse_u256_slice(&data[0..32]).as_usize();
                yield_bps = parse_u256_slice(&data[32..64]).as_u16();
                timestamp = parse_u256_slice(&data[64..96]).as_u64();

                if offset < data.len() && offset + 32 <= data.len() {
                    let len = parse_u256_slice(&data[offset..offset + 32]).as_usize();
                    let mut cur = offset + 32;
                    for _ in 0..len {
                        if cur + 32 <= data.len() {
                            agent_ids.push(parse_u256_slice(&data[cur..cur + 32]).as_u64());
                            cur += 32;
                        }
                    }
                }
            }

            return Ok(Some(Self::CoalitionFormed(CoalitionFormedEvent {
                coalition_id,
                agent_ids,
                yield_bps,
                timestamp,
            })));
        }

        if primary_topic == COALITION_MEMBER_JOINED_TOPIC() {
            // event CoalitionMemberJoined(bytes32 indexed coalitionId, address indexed agentAddress, uint256 indexed agentId, uint256 shareBps, uint256 timestamp)
            if topics.len() < 4 {
                return Err(IndexerError::Decode("CoalitionMemberJoined missing indexed topics".into()));
            }
            let coalition_id = topics[1].clone();
            let agent_address = format!("0x{}", &topics[2].to_lowercase()[topics[2].len().saturating_sub(40)..]);
            let agent_id = {
                let bytes = hex::decode(strip_0x(&topics[3])).unwrap_or_default();
                parse_u256_slice(&bytes).as_u64()
            };

            let share_bps = if data.len() >= 32 { parse_u256_slice(&data[0..32]).as_u32() } else { 0 };
            let timestamp = if data.len() >= 64 { parse_u256_slice(&data[32..64]).as_u64() } else { 0 };

            return Ok(Some(Self::CoalitionMemberJoined(CoalitionMemberJoinedEvent {
                coalition_id,
                agent_address,
                agent_id,
                share_bps,
                timestamp,
            })));
        }

        if primary_topic == DECEPTION_TAX_CHARGED_TOPIC() {
            // event DeceptionTaxCharged(bytes32 indexed marketId, address indexed agent, uint256 taxAmount, uint256 flipVelocity, uint256 credibilityScore, uint256 timestamp)
            if topics.len() < 3 {
                return Err(IndexerError::Decode("DeceptionTaxCharged missing indexed topics".into()));
            }
            let market_id = topics[1].clone();
            let agent = format!("0x{}", &topics[2].to_lowercase()[topics[2].len().saturating_sub(40)..]);

            if data.len() < 128 {
                return Err(IndexerError::Decode("DeceptionTaxCharged data payload too short".into()));
            }

            let tax_amount = parse_u256_slice(&data[0..32]);
            let flip_velocity = parse_u256_slice(&data[32..64]).as_u32();
            let credibility_score = parse_u256_slice(&data[64..96]).as_u32();
            let timestamp = parse_u256_slice(&data[96..128]).as_u64();

            return Ok(Some(Self::DeceptionTaxCharged(DeceptionTaxChargedEvent {
                market_id,
                agent,
                tax_amount,
                flip_velocity,
                credibility_score,
                timestamp,
            })));
        }

        Ok(None)
    }
}
