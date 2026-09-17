use sqlx::Row;
use chrono::{DateTime, Utc};
use bigdecimal::BigDecimal;
use crate::db::Database;
use crate::graph::Neo4jClient;
use crate::solana::accounts::{SocialEdge, BanterMarket, RingTier};
use crate::solana::instructions::BanterBetPlaced;
use crate::solana::error::{SolanaError, Result};

pub const SOLANA_INTERNAL_CHAIN_ID: u64 = 101;

pub struct SolanaProcessor {
    db: Database,
    neo4j: Option<Neo4jClient>,
    chain_id: u64,
    social_program_id: Option<[u8; 32]>,
    banter_program_id: Option<[u8; 32]>,
}

impl SolanaProcessor {
    pub fn new(db: Database, neo4j: Option<Neo4jClient>) -> Self {
        Self {
            db,
            neo4j,
            chain_id: SOLANA_INTERNAL_CHAIN_ID,
            social_program_id: None,
            banter_program_id: None,
        }
    }

    pub fn with_chain_id(mut self, chain_id: u64) -> Self {
        self.chain_id = chain_id;
        self
    }

    pub fn with_program_ids(
        mut self,
        social_program_id: Option<[u8; 32]>,
        banter_program_id: Option<[u8; 32]>,
    ) -> Self {
        self.social_program_id = social_program_id;
        self.banter_program_id = banter_program_id;
        self
    }

    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Process a decoded `SocialEdge` account update streamed from Geyser gRPC.
    ///
    /// 1. Optional PDA verification against expected pubkey and program ID.
    /// 2. Atomic slot-gated upsert into PostgreSQL `social_edges` table.
    /// 3. Update Neo4j graph:
    ///    - Inner Circle & Social Ring: materialized as `:RING_EDGE` with provisional clamped weight.
    ///    - Public Tier: deleted from Neo4j (implicit network baseline, not materialized as an edge).
    pub async fn process_social_edge(
        &self,
        account_pubkey_b58: &str,
        edge: &SocialEdge,
        slot: u64,
    ) -> Result<()> {
        // 1. Verify PDA if program ID is configured
        if let Some(ref pid) = self.social_program_id {
            let bytes = bs58::decode(account_pubkey_b58)
                .into_vec()
                .map_err(SolanaError::Base58)?;
            if bytes.len() == 32 {
                let mut expected_bytes = [0u8; 32];
                expected_bytes.copy_from_slice(&bytes);
                edge.verify_pda(&expected_bytes, pid)?;
            }
        }

        let authority = edge.authority_base58();
        let peer = edge.peer_base58();
        let tier_str = edge.ring_tier.as_str();
        let clamped_weight = edge.clamped_weight();

        // 2. Atomic slot-gated upsert in PostgreSQL
        let rows_affected = sqlx::query(
            "INSERT INTO social_edges (chain_id, authority, peer, ring_tier, weight_bps, interaction_count, last_updated_slot, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, NOW()) \
             ON CONFLICT (chain_id, authority, peer) DO UPDATE SET \
             ring_tier = EXCLUDED.ring_tier, \
             weight_bps = EXCLUDED.weight_bps, \
             interaction_count = EXCLUDED.interaction_count, \
             last_updated_slot = EXCLUDED.last_updated_slot, \
             updated_at = NOW() \
             WHERE social_edges.last_updated_slot <= EXCLUDED.last_updated_slot"
        )
        .bind(self.chain_id as i64)
        .bind(&authority)
        .bind(&peer)
        .bind(tier_str)
        .bind(edge.weight_bps as i32)
        .bind(edge.interaction_count as i64)
        .bind(slot as i64)
        .execute(self.db.pool())
        .await
        .map_err(|e| SolanaError::Database(e.to_string()))?
        .rows_affected();

        // If an older slot was delivered out-of-order, skip downstream graph updates
        if rows_affected == 0 {
            return Ok(());
        }

        // 3. Update Neo4j graph topology if client is configured
        if let Some(ref neo4j) = self.neo4j {
            let source_id = format!("solana:{}:{}", authority, peer);

            if edge.should_materialize_in_graph() {
                // Ensure Agent nodes exist
                neo4j.upsert_agent(&authority).await
                    .map_err(|e| SolanaError::Graph(e.to_string()))?;
                neo4j.upsert_agent(&peer).await
                    .map_err(|e| SolanaError::Graph(e.to_string()))?;

                // Upsert directed RING_EDGE
                neo4j.upsert_ring_edge(
                    &authority,
                    &peer,
                    tier_str,
                    clamped_weight,
                    "SOLANA_SOCIAL",
                    &source_id,
                ).await
                .map_err(|e| SolanaError::Graph(e.to_string()))?;
            } else if edge.ring_tier == RingTier::Public {
                // Public tier is implicit network baseline; remove any existing materialized edge
                neo4j.delete_ring_edge(&authority, &peer, &source_id).await
                    .map_err(|e| SolanaError::Graph(e.to_string()))?;
            }
        }

        Ok(())
    }

    /// Process a decoded `BanterMarket` account update streamed from Geyser gRPC.
    ///
    /// Ingests into `markets` with `chain_id = 101` and `tier = 3` (Banter).
    /// Solana banter markets have `pool_address = NULL`.
    pub async fn process_banter_market(
        &self,
        account_pubkey_b58: &str,
        market: &BanterMarket,
        slot: u64,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        if let Some(ref pid) = self.banter_program_id {
            let bytes = bs58::decode(account_pubkey_b58)
                .into_vec()
                .map_err(SolanaError::Base58)?;
            if bytes.len() == 32 {
                let mut expected_bytes = [0u8; 32];
                expected_bytes.copy_from_slice(&bytes);
                market.verify_pda(&expected_bytes, pid)?;
            }
        }

        let market_id = market.market_id_hex();
        let curator = market.curator_base58();
        let status_str = market.status.as_str();

        let raw_capital_bd = BigDecimal::from(market.total_raw_capital);
        let eff_stake_bd = BigDecimal::from(market.total_effective_stake);

        sqlx::query(
            "INSERT INTO markets (chain_id, market_id, stance_uri, pool_address, curator_address, tier, status, total_raw_capital, total_effective_stake, created_at_block, created_at) \
             VALUES ($1, $2, $3, NULL, $4, 3, $5, $6, $7, $8, $9) \
             ON CONFLICT (chain_id, market_id) DO UPDATE SET \
             total_raw_capital = EXCLUDED.total_raw_capital, \
             total_effective_stake = EXCLUDED.total_effective_stake, \
             status = EXCLUDED.status, \
             updated_at = NOW()"
        )
        .bind(self.chain_id as i64)
        .bind(&market_id)
        .bind(&market.stance_uri)
        .bind(&curator)
        .bind(status_str)
        .bind(&raw_capital_bd)
        .bind(&eff_stake_bd)
        .bind(slot as i64)
        .bind(timestamp)
        .execute(self.db.pool())
        .await
        .map_err(|e| SolanaError::Database(e.to_string()))?;

        Ok(())
    }

    /// Process a `BanterBetPlaced` event decoded from transaction logs.
    ///
    /// - `credibility_score`: Strictly NON-NULL, extracted directly from on-chain event log.
    /// - Idempotent insertion into `agent_trades` with `chain_id = 101`.
    /// - Position flip detection: inserts into `position_flips` and increments `total_lifetime_flips`.
    /// - Upserts into `stance_positions`.
    /// - Recalculates market SVC for `(chain_id, market_id)`.
    pub async fn process_banter_bet(
        &self,
        event: &BanterBetPlaced,
        tx_signature: &str,
        slot: u64,
        log_index: u32,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let market_id = event.market_id_hex();
        let agent_address = event.agent_base58();
        let trade_type = event.trade_type_str();

        let raw_capital_bd = BigDecimal::from(event.raw_capital);
        let token_amount_bd = BigDecimal::from(event.token_amount);
        let effective_stake_bd = BigDecimal::from(event.effective_stake);
        let execution_price_bd = BigDecimal::from(event.execution_price);
        let friction_tax_bd = BigDecimal::from(event.friction_tax);
        let credibility_score = event.credibility_score as i32;

        let mut tx = self.db.pool().begin().await
            .map_err(|e| SolanaError::Database(format!("Failed to begin tx: {}", e)))?;

        // 1. Parameterized lookup of prior stance in stance_positions
        let prior_row = sqlx::query(
            "SELECT last_action, token_balance, raw_capital_staked, effective_stake, total_lifetime_flips \
             FROM stance_positions WHERE chain_id = $1 AND market_id = $2 AND agent_address = $3 FOR UPDATE"
        )
        .bind(self.chain_id as i64)
        .bind(&market_id)
        .bind(&agent_address)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| SolanaError::Database(e.to_string()))?;

        let mut is_flip = false;
        let mut prev_action_str = None;
        let mut curr_lifetime_flips = 0i32;
        let mut current_balance = BigDecimal::from(0);
        let mut current_staked = BigDecimal::from(0);
        let mut current_eff = BigDecimal::from(0);

        if let Some(row) = prior_row {
            let last_act: Option<String> = row.get("last_action");
            if let Some(ref act) = last_act {
                prev_action_str = Some(act.clone());
                if act != trade_type {
                    is_flip = true;
                }
            }
            current_balance = row.get("token_balance");
            current_staked = row.get("raw_capital_staked");
            current_eff = row.get("effective_stake");
            curr_lifetime_flips = row.get("total_lifetime_flips");
        }

        // 2. Parameterized insertion into agent_trades
        let trade_row = sqlx::query(
            "INSERT INTO agent_trades (chain_id, market_id, agent_address, trade_type, raw_capital, token_amount, effective_stake, credibility_score, execution_price, friction_tax, tx_hash, block_number, log_index, block_timestamp) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) \
             ON CONFLICT (tx_hash, log_index) DO NOTHING \
             RETURNING id"
        )
        .bind(self.chain_id as i64)
        .bind(&market_id)
        .bind(&agent_address)
        .bind(trade_type)
        .bind(&raw_capital_bd)
        .bind(&token_amount_bd)
        .bind(&effective_stake_bd)
        .bind(credibility_score) // Strictly NON-NULL
        .bind(&execution_price_bd)
        .bind(&friction_tax_bd)
        .bind(tx_signature)
        .bind(slot as i64)
        .bind(log_index as i32)
        .bind(timestamp)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| SolanaError::Database(e.to_string()))?;

        let trade_id = match trade_row {
            Some(row) => row.get::<i64, _>("id"),
            None => {
                // Idempotent duplicate: already ingested
                tx.rollback().await.ok();
                return Ok(());
            }
        };

        let zero = BigDecimal::from(0);
        let (new_balance, new_staked, new_eff) = if trade_type == "BUY" {
            (
                &current_balance + &token_amount_bd,
                &current_staked + &raw_capital_bd,
                &current_eff + &effective_stake_bd,
            )
        } else {
            let rem_bal = if current_balance > token_amount_bd { &current_balance - &token_amount_bd } else { zero.clone() };
            let rem_stk = if current_staked > raw_capital_bd { &current_staked - &raw_capital_bd } else { zero.clone() };
            let rem_eff = if current_eff > effective_stake_bd { &current_eff - &effective_stake_bd } else { zero.clone() };
            (rem_bal, rem_stk, rem_eff)
        };

        // 3. Position flip recording
        if is_flip {
            curr_lifetime_flips += 1;
            let prev_str = prev_action_str.as_deref().unwrap_or(if trade_type == "BUY" { "SELL" } else { "BUY" });

            sqlx::query(
                "INSERT INTO position_flips (chain_id, market_id, agent_address, previous_action, flip_action, trade_id, token_amount, remaining_balance, tx_hash, block_number, block_timestamp) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"
            )
            .bind(self.chain_id as i64)
            .bind(&market_id)
            .bind(&agent_address)
            .bind(prev_str)
            .bind(trade_type)
            .bind(trade_id)
            .bind(&token_amount_bd)
            .bind(&new_balance)
            .bind(tx_signature)
            .bind(slot as i64)
            .bind(timestamp)
            .execute(&mut *tx)
            .await
            .map_err(|e| SolanaError::Database(e.to_string()))?;
        }

        // 4. Parameterized upsert into stance_positions
        sqlx::query(
            "INSERT INTO stance_positions (chain_id, market_id, agent_address, token_balance, raw_capital_staked, effective_stake, last_credibility_score, last_action, total_lifetime_flips, last_trade_block, last_trade_timestamp, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW()) \
             ON CONFLICT (chain_id, market_id, agent_address) DO UPDATE SET \
             token_balance = EXCLUDED.token_balance, \
             raw_capital_staked = EXCLUDED.raw_capital_staked, \
             effective_stake = EXCLUDED.effective_stake, \
             last_credibility_score = EXCLUDED.last_credibility_score, \
             last_action = EXCLUDED.last_action, \
             total_lifetime_flips = EXCLUDED.total_lifetime_flips, \
             last_trade_block = EXCLUDED.last_trade_block, \
             last_trade_timestamp = EXCLUDED.last_trade_timestamp, \
             updated_at = NOW()"
        )
        .bind(self.chain_id as i64)
        .bind(&market_id)
        .bind(&agent_address)
        .bind(&new_balance)
        .bind(&new_staked)
        .bind(&new_eff)
        .bind(credibility_score)
        .bind(trade_type)
        .bind(curr_lifetime_flips)
        .bind(slot as i64)
        .bind(timestamp)
        .execute(&mut *tx)
        .await
        .map_err(|e| SolanaError::Database(e.to_string()))?;

        tx.commit().await
            .map_err(|e| SolanaError::Database(format!("Failed to commit tx: {}", e)))?;

        // 5. Recalculate market SVC and total conviction for Solana banter market
        self.recalculate_market_svc(&market_id, execution_price_bd).await?;

        Ok(())
    }

    /// Recalculate Stance Volatility Coefficient (SVC) for a Solana banter market
    pub async fn recalculate_market_svc(&self, market_id: &str, current_price: BigDecimal) -> Result<()> {
        let row = sqlx::query(
            "SELECT COALESCE(SUM(raw_capital_staked), 0) AS total_k, COALESCE(SUM(effective_stake), 0) AS total_eff \
             FROM stance_positions WHERE chain_id = $1 AND market_id = $2 AND token_balance > 0"
        )
        .bind(self.chain_id as i64)
        .bind(market_id)
        .fetch_one(self.db.pool())
        .await
        .map_err(|e| SolanaError::Database(e.to_string()))?;

        let total_k: BigDecimal = row.get("total_k");
        let total_eff: BigDecimal = row.get("total_eff");

        let zero = BigDecimal::from(0);
        let svc_val = if total_k > zero && total_k >= total_eff {
            let diff = &total_k - &total_eff;
            &diff / &total_k
        } else {
            zero
        };

        sqlx::query(
            "UPDATE markets SET \
             total_raw_capital = $1, \
             total_effective_stake = $2, \
             current_svc = $3, \
             current_price = $4, \
             updated_at = NOW() \
             WHERE chain_id = $5 AND market_id = $6"
        )
        .bind(&total_k)
        .bind(&total_eff)
        .bind(&svc_val)
        .bind(&current_price)
        .bind(self.chain_id as i64)
        .bind(market_id)
        .execute(self.db.pool())
        .await
        .map_err(|e| SolanaError::Database(e.to_string()))?;

        Ok(())
    }
}
