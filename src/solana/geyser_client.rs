use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use futures_util::StreamExt;
use yellowstone_grpc_proto::geyser::{
    geyser_client::GeyserClient as ProtoGeyserClient,
    subscribe_update::UpdateOneof,
    CommitmentLevel,
    SubscribeRequest,
    SubscribeRequestFilterAccounts,
    SubscribeRequestFilterTransactions,
};
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

    /// Construct a Yellowstone SubscribeRequest filtered by protocol program IDs at Confirmed commitment
    pub fn build_subscribe_request(&self) -> SubscribeRequest {
        let mut owners = Vec::new();
        if let Some(ref pid) = self.config.kairo_social_program_id {
            owners.push(pid.clone());
        }
        if let Some(ref pid) = self.config.kairo_banter_program_id {
            owners.push(pid.clone());
        }

        let mut accounts = HashMap::new();
        if !owners.is_empty() {
            accounts.insert(
                "kairo_program_accounts".to_string(),
                SubscribeRequestFilterAccounts {
                    account: vec![],
                    owner: owners.clone(),
                    filters: vec![],
                    nonempty_txn_signature: None,
                    ..Default::default()
                },
            );
        }

        let mut transactions = HashMap::new();
        if !owners.is_empty() {
            transactions.insert(
                "kairo_program_txs".to_string(),
                SubscribeRequestFilterTransactions {
                    vote: Some(false),
                    failed: Some(false),
                    signature: None,
                    account_include: owners,
                    account_exclude: vec![],
                    account_required: vec![],
                    ..Default::default()
                },
            );
        }

        let commitment = if self.config.commitment_confirmed {
            Some(CommitmentLevel::Confirmed as i32)
        } else {
            Some(CommitmentLevel::Processed as i32)
        };

        SubscribeRequest {
            accounts,
            slots: HashMap::new(),
            transactions,
            transactions_status: HashMap::new(),
            blocks: HashMap::new(),
            blocks_meta: HashMap::new(),
            entry: HashMap::new(),
            commitment,
            accounts_data_slice: vec![],
            ping: None,
            from_slot: None,
        }
    }

    /// Connect to Yellowstone Geyser gRPC and stream updates through an async channel
    pub async fn subscribe(&self) -> Result<mpsc::Receiver<GeyserUpdate>> {
        let (tx, rx) = mpsc::channel(2048);

        let endpoint = self.config.endpoint.clone();
        let x_token = self.config.x_token.clone();
        let request = self.build_subscribe_request();
        let base_delay = Duration::from_millis(self.config.reconnect_base_delay_ms);
        let max_delay = Duration::from_millis(self.config.reconnect_max_delay_ms);

        tokio::spawn(async move {
            let mut current_delay = base_delay;

            loop {
                if tx.is_closed() {
                    break;
                }

                // 1. Establish gRPC channel to Yellowstone Geyser endpoint
                let channel_res = match tonic::transport::Endpoint::from_shared(endpoint.clone()) {
                    Ok(ep) => ep.connect().await,
                    Err(e) => {
                        eprintln!("Invalid Yellowstone endpoint URI '{}': {}", endpoint, e);
                        tokio::time::sleep(current_delay).await;
                        current_delay = std::cmp::min(current_delay * 2, max_delay);
                        continue;
                    }
                };

                let channel = match channel_res {
                    Ok(ch) => ch,
                    Err(e) => {
                        eprintln!(
                            "Failed to connect to Yellowstone gRPC at {}: {}. Retrying in {:?}...",
                            endpoint, e, current_delay
                        );
                        tokio::time::sleep(current_delay).await;
                        current_delay = std::cmp::min(current_delay * 2, max_delay);
                        continue;
                    }
                };

                let mut client = ProtoGeyserClient::new(channel);

                // 2. Build streaming SubscribeRequest with optional x-token header
                let req_clone = request.clone();
                let req_stream = futures_util::stream::once(async move { req_clone });

                let mut grpc_req = tonic::Request::new(req_stream);
                if let Some(ref token) = x_token {
                    if let Ok(val) = token.parse() {
                        grpc_req.metadata_mut().insert("x-token", val);
                    }
                }

                // 3. Initiate subscription stream
                match client.subscribe(grpc_req).await {
                    Ok(response) => {
                        current_delay = base_delay; // Reset backoff on connection success
                        let mut stream = response.into_inner();

                        while let Some(msg_res) = stream.next().await {
                            if tx.is_closed() {
                                return;
                            }

                            match msg_res {
                                Ok(update) => {
                                    if let Some(update_oneof) = update.update_oneof {
                                        match update_oneof {
                                            UpdateOneof::Account(acc_update) => {
                                                if let Some(info) = acc_update.account {
                                                    let pubkey = bs58::encode(&info.pubkey).into_string();
                                                    let owner = bs58::encode(&info.owner).into_string();
                                                    let parsed = GeyserUpdate::Account(GeyserAccountUpdate {
                                                        pubkey,
                                                        owner,
                                                        lamports: info.lamports,
                                                        slot: acc_update.slot,
                                                        data: info.data,
                                                        is_startup: acc_update.is_startup,
                                                    });
                                                    if tx.send(parsed).await.is_err() {
                                                        return;
                                                    }
                                                }
                                            }
                                            UpdateOneof::Transaction(tx_update) => {
                                                if let Some(info) = tx_update.transaction {
                                                    let signature = bs58::encode(&info.signature).into_string();
                                                    let (logs, err) = match info.meta {
                                                        Some(meta) => (
                                                            meta.log_messages,
                                                            meta.err.map(|e| format!("{:?}", e)),
                                                        ),
                                                        None => (Vec::new(), None),
                                                    };
                                                    let parsed = GeyserUpdate::Transaction(GeyserTransactionUpdate {
                                                        signature,
                                                        slot: tx_update.slot,
                                                        is_vote: info.is_vote,
                                                        err,
                                                        logs,
                                                    });
                                                    if tx.send(parsed).await.is_err() {
                                                        return;
                                                    }
                                                }
                                            }
                                            UpdateOneof::Ping(_) => {
                                                // Keepalive ping from server
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                Err(status) => {
                                    eprintln!("Yellowstone gRPC stream error: {}. Reconnecting...", status);
                                    break;
                                }
                            }
                        }
                    }
                    Err(status) => {
                        eprintln!(
                            "Yellowstone subscribe call rejected: {}. Retrying in {:?}...",
                            status, current_delay
                        );
                    }
                }

                tokio::time::sleep(current_delay).await;
                current_delay = std::cmp::min(current_delay * 2, max_delay);
            }
        });

        Ok(rx)
    }
}
