use std::env;
use crate::error::Result;

// Provisional, governance-tunable graph weighting & topology constants (unvalidated defaults)
pub const PROVISIONAL_COALITION_BASE_WEIGHT: f64 = 0.50;
pub const PROVISIONAL_COALITION_MAX_WEIGHT: f64 = 0.90;
pub const PROVISIONAL_COALITION_SHARE_SCALE: f64 = 0.40;
pub const PROVISIONAL_INNER_CIRCLE_MIN_WEIGHT: f64 = 0.90;
pub const PROVISIONAL_SOCIAL_RING_MIN_WEIGHT: f64 = 0.30;
pub const PROVISIONAL_PUBLIC_OUTER_MIN_WEIGHT: f64 = 0.00;

// Provisional threshold to bound O(n^2) materialization (unvalidated default):
// Coalitions with <= 100 members materialize direct RING_EDGEs. Coalitions above
// this threshold fall back to query-time 2-hop evaluation in path-finding.
pub const PROVISIONAL_MAX_COALITION_MATERIALIZE_MEMBERS: usize = 100;
pub const MAX_COALITION_MATERIALIZE_MEMBERS: usize = PROVISIONAL_MAX_COALITION_MATERIALIZE_MEMBERS;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub ws_rpc_url: String,
    pub http_rpc_url: String,
    pub chain_id: u64,
    pub neo4j_uri: String,
    pub neo4j_user: String,
    pub neo4j_pass: String,
    pub neo4j_database: Option<String>,
    pub kairo_market_address: Option<String>,
    pub kairo_settlement_address: Option<String>,
    pub kairo_coalition_address: Option<String>,
    pub kairo_credibility_hook_address: Option<String>,
    pub start_block: u64,
    pub max_batch_size: u64,
    pub reconnect_base_delay_ms: u64,
    pub reconnect_max_delay_ms: u64,
    pub solana_rpc_url: String,
    pub solana_geyser_endpoint: String,
    pub solana_geyser_x_token: Option<String>,
    pub solana_programs: crate::solana::config::SolanaProgramsConfig,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();

        let database_url = env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://postgres@localhost:5432/kairo_indexer".to_string());

        let ws_rpc_url = env::var("BASE_WS_URL")
            .unwrap_or_else(|_| "wss://sepolia.base.org".to_string());

        let http_rpc_url = env::var("BASE_RPC_URL")
            .unwrap_or_else(|_| "https://sepolia.base.org".to_string());

        let chain_id = env::var("CHAIN_ID")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(84532); // Base Sepolia default, 8453 for Base Mainnet

        let neo4j_uri = env::var("NEO4J_URI")
            .unwrap_or_else(|_| "bolt://localhost:7687".to_string());

        let neo4j_user = env::var("NEO4J_USER")
            .unwrap_or_else(|_| "neo4j".to_string());

        let neo4j_pass = env::var("NEO4J_PASSWORD")
            .unwrap_or_else(|_| "password".to_string());

        let neo4j_database = env::var("NEO4J_DATABASE").ok();

        let start_block = env::var("START_BLOCK")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let max_batch_size = env::var("MAX_BATCH_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2000);

        let kairo_market_address = env::var("KAIRO_MARKET_ADDRESS")
            .ok()
            .map(|s| s.to_lowercase());

        let kairo_settlement_address = env::var("KAIRO_SETTLEMENT_ADDRESS")
            .ok()
            .map(|s| s.to_lowercase());

        let kairo_coalition_address = env::var("KAIRO_COALITION_ADDRESS")
            .ok()
            .map(|s| s.to_lowercase());

        let kairo_credibility_hook_address = env::var("KAIRO_CREDIBILITY_HOOK_ADDRESS")
            .ok()
            .map(|s| s.to_lowercase());

        let solana_rpc_url = env::var("SOLANA_RPC_URL")
            .unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());

        let solana_geyser_endpoint = env::var("SOLANA_GEYSER_ENDPOINT")
            .unwrap_or_else(|_| "http://127.0.0.1:10000".to_string());

        let solana_geyser_x_token = env::var("SOLANA_GEYSER_X_TOKEN").ok();

        let kairo_social_program_id = env::var("KAIRO_SOCIAL_PROGRAM_ID").ok();
        let kairo_banter_program_id = env::var("KAIRO_BANTER_PROGRAM_ID").ok();

        let solana_programs = crate::solana::config::SolanaProgramsConfig::new(
            kairo_social_program_id.as_deref(),
            kairo_banter_program_id.as_deref(),
        ).map_err(|e| crate::error::IndexerError::Config(e.to_string()))?;

        Ok(Self {
            database_url,
            ws_rpc_url,
            http_rpc_url,
            chain_id,
            neo4j_uri,
            neo4j_user,
            neo4j_pass,
            neo4j_database,
            kairo_market_address,
            kairo_settlement_address,
            kairo_coalition_address,
            kairo_credibility_hook_address,
            start_block,
            max_batch_size,
            reconnect_base_delay_ms: 1000,
            reconnect_max_delay_ms: 30000,
            solana_rpc_url,
            solana_geyser_endpoint,
            solana_geyser_x_token,
            solana_programs,
        })
    }

    /// List of target contract addresses to subscribe to (if configured)
    pub fn target_contracts(&self) -> Vec<String> {
        let mut addresses = Vec::new();
        if let Some(addr) = &self.kairo_market_address {
            addresses.push(addr.clone());
        }
        if let Some(addr) = &self.kairo_settlement_address {
            addresses.push(addr.clone());
        }
        if let Some(addr) = &self.kairo_coalition_address {
            addresses.push(addr.clone());
        }
        if let Some(addr) = &self.kairo_credibility_hook_address {
            addresses.push(addr.clone());
        }
        addresses
    }
}
