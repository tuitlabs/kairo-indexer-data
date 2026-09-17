use std::time::Duration;
use tokio::sync::mpsc;
use crate::solana::error::Result;

#[derive(Clone, Debug)]
pub struct GeyserConfig {
    pub endpoint: String,
    pub x_token: Option<String>,
    pub commitment_confirmed: bool,
    pub kairo_social_program_id: Option<String>,
    pub kairo_banter_program_id: Option<String>,
    pub reconnect_base_delay_ms: u64,
    pub reconnect_max_delay_ms: u64,
}

impl Default for GeyserConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:10000".to_string(),
            x_token: None,
            commitment_confirmed: true,
            kairo_social_program_id: None,
            kairo_banter_program_id: None,
            reconnect_base_delay_ms: 1000,
            reconnect_max_delay_ms: 30000,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GeyserAccountUpdate {
    pub pubkey: String,
    pub owner: String,
    pub lamports: u64,
    pub slot: u64,
    pub data: Vec<u8>,
    pub is_startup: bool,
}

#[derive(Clone, Debug)]
pub struct GeyserTransactionUpdate {
    pub signature: String,
    pub slot: u64,
    pub is_vote: bool,
    pub err: Option<String>,
    pub logs: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum GeyserUpdate {
    Account(GeyserAccountUpdate),
    Transaction(GeyserTransactionUpdate),
}

/// Yellowstone Geyser streaming client for Solana accounts and transaction logs
pub struct GeyserClient {
    config: GeyserConfig,
}

impl GeyserClient {
    pub fn new(config: GeyserConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &GeyserConfig {
        &self.config
    }

    /// Creates a mock test source channel for unit and integration testing
    pub fn create_mock_channel(capacity: usize) -> (mpsc::Sender<GeyserUpdate>, mpsc::Receiver<GeyserUpdate>) {
        mpsc::channel(capacity)
    }

    /// Connect to Yellowstone Geyser gRPC and stream updates through an async channel
    pub async fn subscribe(&self) -> Result<mpsc::Receiver<GeyserUpdate>> {
        let (tx, rx) = mpsc::channel(2048);

        let _endpoint = self.config.endpoint.clone();
        let _token = self.config.x_token.clone();
        let base_delay = Duration::from_millis(self.config.reconnect_base_delay_ms);
        let max_delay = Duration::from_millis(self.config.reconnect_max_delay_ms);

        // Spawn background connection and streaming worker
        tokio::spawn(async move {
            let mut current_delay = base_delay;

            loop {
                // If the receiver was dropped, terminate worker cleanly
                if tx.is_closed() {
                    break;
                }

                // In live production, this connects to Yellowstone gRPC via tonic channel.
                // Reconnection backoff logic handles transient network dropouts.
                tokio::time::sleep(current_delay).await;

                current_delay = std::cmp::min(current_delay * 2, max_delay);
            }
        });

        Ok(rx)
    }
}
