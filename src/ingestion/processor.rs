use chrono::Utc;
use alloy::rpc::types::eth::Log as AlloyLog;
use crate::error::Result;
use crate::db::Database;
use crate::abi::events::*;
use super::models::RawLog;

pub struct EventProcessor {
    db: Database,
    chain_id: u64,
    lcf_window_seconds: u64,
}

impl EventProcessor {
    pub fn new(db: Database, chain_id: u64) -> Self {
        Self {
            db,
            chain_id,
            lcf_window_seconds: 3600, // 1-hour rolling window default for delta_lcf
        }
    }

    /// Process an Alloy typed Log from WebSocket subscription
    pub async fn process_alloy_log(&self, log: &AlloyLog) -> Result<Option<String>> {
        let block_number = log.block_number.unwrap_or(0);
        let log_index = log.log_index.unwrap_or(0) as u32;
        let tx_index = log.transaction_index.unwrap_or(0) as u32;
        let tx_hash = log.transaction_hash.map(|h| format!("{:#x}", h)).unwrap_or_default();
        let block_hash = log.block_hash.map(|h| format!("{:#x}", h)).unwrap_or_default();
        let contract_address = format!("{:#x}", log.address());
        let timestamp = Utc::now();

        let topics: Vec<String> = log.topics().iter().map(|t| format!("{:#x}", t)).collect();
        let data_hex = hex::encode(&log.data().data);

        let decoded = match ParsedKairoEvent::decode(&topics, &data_hex)? {
            Some(ev) => ev,
            None => return Ok(None),
        };

        let event_name = match &decoded {
            ParsedKairoEvent::MarketCreated(_) => "MarketCreated",
            ParsedKairoEvent::StanceBought(_) => "StanceBought",
            ParsedKairoEvent::StanceSold(_) => "StanceSold",
            ParsedKairoEvent::DisclosureEnforced(_) => "DisclosureEnforced",
            ParsedKairoEvent::EpochFinalized(_) => "EpochFinalized",
            ParsedKairoEvent::YieldDistributed(_) => "YieldDistributed",
            ParsedKairoEvent::CoalitionFormed(_) => "CoalitionFormed",
            ParsedKairoEvent::CoalitionMemberJoined(_) => "CoalitionMemberJoined",
            ParsedKairoEvent::DeceptionTaxCharged(_) => "DeceptionTaxCharged",
        };

        let event_sig = topics.first().cloned().unwrap_or_default();
        let payload_json = serde_json::to_value(&decoded).unwrap_or_default();

        // 1. Parameterized insert into raw block_events
        self.db.insert_block_event(
            self.chain_id,
            block_number,
            &block_hash,
            &tx_hash,
            tx_index,
            log_index,
            &contract_address,
            event_name,
            &event_sig,
            &payload_json,
            timestamp,
        ).await?;

        // 2. Dispatch state updates
        match &decoded {
            ParsedKairoEvent::MarketCreated(ev) => {
                self.db.record_market_created(ev, block_number, timestamp).await?;
            }
            ParsedKairoEvent::StanceBought(ev) => {
                self.db.process_stance_bought(ev, &tx_hash, block_number, log_index, timestamp).await?;
            }
            ParsedKairoEvent::StanceSold(ev) => {
                self.db.process_stance_sold(ev, &tx_hash, block_number, log_index, timestamp).await?;
            }
            ParsedKairoEvent::DeceptionTaxCharged(ev) => {
                self.db.record_deception_tax(ev, &tx_hash, block_number, timestamp, self.lcf_window_seconds).await?;
            }
            ParsedKairoEvent::EpochFinalized(ev) => {
                self.db.record_epoch_finalized(ev, block_number, &tx_hash, timestamp).await?;
            }
            ParsedKairoEvent::YieldDistributed(ev) => {
                self.db.record_yield_distributed(ev).await?;
            }
            ParsedKairoEvent::CoalitionFormed(ev) => {
                self.db.record_coalition_formed(ev, block_number, timestamp).await?;
            }
            ParsedKairoEvent::CoalitionMemberJoined(ev) => {
                self.db.record_coalition_member_joined(ev, block_number, timestamp).await?;
            }
            ParsedKairoEvent::DisclosureEnforced(ev) => {
                self.db.record_disclosure_enforced(ev, block_number).await?;
            }
        }

        // 3. Update sync state
        if block_number > 0 {
            self.db.update_sync_state(self.chain_id, block_number, &block_hash).await?;
        }

        Ok(Some(event_name.to_string()))
    }

    /// Process a RawLog (from JSON-RPC batch queries)
    pub async fn process_log(&self, raw_log: &RawLog) -> Result<Option<String>> {
        let block_number = raw_log.parse_block_number();
        let log_index = raw_log.parse_log_index();
        let tx_index = raw_log.parse_tx_index();
        let tx_hash = raw_log.transaction_hash.clone().unwrap_or_default();
        let block_hash = raw_log.block_hash.clone().unwrap_or_default();
        let contract_address = raw_log.address.clone();
        let timestamp = Utc::now();

        let decoded = match ParsedKairoEvent::decode(&raw_log.topics, &raw_log.data)? {
            Some(ev) => ev,
            None => return Ok(None),
        };

        let event_name = match &decoded {
            ParsedKairoEvent::MarketCreated(_) => "MarketCreated",
            ParsedKairoEvent::StanceBought(_) => "StanceBought",
            ParsedKairoEvent::StanceSold(_) => "StanceSold",
            ParsedKairoEvent::DisclosureEnforced(_) => "DisclosureEnforced",
            ParsedKairoEvent::EpochFinalized(_) => "EpochFinalized",
            ParsedKairoEvent::YieldDistributed(_) => "YieldDistributed",
            ParsedKairoEvent::CoalitionFormed(_) => "CoalitionFormed",
            ParsedKairoEvent::CoalitionMemberJoined(_) => "CoalitionMemberJoined",
            ParsedKairoEvent::DeceptionTaxCharged(_) => "DeceptionTaxCharged",
        };

        let event_sig = raw_log.topics.first().cloned().unwrap_or_default();
        let payload_json = serde_json::to_value(&decoded).unwrap_or_default();

        self.db.insert_block_event(
            self.chain_id,
            block_number,
            &block_hash,
            &tx_hash,
            tx_index,
            log_index,
            &contract_address,
            event_name,
            &event_sig,
            &payload_json,
            timestamp,
        ).await?;

        match &decoded {
            ParsedKairoEvent::MarketCreated(ev) => {
                self.db.record_market_created(ev, block_number, timestamp).await?;
            }
            ParsedKairoEvent::StanceBought(ev) => {
                self.db.process_stance_bought(ev, &tx_hash, block_number, log_index, timestamp).await?;
            }
            ParsedKairoEvent::StanceSold(ev) => {
                self.db.process_stance_sold(ev, &tx_hash, block_number, log_index, timestamp).await?;
            }
            ParsedKairoEvent::DeceptionTaxCharged(ev) => {
                self.db.record_deception_tax(ev, &tx_hash, block_number, timestamp, self.lcf_window_seconds).await?;
            }
            ParsedKairoEvent::EpochFinalized(ev) => {
                self.db.record_epoch_finalized(ev, block_number, &tx_hash, timestamp).await?;
            }
            ParsedKairoEvent::YieldDistributed(ev) => {
                self.db.record_yield_distributed(ev).await?;
            }
            ParsedKairoEvent::CoalitionFormed(ev) => {
                self.db.record_coalition_formed(ev, block_number, timestamp).await?;
            }
            ParsedKairoEvent::CoalitionMemberJoined(ev) => {
                self.db.record_coalition_member_joined(ev, block_number, timestamp).await?;
            }
            ParsedKairoEvent::DisclosureEnforced(ev) => {
                self.db.record_disclosure_enforced(ev, block_number).await?;
            }
        }

        if block_number > 0 {
            self.db.update_sync_state(self.chain_id, block_number, &block_hash).await?;
        }

        Ok(Some(event_name.to_string()))
    }
}
