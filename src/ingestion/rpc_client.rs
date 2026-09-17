use serde_json::json;
use crate::error::{IndexerError, Result};
use super::models::RawLog;

pub struct RpcClient {
    http_url: String,
    client: reqwest::Client,
}

impl RpcClient {
    pub fn new(http_url: &str) -> Self {
        Self {
            http_url: http_url.to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn get_block_number(&self) -> Result<u64> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": "eth_blockNumber",
            "params": [],
            "id": 1
        });

        let resp = self.client.post(&self.http_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| IndexerError::Network(format!("RPC call failed: {}", e)))?;

        let json: serde_json::Value = resp.json()
            .await
            .map_err(|e| IndexerError::Network(format!("Failed to parse JSON response: {}", e)))?;

        let hex_str = json["result"].as_str()
            .ok_or_else(|| IndexerError::Network("Missing result in eth_blockNumber".into()))?;

        let clean = hex_str.strip_prefix("0x").unwrap_or(hex_str);
        u64::from_str_radix(clean, 16)
            .map_err(|e| IndexerError::Network(format!("Invalid block hex {}: {}", hex_str, e)))
    }

    pub async fn get_logs(
        &self,
        from_block: u64,
        to_block: u64,
        addresses: &[String],
    ) -> Result<Vec<RawLog>> {
        let mut filter = json!({
            "fromBlock": format!("0x{:x}", from_block),
            "toBlock": format!("0x{:x}", to_block),
        });

        if !addresses.is_empty() {
            filter["address"] = json!(addresses);
        }

        let payload = json!({
            "jsonrpc": "2.0",
            "method": "eth_getLogs",
            "params": [filter],
            "id": 2
        });

        let resp = self.client.post(&self.http_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| IndexerError::Network(format!("RPC eth_getLogs failed: {}", e)))?;

        let json: serde_json::Value = resp.json()
            .await
            .map_err(|e| IndexerError::Network(format!("Failed to parse JSON response: {}", e)))?;

        if let Some(err) = json.get("error") {
            return Err(IndexerError::Network(format!("RPC error response: {}", err)));
        }

        let logs: Vec<RawLog> = serde_json::from_value(json["result"].clone())
            .map_err(|e| IndexerError::Network(format!("Failed to deserialize logs: {}", e)))?;

        Ok(logs)
    }
}
