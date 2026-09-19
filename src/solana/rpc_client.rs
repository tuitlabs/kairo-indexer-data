use base64::prelude::*;
use serde_json::json;
use crate::solana::error::{SolanaError, Result};

/// An account entry returned by `getProgramAccounts`.
#[derive(Clone, Debug)]
pub struct RpcAccountEntry {
    pub pubkey: String,
    pub owner: String,
    pub lamports: u64,
    pub data: Vec<u8>,
}

/// A parsed snapshot response from `getProgramAccounts` containing the snapshot slot.
#[derive(Clone, Debug)]
pub struct ProgramAccountsResponse {
    pub slot: u64,
    pub accounts: Vec<RpcAccountEntry>,
}

/// HTTP JSON-RPC 2.0 client for Solana historical backfill and snapshot queries.
pub struct SolanaRpcClient {
    http_url: String,
    client: reqwest::Client,
}

impl SolanaRpcClient {
    pub fn new(http_url: &str) -> Self {
        Self {
            http_url: http_url.to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub fn http_url(&self) -> &str {
        &self.http_url
    }

    /// Query the current confirmed slot from the Solana JSON-RPC endpoint.
    pub async fn get_slot(&self) -> Result<u64> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getSlot",
            "params": [{"commitment": "confirmed"}]
        });

        let resp = self.client.post(&self.http_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| SolanaError::Rpc(format!("getSlot network error: {}", e)))?;

        let json: serde_json::Value = resp.json()
            .await
            .map_err(|e| SolanaError::Rpc(format!("Failed to parse getSlot JSON: {}", e)))?;

        if let Some(err) = json.get("error") {
            return Err(SolanaError::Rpc(format!("getSlot RPC error: {}", err)));
        }

        json["result"].as_u64()
            .ok_or_else(|| SolanaError::Rpc("Missing result in getSlot response".into()))
    }

    /// Parse a JSON-RPC response from `getProgramAccounts`.
    /// Handles both `withContext: true` (object with slot) and standard array formats.
    pub fn parse_program_accounts_response(
        json: &serde_json::Value,
        fallback_slot: u64,
    ) -> Result<ProgramAccountsResponse> {
        if let Some(err) = json.get("error") {
            return Err(SolanaError::Rpc(format!("getProgramAccounts RPC error: {}", err)));
        }

        let result = json.get("result")
            .ok_or_else(|| SolanaError::Rpc("Missing result in getProgramAccounts response".into()))?;

        let (slot, value_array) = if let Some(context) = result.get("context") {
            let slot = context.get("slot").and_then(|s| s.as_u64()).unwrap_or(fallback_slot);
            let val = result.get("value").and_then(|v| v.as_array())
                .ok_or_else(|| SolanaError::Rpc("Missing 'value' array in withContext getProgramAccounts response".into()))?;
            (slot, val)
        } else if let Some(arr) = result.as_array() {
            (fallback_slot, arr)
        } else {
            return Err(SolanaError::Rpc("Unexpected result format in getProgramAccounts".into()));
        };

        let mut accounts = Vec::new();
        for item in value_array {
            let pubkey = item.get("pubkey")
                .and_then(|p| p.as_str())
                .ok_or_else(|| SolanaError::Rpc("Missing pubkey in getProgramAccounts entry".into()))?
                .to_string();

            let account_obj = item.get("account")
                .ok_or_else(|| SolanaError::Rpc("Missing account in getProgramAccounts entry".into()))?;

            let owner = account_obj.get("owner")
                .and_then(|o| o.as_str())
                .unwrap_or_default()
                .to_string();

            let lamports = account_obj.get("lamports")
                .and_then(|l| l.as_u64())
                .unwrap_or(0);

            let data_val = account_obj.get("data")
                .ok_or_else(|| SolanaError::Rpc("Missing data in account entry".into()))?;

            let data_bytes = if let Some(arr) = data_val.as_array() {
                // ["<base64>", "base64"]
                let b64_str = arr.first()
                    .and_then(|s| s.as_str())
                    .ok_or_else(|| SolanaError::Rpc("Empty data array in account entry".into()))?;
                BASE64_STANDARD.decode(b64_str)?
            } else if let Some(b64_str) = data_val.as_str() {
                BASE64_STANDARD.decode(b64_str)?
            } else {
                return Err(SolanaError::Rpc("Invalid data field type in account entry".into()));
            };

            accounts.push(RpcAccountEntry {
                pubkey,
                owner,
                lamports,
                data: data_bytes,
            });
        }

        Ok(ProgramAccountsResponse { slot, accounts })
    }

    /// Fetch all accounts owned by a program ID using `getProgramAccounts` with Base64 encoding.
    pub async fn get_program_accounts(&self, program_id: &str) -> Result<ProgramAccountsResponse> {
        let current_slot = self.get_slot().await.unwrap_or(0);

        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getProgramAccounts",
            "params": [
                program_id,
                {
                    "encoding": "base64",
                    "commitment": "confirmed",
                    "withContext": true
                }
            ]
        });

        let resp = self.client.post(&self.http_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| SolanaError::Rpc(format!("getProgramAccounts network error: {}", e)))?;

        let json: serde_json::Value = resp.json()
            .await
            .map_err(|e| SolanaError::Rpc(format!("Failed to parse getProgramAccounts JSON: {}", e)))?;

        Self::parse_program_accounts_response(&json, current_slot)
    }
}
