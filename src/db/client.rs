use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use chrono::{DateTime, Utc};
use bigdecimal::BigDecimal;
use crate::error::{IndexerError, Result};
use crate::abi::events::*;

pub fn u256_to_bigdecimal(val: ethnum::U256) -> BigDecimal {
    let s = val.to_string();
    BigDecimal::parse_bytes(s.as_bytes(), 10).unwrap_or_default()
}

#[derive(Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .connect(url)
            .await
            .map_err(|e| IndexerError::Database(format!("Failed to connect to PG pool: {}", e)))?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn get_last_indexed_block(&self, chain_id: u64) -> Result<Option<u64>> {
        let row = sqlx::query(
            "SELECT last_indexed_block FROM indexer_sync_state WHERE chain_id = $1"
        )
        .bind(chain_id as i64)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        Ok(row.map(|r| {
            let val: i64 = r.get("last_indexed_block");
            val as u64
        }))
    }

    pub async fn update_sync_state(&self, chain_id: u64, block_number: u64, block_hash: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO indexer_sync_state (chain_id, last_indexed_block, last_indexed_block_hash, updated_at) \
             VALUES ($1, $2, $3, NOW()) \
             ON CONFLICT (chain_id) DO UPDATE SET \
             last_indexed_block = EXCLUDED.last_indexed_block, \
             last_indexed_block_hash = EXCLUDED.last_indexed_block_hash, \
             updated_at = NOW()"
        )
        .bind(chain_id as i64)
        .bind(block_number as i64)
        .bind(block_hash)
        .execute(&self.pool)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        Ok(())
    }

    pub async fn insert_block_event(
        &self,
        chain_id: u64,
        block_number: u64,
        block_hash: &str,
        tx_hash: &str,
        tx_index: u32,
        log_index: u32,
        contract_address: &str,
        event_name: &str,
        event_sig: &str,
        payload: &serde_json::Value,
        block_timestamp: DateTime<Utc>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "INSERT INTO block_events (chain_id, block_number, block_hash, tx_hash, tx_index, log_index, contract_address, event_name, event_signature, payload, block_timestamp) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
             ON CONFLICT (chain_id, block_number, log_index) DO NOTHING"
        )
        .bind(chain_id as i64)
        .bind(block_number as i64)
        .bind(block_hash)
        .bind(tx_hash)
        .bind(tx_index as i32)
        .bind(log_index as i32)
        .bind(contract_address.to_lowercase())
        .bind(event_name)
        .bind(event_sig)
        .bind(payload)
        .bind(block_timestamp)
        .execute(&self.pool)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn record_market_created(
        &self,
        event: &MarketCreatedEvent,
        block_number: u64,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO markets (chain_id, market_id, stance_uri, pool_address, curator_address, tier, status, created_at_block, created_at) \
             VALUES (8453, $1, $2, $3, $4, 2, 'ACTIVE', $5, $6) \
             ON CONFLICT (chain_id, market_id) DO NOTHING"
        )
        .bind(&event.market_id)
        .bind(&event.stance_uri)
        .bind(event.pool_address.to_lowercase())
        .bind(event.curator.to_lowercase())
        .bind(block_number as i64)
        .bind(timestamp)
        .execute(&self.pool)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        Ok(())
    }

    pub async fn process_stance_bought(
        &self,
        event: &StanceBoughtEvent,
        tx_hash: &str,
        block_number: u64,
        log_index: u32,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await
            .map_err(|e| IndexerError::Database(format!("Failed to begin tx: {}", e)))?;

        // 1. Parameterized lookup of prior posture in stance_positions
        let prior_row = sqlx::query(
            "SELECT last_action, token_balance, raw_capital_staked, effective_stake, total_lifetime_flips \
             FROM stance_positions WHERE market_id = $1 AND agent_address = $2 FOR UPDATE"
        )
        .bind(&event.market_id)
        .bind(event.agent.to_lowercase())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        let mut is_flip = false;
        let mut prev_action_str = None;
        let mut curr_lifetime_flips = 0i32;
        let mut current_balance = BigDecimal::from(0);
        let mut current_staked = BigDecimal::from(0);
        let mut current_eff = BigDecimal::from(0);

        if let Some(row) = prior_row {
            let last_act: Option<String> = row.get("last_action");
            if let Some(act) = last_act {
                prev_action_str = Some(act.clone());
                if act == "SELL" {
                    // Action reversal: SELL -> BUY!
                    is_flip = true;
                }
            }
            current_balance = row.get("token_balance");
            current_staked = row.get("raw_capital_staked");
            current_eff = row.get("effective_stake");
            curr_lifetime_flips = row.get("total_lifetime_flips");
        }

        let deposit_bd = u256_to_bigdecimal(event.deposit_amt);
        let tokens_bd = u256_to_bigdecimal(event.tokens_received);
        let eff_bd = u256_to_bigdecimal(event.effective_stake);
        let price_bd = u256_to_bigdecimal(event.current_price);

        // 2. Parameterized insertion into agent_trades
        let trade_row = sqlx::query(
            "INSERT INTO agent_trades (market_id, agent_address, trade_type, raw_capital, token_amount, effective_stake, credibility_score, execution_price, tx_hash, block_number, log_index, block_timestamp) \
             VALUES ($1, $2, 'BUY', $3, $4, $5, $6, $7, $8, $9, $10, $11) \
             RETURNING id"
        )
        .bind(&event.market_id)
        .bind(event.agent.to_lowercase())
        .bind(&deposit_bd)
        .bind(&tokens_bd)
        .bind(&eff_bd)
        .bind(event.credibility_score as i32)
        .bind(&price_bd)
        .bind(tx_hash)
        .bind(block_number as i64)
        .bind(log_index as i32)
        .bind(timestamp)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        let trade_id: i64 = trade_row.get("id");

        let new_balance = &current_balance + &tokens_bd;
        let new_staked = &current_staked + &deposit_bd;
        let new_eff = &current_eff + &eff_bd;

        // 3. If action reversal, record in position_flips and increment total_lifetime_flips
        if is_flip {
            curr_lifetime_flips += 1;
            sqlx::query(
                "INSERT INTO position_flips (market_id, agent_address, previous_action, flip_action, trade_id, token_amount, remaining_balance, tx_hash, block_number, block_timestamp) \
                 VALUES ($1, $2, $3, 'BUY', $4, $5, $6, $7, $8, $9)"
            )
            .bind(&event.market_id)
            .bind(event.agent.to_lowercase())
            .bind(prev_action_str.as_deref().unwrap_or("SELL"))
            .bind(trade_id)
            .bind(&tokens_bd)
            .bind(&new_balance)
            .bind(tx_hash)
            .bind(block_number as i64)
            .bind(timestamp)
            .execute(&mut *tx)
            .await
            .map_err(|e| IndexerError::Database(e.to_string()))?;
        }

        // 4. Parameterized upsert into stance_positions, strictly persisting total_lifetime_flips
        sqlx::query(
            "INSERT INTO stance_positions (chain_id, market_id, agent_address, token_balance, raw_capital_staked, effective_stake, last_credibility_score, last_action, total_lifetime_flips, last_trade_block, last_trade_timestamp, updated_at) \
             VALUES (8453, $1, $2, $3, $4, $5, $6, 'BUY', $7, $8, $9, NOW()) \
             ON CONFLICT (chain_id, market_id, agent_address) DO UPDATE SET \
             token_balance = EXCLUDED.token_balance, \
             raw_capital_staked = EXCLUDED.raw_capital_staked, \
             effective_stake = EXCLUDED.effective_stake, \
             last_credibility_score = EXCLUDED.last_credibility_score, \
             last_action = 'BUY', \
             total_lifetime_flips = EXCLUDED.total_lifetime_flips, \
             last_trade_block = EXCLUDED.last_trade_block, \
             last_trade_timestamp = EXCLUDED.last_trade_timestamp, \
             updated_at = NOW()"
        )
        .bind(&event.market_id)
        .bind(event.agent.to_lowercase())
        .bind(&new_balance)
        .bind(&new_staked)
        .bind(&new_eff)
        .bind(event.credibility_score as i32)
        .bind(curr_lifetime_flips)
        .bind(block_number as i64)
        .bind(timestamp)
        .execute(&mut *tx)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        tx.commit().await
            .map_err(|e| IndexerError::Database(format!("Failed to commit tx: {}", e)))?;

        // 5. Recalculate market SVC and total conviction
        self.recalculate_market_svc(&event.market_id, price_bd).await?;

        Ok(())
    }

    pub async fn process_stance_sold(
        &self,
        event: &StanceSoldEvent,
        tx_hash: &str,
        block_number: u64,
        log_index: u32,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await
            .map_err(|e| IndexerError::Database(format!("Failed to begin tx: {}", e)))?;

        let prior_row = sqlx::query(
            "SELECT last_action, token_balance, raw_capital_staked, effective_stake, total_lifetime_flips, last_credibility_score \
             FROM stance_positions WHERE market_id = $1 AND agent_address = $2 FOR UPDATE"
        )
        .bind(&event.market_id)
        .bind(event.agent.to_lowercase())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        let mut is_flip = false;
        let mut prev_action_str = None;
        let mut curr_lifetime_flips = 0i32;
        let mut current_balance = BigDecimal::from(0);
        let mut current_staked = BigDecimal::from(0);
        let mut current_eff = BigDecimal::from(0);
        let mut cred_score = 500i32;

        if let Some(row) = prior_row {
            let last_act: Option<String> = row.get("last_action");
            if let Some(act) = last_act {
                prev_action_str = Some(act.clone());
                if act == "BUY" {
                    // Action reversal: BUY -> SELL!
                    is_flip = true;
                }
            }
            current_balance = row.get("token_balance");
            current_staked = row.get("raw_capital_staked");
            current_eff = row.get("effective_stake");
            curr_lifetime_flips = row.get("total_lifetime_flips");
            cred_score = row.get("last_credibility_score");
        }

        let token_bd = u256_to_bigdecimal(event.token_amt);
        let col_ret_bd = u256_to_bigdecimal(event.collateral_returned);
        let price_bd = u256_to_bigdecimal(event.exit_price);

        let zero = BigDecimal::from(0);
        let new_balance = if current_balance > token_bd { &current_balance - &token_bd } else { zero.clone() };
        let new_staked = if current_staked > col_ret_bd { &current_staked - &col_ret_bd } else { zero.clone() };

        // Note (v1 simplification): new_eff uses a weighted-average reduction
        // (current_eff * new_balance / current_balance) rather than tracking effective
        // stake per acquisition lot. Credibility-at-entry is therefore blended across
        // held tokens rather than preserved per-lot.
        let new_eff = if current_balance > zero && new_balance > zero {
            (&current_eff * &new_balance) / &current_balance
        } else {
            zero.clone()
        };
        // Compute the delta effective stake liquidated/removed by this SELL (blended average)
        let eff_removed = if current_eff > new_eff {
            &current_eff - &new_eff
        } else {
            zero
        };

        let trade_row = sqlx::query(
            "INSERT INTO agent_trades (market_id, agent_address, trade_type, raw_capital, token_amount, effective_stake, credibility_score, execution_price, tx_hash, block_number, log_index, block_timestamp) \
             VALUES ($1, $2, 'SELL', $3, $4, $5, $6, $7, $8, $9, $10, $11) \
             RETURNING id"
        )
        .bind(&event.market_id)
        .bind(event.agent.to_lowercase())
        .bind(&col_ret_bd)
        .bind(&token_bd)
        .bind(&eff_removed)
        .bind(cred_score)
        .bind(&price_bd)
        .bind(tx_hash)
        .bind(block_number as i64)
        .bind(log_index as i32)
        .bind(timestamp)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        let trade_id: i64 = trade_row.get("id");

        if is_flip {
            curr_lifetime_flips += 1;
            sqlx::query(
                "INSERT INTO position_flips (market_id, agent_address, previous_action, flip_action, trade_id, token_amount, remaining_balance, tx_hash, block_number, block_timestamp) \
                 VALUES ($1, $2, $3, 'SELL', $4, $5, $6, $7, $8, $9)"
            )
            .bind(&event.market_id)
            .bind(event.agent.to_lowercase())
            .bind(prev_action_str.as_deref().unwrap_or("BUY"))
            .bind(trade_id)
            .bind(&token_bd)
            .bind(&new_balance)
            .bind(tx_hash)
            .bind(block_number as i64)
            .bind(timestamp)
            .execute(&mut *tx)
            .await
            .map_err(|e| IndexerError::Database(e.to_string()))?;
        }

        sqlx::query(
            "INSERT INTO stance_positions (chain_id, market_id, agent_address, token_balance, raw_capital_staked, effective_stake, last_credibility_score, last_action, total_lifetime_flips, last_trade_block, last_trade_timestamp, updated_at) \
             VALUES (8453, $1, $2, $3, $4, $5, $6, 'SELL', $7, $8, $9, NOW()) \
             ON CONFLICT (chain_id, market_id, agent_address) DO UPDATE SET \
             token_balance = EXCLUDED.token_balance, \
             raw_capital_staked = EXCLUDED.raw_capital_staked, \
             effective_stake = EXCLUDED.effective_stake, \
             last_action = 'SELL', \
             total_lifetime_flips = EXCLUDED.total_lifetime_flips, \
             last_trade_block = EXCLUDED.last_trade_block, \
             last_trade_timestamp = EXCLUDED.last_trade_timestamp, \
             updated_at = NOW()"
        )
        .bind(&event.market_id)
        .bind(event.agent.to_lowercase())
        .bind(&new_balance)
        .bind(&new_staked)
        .bind(&new_eff)
        .bind(cred_score)
        .bind(curr_lifetime_flips)
        .bind(block_number as i64)
        .bind(timestamp)
        .execute(&mut *tx)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        tx.commit().await
            .map_err(|e| IndexerError::Database(format!("Failed to commit tx: {}", e)))?;

        self.recalculate_market_svc(&event.market_id, price_bd).await?;
        Ok(())
    }

    pub async fn recalculate_market_svc(&self, market_id: &str, current_price: BigDecimal) -> Result<()> {
        let row = sqlx::query(
            "SELECT COALESCE(SUM(raw_capital_staked), 0) AS total_k, COALESCE(SUM(effective_stake), 0) AS total_eff \
             FROM stance_positions WHERE market_id = $1 AND token_balance > 0"
        )
        .bind(market_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

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
             WHERE market_id = $5"
        )
        .bind(&total_k)
        .bind(&total_eff)
        .bind(&svc_val)
        .bind(&current_price)
        .bind(market_id)
        .execute(&self.pool)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        Ok(())
    }

    pub async fn calculate_windowed_velocity(
        &self,
        agent_address: &str,
        market_id: &str,
        window_seconds: u64,
    ) -> Result<u32> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS flip_count FROM position_flips \
             WHERE agent_address = $1 AND market_id = $2 \
               AND block_timestamp >= (NOW() - ($3 * INTERVAL '1 second'))"
        )
        .bind(agent_address.to_lowercase())
        .bind(market_id)
        .bind(window_seconds as i64)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        let count: i64 = row.get("flip_count");
        Ok(count as u32)
    }

    pub async fn record_deception_tax(
        &self,
        event: &DeceptionTaxChargedEvent,
        tx_hash: &str,
        block_number: u64,
        timestamp: DateTime<Utc>,
        window_seconds: u64,
    ) -> Result<()> {
        let calculated_velocity = self.calculate_windowed_velocity(&event.agent, &event.market_id, window_seconds).await.unwrap_or(0);
        let tax_bd = u256_to_bigdecimal(event.tax_amount);

        sqlx::query(
            "INSERT INTO deception_tax_events (market_id, agent_address, tax_amount, reported_flip_velocity, calculated_flip_velocity, credibility_score, tx_hash, block_number, block_timestamp) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
        )
        .bind(&event.market_id)
        .bind(event.agent.to_lowercase())
        .bind(&tax_bd)
        .bind(event.flip_velocity as i32)
        .bind(calculated_velocity as i32)
        .bind(event.credibility_score as i32)
        .bind(tx_hash)
        .bind(block_number as i64)
        .bind(timestamp)
        .execute(&self.pool)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        Ok(())
    }

    pub async fn record_epoch_finalized(
        &self,
        event: &EpochFinalizedEvent,
        block_number: u64,
        tx_hash: &str,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let ratio_bd = u256_to_bigdecimal(event.consensus_ratio);
        let eff_bd = u256_to_bigdecimal(event.total_effective_stake);
        let k_bd = u256_to_bigdecimal(event.total_raw_capital);

        sqlx::query(
            "INSERT INTO epoch_settlements (chain_id, market_id, epoch_id, consensus_ratio, total_effective_stake, total_raw_capital, block_number, tx_hash, settled_at) \
             VALUES (8453, $1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (chain_id, market_id, epoch_id) DO UPDATE SET \
             consensus_ratio = EXCLUDED.consensus_ratio, \
             total_effective_stake = EXCLUDED.total_effective_stake, \
             total_raw_capital = EXCLUDED.total_raw_capital"
        )
        .bind(&event.market_id)
        .bind(event.epoch_id as i64)
        .bind(&ratio_bd)
        .bind(&eff_bd)
        .bind(&k_bd)
        .bind(block_number as i64)
        .bind(tx_hash)
        .bind(timestamp)
        .execute(&self.pool)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        Ok(())
    }

    pub async fn record_yield_distributed(&self, event: &YieldDistributedEvent) -> Result<()> {
        let yield_bd = u256_to_bigdecimal(event.total_yield);
        let wallet_bd = u256_to_bigdecimal(event.agent_wallet_share);
        let owner_bd = u256_to_bigdecimal(event.agent_owner_share);
        let coalition_bd = u256_to_bigdecimal(event.coalition_share);
        let treasury_bd = u256_to_bigdecimal(event.treasury_share);

        sqlx::query(
            "UPDATE epoch_settlements SET \
             total_yield = $1, \
             agent_wallet_payout = $2, \
             agent_owner_payout = $3, \
             coalition_payout = $4, \
             treasury_payout = $5 \
             WHERE chain_id = 8453 AND market_id = $6 AND epoch_id = $7"
        )
        .bind(&yield_bd)
        .bind(&wallet_bd)
        .bind(&owner_bd)
        .bind(&coalition_bd)
        .bind(&treasury_bd)
        .bind(&event.market_id)
        .bind(event.epoch_id as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        Ok(())
    }

    pub async fn record_coalition_formed(
        &self,
        event: &CoalitionFormedEvent,
        block_number: u64,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO coalitions (coalition_id, yield_bps, status, created_at_block, created_at) \
             VALUES ($1, $2, 'ACTIVE', $3, $4) \
             ON CONFLICT (coalition_id) DO NOTHING"
        )
        .bind(&event.coalition_id)
        .bind(event.yield_bps as i32)
        .bind(block_number as i64)
        .bind(timestamp)
        .execute(&self.pool)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        Ok(())
    }

    pub async fn record_coalition_member_joined(
        &self,
        event: &CoalitionMemberJoinedEvent,
        block_number: u64,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO coalition_members (coalition_id, agent_address, agent_id, share_bps, joined_at_block, joined_at) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (coalition_id, agent_address) DO UPDATE SET share_bps = EXCLUDED.share_bps"
        )
        .bind(&event.coalition_id)
        .bind(event.agent_address.to_lowercase())
        .bind(event.agent_id as i64)
        .bind(event.share_bps as i32)
        .bind(block_number as i64)
        .bind(timestamp)
        .execute(&self.pool)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        Ok(())
    }

    pub async fn record_disclosure_enforced(
        &self,
        event: &DisclosureEnforcedEvent,
        block_number: u64,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE coalitions SET status = 'DISCLOSED', disclosure_block = $1, updated_at = NOW() \
             WHERE coalition_id = $2"
        )
        .bind(block_number as i64)
        .bind(&event.coalition_id)
        .execute(&self.pool)
        .await
        .map_err(|e| IndexerError::Database(e.to_string()))?;

        Ok(())
    }
}
