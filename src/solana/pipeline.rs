use std::sync::Arc;
use chrono::Utc;
use tokio::sync::mpsc;
use crate::solana::accounts::{SocialEdge, BanterMarket};
use crate::solana::instructions::BanterBetPlaced;
use crate::solana::processor::SolanaProcessor;
use crate::solana::geyser_client::{GeyserUpdate, GeyserAccountUpdate, GeyserTransactionUpdate};
use crate::solana::error::Result;

/// Orchestration pipeline that consumes streamed `GeyserUpdate` messages,
/// routes account updates to their respective Anchor decoders by owner pubkey,
/// parses transaction logs for Anchor events, and updates PostgreSQL and Neo4j.
pub struct SolanaPipeline {
    processor: Arc<SolanaProcessor>,
    kairo_social_program_id: String,
    kairo_banter_program_id: String,
}

impl SolanaPipeline {
    pub fn new(
        processor: Arc<SolanaProcessor>,
        kairo_social_program_id: String,
        kairo_banter_program_id: String,
    ) -> Self {
        Self {
            processor,
            kairo_social_program_id,
            kairo_banter_program_id,
        }
    }

    pub fn processor(&self) -> &Arc<SolanaProcessor> {
        &self.processor
    }

    pub fn social_program_id(&self) -> &str {
        &self.kairo_social_program_id
    }

    pub fn banter_program_id(&self) -> &str {
        &self.kairo_banter_program_id
    }

    /// Process a single incoming GeyserUpdate (Account or Transaction).
    pub async fn process_update(&self, update: GeyserUpdate) -> Result<()> {
        match update {
            GeyserUpdate::Account(acc) => self.process_account_update(&acc).await,
            GeyserUpdate::Transaction(tx) => self.process_transaction_update(&tx).await,
        }
    }

    /// Route account update by owner pubkey and Anchor discriminator.
    pub async fn process_account_update(&self, acc: &GeyserAccountUpdate) -> Result<()> {
        if acc.owner == self.kairo_social_program_id {
            // Check Anchor discriminator for SocialEdge: sha256("account:SocialEdge")[0..8]
            let expected_disc = SocialEdge::discriminator();
            if acc.data.len() >= 8 && acc.data[..8] == expected_disc {
                let edge = SocialEdge::from_bytes(&acc.data)?;
                self.processor.process_social_edge(&acc.pubkey, &edge, acc.slot).await?;
            }
        } else if acc.owner == self.kairo_banter_program_id {
            // Check Anchor discriminator for BanterMarket: sha256("account:BanterMarket")[0..8]
            let expected_disc = BanterMarket::discriminator();
            if acc.data.len() >= 8 && acc.data[..8] == expected_disc {
                let market = BanterMarket::from_bytes(&acc.data)?;
                self.processor.process_banter_market(&acc.pubkey, &market, acc.slot, Utc::now()).await?;
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

    /// Spawn the orchestration loop as a background Tokio task.
    pub fn start(self: Arc<Self>, rx: mpsc::Receiver<GeyserUpdate>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(e) = self.run(rx).await {
                eprintln!("Solana pipeline orchestration loop exited with error: {}", e);
            }
        })
    }
}
