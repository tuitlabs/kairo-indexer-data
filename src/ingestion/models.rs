use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawLog {
    pub address: String,
    pub block_hash: Option<String>,
    pub block_number: Option<String>,
    pub data: String,
    pub log_index: Option<String>,
    pub topics: Vec<String>,
    pub transaction_hash: Option<String>,
    pub transaction_index: Option<String>,
}

impl RawLog {
    pub fn parse_block_number(&self) -> u64 {
        self.block_number
            .as_deref()
            .and_then(|s| s.strip_prefix("0x").unwrap_or(s).parse::<u64>().ok()
                .or_else(|| u64::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16).ok()))
            .unwrap_or(0)
    }

    pub fn parse_log_index(&self) -> u32 {
        self.log_index
            .as_deref()
            .and_then(|s| s.strip_prefix("0x").unwrap_or(s).parse::<u32>().ok()
                .or_else(|| u32::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16).ok()))
            .unwrap_or(0)
    }

    pub fn parse_tx_index(&self) -> u32 {
        self.transaction_index
            .as_deref()
            .and_then(|s| s.strip_prefix("0x").unwrap_or(s).parse::<u32>().ok()
                .or_else(|| u32::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16).ok()))
            .unwrap_or(0)
    }
}
