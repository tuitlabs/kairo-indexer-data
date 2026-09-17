use alloy::providers::{Provider, ProviderBuilder, RootProvider};
use alloy::rpc::client::WsConnect;
use alloy::rpc::types::eth::{Filter, Log};
use alloy::primitives::Address;
use std::str::FromStr;
use crate::error::{IndexerError, Result};

pub struct AlloyWsClient {
    provider: RootProvider,
}

impl AlloyWsClient {
    pub async fn connect(ws_url: &str) -> Result<Self> {
        let ws = WsConnect::new(ws_url);
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .connect_ws(ws)
            .await
            .map_err(|e| IndexerError::Network(format!("Failed to connect Alloy WS: {}", e)))?;

        Ok(Self { provider })
    }

    pub fn provider(&self) -> &RootProvider {
        &self.provider
    }

    /// Subscribe to logs matching target contract addresses
    pub async fn subscribe_logs(
        &self,
        target_addresses: &[String],
    ) -> Result<impl futures_util::Stream<Item = Log>> {
        let mut filter = Filter::new();
        if !target_addresses.is_empty() {
            let mut addrs = Vec::new();
            for a in target_addresses {
                if let Ok(addr) = Address::from_str(a) {
                    addrs.push(addr);
                }
            }
            if !addrs.is_empty() {
                filter = filter.address(addrs);
            }
        }

        let sub = self.provider.subscribe_logs(&filter).await
            .map_err(|e| IndexerError::Network(format!("Alloy log subscription failed: {}", e)))?;

        Ok(sub.into_stream())
    }
}
