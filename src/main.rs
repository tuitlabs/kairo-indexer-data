pub mod error;
pub mod config;
pub mod abi;
pub mod db;
pub mod ingestion;

use std::time::Duration;
use futures_util::StreamExt;
use crate::config::Config;
use crate::db::Database;
use crate::ingestion::{EventProcessor, RpcClient, AlloyWsClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Kairo Protocol: Base EVM Ingestion Worker (sqlx + alloy) ===");
    
    let config = Config::from_env()?;
    println!("Chain ID: {}", config.chain_id);
    println!("Database URL: {}", config.database_url);
    println!("WebSocket RPC: {}", config.ws_rpc_url);
    println!("HTTP RPC: {}", config.http_rpc_url);

    let targets = config.target_contracts();
    if targets.is_empty() {
        println!("No contract filter specified; listening to all protocol events");
    } else {
        println!("Tracking {} contract address(es): {:?}", targets.len(), targets);
    }

    // 1. Initialize DB connection with sqlx PgPool
    println!("Connecting to PostgreSQL pool via sqlx...");
    let db = Database::connect(&config.database_url).await?;
    println!("Database pool connection established successfully.");

    let processor = EventProcessor::new(db.clone(), config.chain_id);
    let rpc_client = RpcClient::new(&config.http_rpc_url);

    // 2. Check sync cursor and historical catch-up
    let last_indexed = db.get_last_indexed_block(config.chain_id).await?.unwrap_or(config.start_block);
    println!("Last indexed block: {}", last_indexed);

    if let Ok(latest_block) = rpc_client.get_block_number().await {
        println!("Current network block head: {}", latest_block);
        if latest_block > last_indexed {
            println!("Catching up {} blocks (from {} to {})...", latest_block - last_indexed, last_indexed + 1, latest_block);
            let mut from = last_indexed + 1;
            while from <= latest_block {
                let to = std::cmp::min(from + config.max_batch_size, latest_block);
                println!("Fetching historical logs from block {} to {}...", from, to);
                match rpc_client.get_logs(from, to, &targets).await {
                    Ok(logs) => {
                        println!("Fetched {} logs in range [{}, {}]", logs.len(), from, to);
                        for log in logs {
                            if let Ok(Some(ev_name)) = processor.process_log(&log).await {
                                println!("Processed historical event: {}", ev_name);
                            }
                        }
                        db.update_sync_state(config.chain_id, to, "0x0").await?;
                    }
                    Err(e) => {
                        eprintln!("Warning: Historical log catch-up error: {}. Proceeding to live stream.", e);
                        break;
                    }
                }
                from = to + 1;
            }
        }
    } else {
        println!("HTTP RPC head check skipped or offline; proceeding to WebSocket listener.");
    }

    // 3. Live WebSocket listener loop with Alloy and exponential backoff
    let mut delay_ms = config.reconnect_base_delay_ms;
    loop {
        println!("Connecting to Base WebSocket via Alloy: {} ...", config.ws_rpc_url);
        match AlloyWsClient::connect(&config.ws_rpc_url).await {
            Ok(ws_client) => {
                println!("Connected to Alloy WebSocket. Subscribing to contract event logs...");
                match ws_client.subscribe_logs(&targets).await {
                    Ok(mut stream) => {
                        println!("Alloy log subscription active. Listening for real-time blocks & events...");
                        delay_ms = config.reconnect_base_delay_ms;

                        while let Some(log) = stream.next().await {
                            match processor.process_alloy_log(&log).await {
                                Ok(Some(name)) => {
                                    println!(
                                        "Indexed [{}] at block {} tx {:?}",
                                        name,
                                        log.block_number.unwrap_or(0),
                                        log.transaction_hash
                                    );
                                }
                                Ok(None) => {}
                                Err(e) => eprintln!("Error processing log: {}", e),
                            }
                        }
                        eprintln!("Alloy WebSocket stream ended.");
                    }
                    Err(e) => eprintln!("Subscription failed: {}", e),
                }
            }
            Err(e) => {
                eprintln!("Alloy WebSocket connection failed: {}", e);
            }
        }

        println!("Reconnecting in {}ms...", delay_ms);
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        delay_ms = std::cmp::min(delay_ms * 2, config.reconnect_max_delay_ms);
    }
}
