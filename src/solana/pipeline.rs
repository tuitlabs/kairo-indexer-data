use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use chrono::Utc;
use tokio::sync::mpsc;
use crate::solana::accounts::{SocialEdge, BanterMarket};
use crate::solana::instructions::BanterBetPlaced;
use crate::solana::processor::SolanaProcessor;
use crate::solana::rpc_client::SolanaRpcClient;
use crate::solana::geyser_client::{GeyserUpdate, GeyserAccountUpdate, GeyserTransactionUpdate};
use crate::solana::error::{SolanaError, Result};

/// Orchestration pipeline that consumes streamed `GeyserUpdate` messages,
/// routes account updates to their respective Anchor decoders by owner pubkey,
/// parses transaction logs for Anchor events, performs cold-start backfill snapshots,
/// and continuously updates indexer_sync_state in PostgreSQL.
pub struct SolanaPipeline {
    processor: Arc<SolanaProcessor>,
    last_synced_slot: Arc<AtomicU64>,
}

impl SolanaPipeline {
    /// Create a new pipeline referencing the shared SolanaProcessor.
    /// Program IDs are sourced directly from processor.programs() to guarantee zero drift.
    pub fn new(processor: Arc<SolanaProcessor>) -> Self {
        Self {
            processor,
            last_synced_slot: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Backwards-compatible constructor allowing explicit program ID strings.
    pub fn new_with_program_ids(
        processor: Arc<SolanaProcessor>,
        _kairo_social_program_id: String,
        _kairo_banter_program_id: String,
    ) -> Self {
        Self::new(processor)
    }

    pub fn processor(&self) -> &Arc<SolanaProcessor> {
        &self.processor
    }

    pub fn social_program_id(&self) -> Option<&str> {
        self.processor.social_program_id().map(|p| p.as_str())
    }

    pub fn banter_program_id(&self) -> Option<&str> {
        self.processor.banter_program_id().map(|p| p.as_str())
    }

    pub fn last_synced_slot(&self) -> u64 {
        self.last_synced_slot.load(Ordering::Relaxed)
    }

    /// Record a processed slot back into `indexer_sync_state` in PostgreSQL.
    pub async fn record_sync_slot(&self, slot: u64) -> Result<()> {
        let prev = self.last_synced_slot.load(Ordering::Relaxed);
        if slot > prev {
            self.processor.db().update_sync_state(
                self.processor.chain_id(),
                slot,
                &format!("slot_{}", slot),
            ).await.map_err(|e| SolanaError::Database(e.to_string()))?;
            self.last_synced_slot.store(slot, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Cold-start / restart backfill: checks `indexer_sync_state` for chain_id=101.
    /// If there is a gap or cold start, fetches snapshots via `getProgramAccounts`
    /// for kairo_social and kairo_banter, ingests them, and advances `indexer_sync_state`.
    pub async fn backfill(&self, rpc: &SolanaRpcClient) -> Result<Option<u64>> {
        let chain_id = self.processor.chain_id();
        let last_indexed = self.processor.db().get_last_indexed_block(chain_id)
            .await
            .map_err(|e| SolanaError::Database(e.to_string()))?;

        let current_head = match rpc.get_slot().await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Warning: Failed to fetch Solana cluster slot for backfill: {}", e);
                return Ok(None);
            }
        };

        // If we already have a record and it's up to date with cluster head, skip backfill
        if let Some(last) = last_indexed {
            if last >= current_head {
                self.last_synced_slot.store(last, Ordering::Relaxed);
                return Ok(None);
            }
        }

        let mut max_snapshot_slot = current_head;

        // 1. Backfill kairo_social accounts via getProgramAccounts
        if let Some(social_id) = self.processor.social_program_id() {
            match rpc.get_program_accounts(social_id.as_str()).await {
                Ok(resp) => {
                    max_snapshot_slot = std::cmp::max(max_snapshot_slot, resp.slot);
                    for acc in resp.accounts {
                        let disc = SocialEdge::discriminator();
                        if acc.data.len() >= 8 && acc.data[..8] == disc {
                            if let Ok(edge) = SocialEdge::from_bytes(&acc.data) {
                                self.processor.process_social_edge(&acc.pubkey, &edge, resp.slot).await?;
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Warning: Failed to fetch kairo_social snapshot: {}", e);
                }
            }
        }

        // 2. Backfill kairo_banter accounts via getProgramAccounts
        if let Some(banter_id) = self.processor.banter_program_id() {
            match rpc.get_program_accounts(banter_id.as_str()).await {
                Ok(resp) => {
                    max_snapshot_slot = std::cmp::max(max_snapshot_slot, resp.slot);
                    for acc in resp.accounts {
                        let disc = BanterMarket::discriminator();
                        if acc.data.len() >= 8 && acc.data[..8] == disc {
                            if let Ok(market) = BanterMarket::from_bytes(&acc.data) {
                                self.processor.process_banter_market(&acc.pubkey, &market, resp.slot, Utc::now()).await?;
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Warning: Failed to fetch kairo_banter snapshot: {}", e);
                }
            }
        }

        // 3. Write snapshot sync progress back to PostgreSQL
        self.processor.db().update_sync_state(
            chain_id,
            max_snapshot_slot,
            &format!("snapshot_{}", max_snapshot_slot),
        ).await.map_err(|e| SolanaError::Database(e.to_string()))?;

        self.last_synced_slot.store(max_snapshot_slot, Ordering::Relaxed);
        Ok(Some(max_snapshot_slot))
    }

    /// Process a single incoming GeyserUpdate (Account or Transaction) and advance sync progress.
    pub async fn process_update(&self, update: GeyserUpdate) -> Result<()> {
        let slot = match &update {
            GeyserUpdate::Account(acc) => {
                self.process_account_update(acc).await?;
                acc.slot
            }
            GeyserUpdate::Transaction(tx) => {
                self.process_transaction_update(tx).await?;
                tx.slot
            }
        };

        self.record_sync_slot(slot).await?;
        Ok(())
    }

    /// Route account update by owner pubkey and Anchor discriminator.
    pub async fn process_account_update(&self, acc: &GeyserAccountUpdate) -> Result<()> {
        if let Some(social_id) = self.processor.social_program_id() {
            if acc.owner == social_id.as_str() {
                // Check Anchor discriminator for SocialEdge: sha256("account:SocialEdge")[0..8]
                let expected_disc = SocialEdge::discriminator();
                if acc.data.len() >= 8 && acc.data[..8] == expected_disc {
                    let edge = SocialEdge::from_bytes(&acc.data)?;
                    self.processor.process_social_edge(&acc.pubkey, &edge, acc.slot).await?;
                }
            }
        }

        if let Some(banter_id) = self.processor.banter_program_id() {
            if acc.owner == banter_id.as_str() {
                // Check Anchor discriminator for BanterMarket: sha256("account:BanterMarket")[0..8]
                let expected_disc = BanterMarket::discriminator();
                if acc.data.len() >= 8 && acc.data[..8] == expected_disc {
                    let market = BanterMarket::from_bytes(&acc.data)?;
                    self.processor.process_banter_market(&acc.pubkey, &market, acc.slot, Utc::now()).await?;
                }
            }
        }

        Ok(())
    }

    /// Extract Anchor events from transaction logs and process bets.
    pub async fn process_transaction_update(&self, tx: &GeyserTransactionUpdate) -> Result<()> {
        // Skip failed transactions
        if tx.err.is_some() {
            return Ok(());
        }

        let bets = BanterBetPlaced::extract_from_logs(&tx.logs)?;
        for (idx, bet) in bets.iter().enumerate() {
            self.processor.process_banter_bet(bet, &tx.signature, tx.slot, idx as u32, Utc::now()).await?;
        }
        Ok(())
    }

    /// Orchestration loop: continuously consumes streamed updates from GeyserClient until closed.
    pub async fn run(&self, mut rx: mpsc::Receiver<GeyserUpdate>) -> Result<()> {
        while let Some(update) = rx.recv().await {
            if let Err(e) = self.process_update(update).await {
                eprintln!("Error processing Solana Geyser update: {}", e);
            }
        }
        Ok(())
    }

    /// Full pipeline runner: performs cold-start backfill via RPC then transitions to live Geyser stream.
    pub async fn run_with_backfill(&self, rpc: &SolanaRpcClient, rx: mpsc::Receiver<GeyserUpdate>) -> Result<()> {
        if let Ok(Some(slot)) = self.backfill(rpc).await {
            println!("Solana snapshot backfill completed at slot {}", slot);
        }
        self.run(rx).await
    }

    /// Spawn the orchestration loop as a background Tokio task.
    pub fn start(self: Arc<Self>, rx: mpsc::Receiver<GeyserUpdate>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(e) = self.run(rx).await {
                eprintln!("Solana pipeline orchestration loop exited with error: {}", e);
            }
        })
    }
}
