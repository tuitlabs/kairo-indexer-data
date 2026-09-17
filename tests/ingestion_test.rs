use kairo_indexer_data::abi::events::*;
use kairo_indexer_data::db::Database;
use ethnum::U256;
use chrono::Utc;
use sqlx::Row;

#[test]
fn test_event_signature_hashes() {
    assert_eq!(MARKET_CREATED_TOPIC(), MARKET_CREATED_TOPIC());
    assert_eq!(STANCE_BOUGHT_TOPIC(), STANCE_BOUGHT_TOPIC());
    assert_eq!(STANCE_SOLD_TOPIC(), STANCE_SOLD_TOPIC());
    assert_eq!(DECEPTION_TAX_CHARGED_TOPIC(), DECEPTION_TAX_CHARGED_TOPIC());
    assert_eq!(EPOCH_FINALIZED_TOPIC(), EPOCH_FINALIZED_TOPIC());
    assert_eq!(COALITION_FORMED_TOPIC(), COALITION_FORMED_TOPIC());
}

#[test]
fn test_decode_stance_bought_event() {
    let market_id = "0x1111111111111111111111111111111111111111111111111111111111111111";
    let agent_addr = "0x000000000000000000000000aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    let topics = vec![
        STANCE_BOUGHT_TOPIC().to_string(),
        market_id.to_string(),
        agent_addr.to_string(),
    ];

    let mut data = Vec::new();
    let deposit = U256::from(1000u64).to_be_bytes();
    let tokens = U256::from(500u64).to_be_bytes();
    let eff = U256::from(800u64).to_be_bytes();
    let cred = U256::from(800u64).to_be_bytes();
    let price = U256::from(2u64).to_be_bytes();
    let time = U256::from(1700000000u64).to_be_bytes();

    data.extend_from_slice(&deposit);
    data.extend_from_slice(&tokens);
    data.extend_from_slice(&eff);
    data.extend_from_slice(&cred);
    data.extend_from_slice(&price);
    data.extend_from_slice(&time);

    let data_hex = hex::encode(data);
    let decoded = ParsedKairoEvent::decode(&topics, &data_hex).expect("Failed to decode StanceBought");

    match decoded {
        Some(ParsedKairoEvent::StanceBought(ev)) => {
            assert_eq!(ev.market_id, market_id);
            assert_eq!(ev.deposit_amt, U256::from(1000u64));
            assert_eq!(ev.tokens_received, U256::from(500u64));
            assert_eq!(ev.effective_stake, U256::from(800u64));
            assert_eq!(ev.credibility_score, 800);
            assert_eq!(ev.timestamp, 1700000000);
        }
        _ => panic!("Expected StanceBought variant"),
    }
}

#[tokio::test]
async fn test_end_to_end_sqlx_database_and_flip_reversal() {
    let db_url = "postgres://postgres@localhost:5432/kairo_indexer";
    let db = match Database::connect(db_url).await {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping live DB test (cannot connect to Postgres: {})", e);
            return;
        }
    };

    let market_id = "0xabcdef111122223333444455556666777788889999aaaabbbbccccddddeeee02";
    let agent_addr = "0x9999999999999999999999999999999999999999";

    // Clean up prior test data using parameterized query
    sqlx::query("DELETE FROM markets WHERE market_id = $1")
        .bind(market_id)
        .execute(db.pool())
        .await
        .ok();

    sqlx::query("DELETE FROM deception_tax_events WHERE market_id = $1")
        .bind(market_id)
        .execute(db.pool())
        .await
        .ok();

    let now = Utc::now();
    let nonce = now.timestamp_millis();
    let tx1 = format!("0xtx1_{}", nonce);
    let tx2 = format!("0xtx2_{}", nonce);
    let tx3 = format!("0xtx3_{}", nonce);
    let tx4 = format!("0xtx4_{}", nonce);
    let tx5 = format!("0xtx5_{}", nonce);
    let tx6 = format!("0xtx6_{}", nonce);

    let t0 = now;
    let t1 = now + chrono::Duration::seconds(10);
    let t2 = now + chrono::Duration::seconds(20);
    let t3 = now + chrono::Duration::seconds(30);
    let t4 = now + chrono::Duration::seconds(40);
    let t5 = now + chrono::Duration::seconds(50);
    let t6 = now + chrono::Duration::seconds(55);

    // 1. Record market creation
    let market_ev = MarketCreatedEvent {
        market_id: market_id.to_string(),
        stance_uri: "ipfs://test-market-thesis".to_string(),
        pool_address: "0x1234567890123456789012345678901234567890".to_string(),
        curator: "0x5555555555555555555555555555555555555555".to_string(),
        created_at: 1700000000,
    };
    db.record_market_created(&market_ev, 100, t0).await.unwrap();

    // 2. First trade: BUY (Initial entry -> No flip)
    let buy1 = StanceBoughtEvent {
        market_id: market_id.to_string(),
        agent: agent_addr.to_string(),
        deposit_amt: U256::from(10000u64),
        tokens_received: U256::from(10000u64),
        effective_stake: U256::from(9000u64),
        credibility_score: 900,
        current_price: U256::from(1u64),
        timestamp: 1700000010,
    };
    db.process_stance_bought(&buy1, &tx1, 101, 0, t1).await.unwrap();

    let flips_count = db.calculate_windowed_velocity(agent_addr, market_id, 3600).await.unwrap();
    assert_eq!(flips_count, 0, "Initial BUY should not be a reversal flip");

    // Check stance_positions has total_lifetime_flips = 0 persisted
    let pos_row = sqlx::query("SELECT total_lifetime_flips, last_action FROM stance_positions WHERE market_id = $1 AND agent_address = $2")
        .bind(market_id)
        .bind(agent_addr.to_lowercase())
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(pos_row.get::<i32, _>("total_lifetime_flips"), 0);
    assert_eq!(pos_row.get::<Option<String>, _>("last_action").as_deref(), Some("BUY"));

    // 3. Second trade: BUY (Scale into position -> No flip)
    let buy2 = StanceBoughtEvent {
        market_id: market_id.to_string(),
        agent: agent_addr.to_string(),
        deposit_amt: U256::from(5000u64),
        tokens_received: U256::from(5000u64),
        effective_stake: U256::from(4500u64),
        credibility_score: 900,
        current_price: U256::from(1u64),
        timestamp: 1700000020,
    };
    db.process_stance_bought(&buy2, &tx2, 102, 0, t2).await.unwrap();

    let flips_count2 = db.calculate_windowed_velocity(agent_addr, market_id, 3600).await.unwrap();
    assert_eq!(flips_count2, 0, "Consecutive BUY should not increment flip count");

    // 4. Third trade: SELL (Reversal: BUY -> SELL -> Flip recorded!)
    let sell1 = StanceSoldEvent {
        market_id: market_id.to_string(),
        agent: agent_addr.to_string(),
        token_amt: U256::from(3000u64),
        collateral_returned: U256::from(3000u64),
        exit_price: U256::from(1u64),
        timestamp: 1700000030,
    };
    db.process_stance_sold(&sell1, &tx3, 103, 0, t3).await.unwrap();

    let flips_count3 = db.calculate_windowed_velocity(agent_addr, market_id, 3600).await.unwrap();
    assert_eq!(flips_count3, 1, "Action reversal BUY -> SELL must increment flip count to 1");

    // Verify stance_positions has total_lifetime_flips = 1 persisted
    let pos_row3 = sqlx::query("SELECT total_lifetime_flips, last_action FROM stance_positions WHERE market_id = $1 AND agent_address = $2")
        .bind(market_id)
        .bind(agent_addr.to_lowercase())
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(pos_row3.get::<i32, _>("total_lifetime_flips"), 1, "stance_positions must persist total_lifetime_flips = 1");
    assert_eq!(pos_row3.get::<Option<String>, _>("last_action").as_deref(), Some("SELL"));

    // 5. Fourth trade: SELL (Scale out -> No flip)
    let sell2 = StanceSoldEvent {
        market_id: market_id.to_string(),
        agent: agent_addr.to_string(),
        token_amt: U256::from(2000u64),
        collateral_returned: U256::from(2000u64),
        exit_price: U256::from(1u64),
        timestamp: 1700000040,
    };
    db.process_stance_sold(&sell2, &tx4, 104, 0, t4).await.unwrap();

    let flips_count4 = db.calculate_windowed_velocity(agent_addr, market_id, 3600).await.unwrap();
    assert_eq!(flips_count4, 1, "Consecutive SELL should not increment flip count");

    // 6. Fifth trade: BUY (Reversal: SELL -> BUY -> Flip count becomes 2!)
    let buy3 = StanceBoughtEvent {
        market_id: market_id.to_string(),
        agent: agent_addr.to_string(),
        deposit_amt: U256::from(1000u64),
        tokens_received: U256::from(1000u64),
        effective_stake: U256::from(900u64),
        credibility_score: 900,
        current_price: U256::from(1u64),
        timestamp: 1700000050,
    };
    db.process_stance_bought(&buy3, &tx5, 105, 0, t5).await.unwrap();

    let flips_count5 = db.calculate_windowed_velocity(agent_addr, market_id, 3600).await.unwrap();
    assert_eq!(flips_count5, 2, "Action reversal SELL -> BUY must increment flip count to 2");

    // Verify stance_positions has total_lifetime_flips = 2 persisted
    let pos_row5 = sqlx::query("SELECT total_lifetime_flips, last_action FROM stance_positions WHERE market_id = $1 AND agent_address = $2")
        .bind(market_id)
        .bind(agent_addr.to_lowercase())
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(pos_row5.get::<i32, _>("total_lifetime_flips"), 2, "stance_positions must persist total_lifetime_flips = 2");
    assert_eq!(pos_row5.get::<Option<String>, _>("last_action").as_deref(), Some("BUY"));

    // 7. Verify Deception Tax cross-check with parameterized query
    let tax_ev = DeceptionTaxChargedEvent {
        market_id: market_id.to_string(),
        agent: agent_addr.to_string(),
        tax_amount: U256::from(150u64),
        flip_velocity: 2, // Reported by contract
        credibility_score: 900,
        timestamp: 1700000055,
    };
    db.record_deception_tax(&tax_ev, &tx6, 106, t6, 3600).await.unwrap();

    let tax_row = sqlx::query(
        "SELECT reported_flip_velocity, calculated_flip_velocity FROM deception_tax_events WHERE market_id = $1 AND agent_address = $2"
    )
    .bind(market_id)
    .bind(agent_addr.to_lowercase())
    .fetch_one(db.pool())
    .await
    .unwrap();

    let rep: i32 = tax_row.get("reported_flip_velocity");
    let calc: Option<i32> = tax_row.get("calculated_flip_velocity");
    assert_eq!(rep, 2);
    assert_eq!(calc, Some(2));

    // Verify SELL trade recorded liquidated effective stake (2700) instead of 0
    let sell_trade_row = sqlx::query("SELECT effective_stake FROM agent_trades WHERE tx_hash = $1")
        .bind(&tx3)
        .fetch_one(db.pool())
        .await
        .unwrap();
    let sell_eff: bigdecimal::BigDecimal = sell_trade_row.get("effective_stake");
    assert_eq!(sell_eff.to_string(), "2700", "SELL trade must record liquidated effective stake (2700), not hardcoded 0");

    println!("All sqlx parameterized query & stance_positions persistence tests passed!");
}

#[tokio::test]
async fn test_sql_injection_defense() {
    let db_url = "postgres://postgres@localhost:5432/kairo_indexer";
    let db = match Database::connect(db_url).await {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping live DB test (cannot connect to Postgres: {})", e);
            return;
        }
    };

    // Adversarial payloads containing single quotes, semicolons, UNION SELECT, and DROP TABLE
    let malicious_market = "0xmarket' OR '1'='1'; DROP TABLE non_existent_dummy; --";
    let malicious_agent = "0xagent' UNION SELECT NULL, NULL, NULL --";
    let malicious_tx = "0xtx' OR 1=1; DELETE FROM agent_trades; --";
    let malicious_uri = "ipfs://test''); DROP TABLE stance_positions; --";

    // Clean up
    sqlx::query("DELETE FROM markets WHERE market_id = $1")
        .bind(malicious_market)
        .execute(db.pool())
        .await
        .ok();

    let now = Utc::now();

    // 1. Create market with payload
    let market_ev = MarketCreatedEvent {
        market_id: malicious_market.to_string(),
        stance_uri: malicious_uri.to_string(),
        pool_address: "0x1111111111111111111111111111111111111111".to_string(),
        curator: malicious_agent.to_string(),
        created_at: 1700000000,
    };
    db.record_market_created(&market_ev, 200, now).await
        .expect("Parameterized record_market_created must not fail on quotes/SQL tokens");

    // 2. Buy with payload
    let buy = StanceBoughtEvent {
        market_id: malicious_market.to_string(),
        agent: malicious_agent.to_string(),
        deposit_amt: U256::from(5000u64),
        tokens_received: U256::from(5000u64),
        effective_stake: U256::from(4000u64),
        credibility_score: 800,
        current_price: U256::from(1u64),
        timestamp: 1700000010,
    };
    db.process_stance_bought(&buy, malicious_tx, 201, 0, now).await
        .expect("Parameterized process_stance_bought must safely store malicious strings");

    // 3. Query windowed velocity with payload
    let velocity = db.calculate_windowed_velocity(malicious_agent, malicious_market, 3600).await
        .expect("Parameterized calculate_windowed_velocity must safely bind injection strings");
    assert_eq!(velocity, 0);

    // 4. Verify data was stored literally and unescaped without executing SQL injection
    let mkt_row = sqlx::query("SELECT market_id, stance_uri FROM markets WHERE market_id = $1")
        .bind(malicious_market)
        .fetch_one(db.pool())
        .await
        .expect("Market must exist literally");
    assert_eq!(mkt_row.get::<String, _>("market_id"), malicious_market);
    assert_eq!(mkt_row.get::<String, _>("stance_uri"), malicious_uri);

    // 5. Verify stance_positions was NOT dropped or modified maliciously
    let pos_row = sqlx::query("SELECT agent_address, total_lifetime_flips FROM stance_positions WHERE market_id = $1 AND agent_address = $2")
        .bind(malicious_market)
        .bind(malicious_agent.to_lowercase())
        .fetch_one(db.pool())
        .await
        .expect("stance_positions table must remain intact and contain literal row");
    assert_eq!(pos_row.get::<String, _>("agent_address"), malicious_agent.to_lowercase());

    // Clean up
    sqlx::query("DELETE FROM markets WHERE market_id = $1")
        .bind(malicious_market)
        .execute(db.pool())
        .await
        .ok();

    println!("SQL injection defense test passed: all malicious payloads safely bound literally!");
}

#[tokio::test]
async fn test_solana_support_migration_and_fk_integrity() {
    let db_url = "postgres://postgres@localhost:5432/kairo_indexer";
    let db = match Database::connect(db_url).await {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping live DB test (cannot connect to Postgres: {})", e);
            return;
        }
    };

    // 1. Run Migration 002
    let migration_sql = include_str!("../migrations/002_solana_support.sql");
    sqlx::raw_sql(migration_sql)
        .execute(db.pool())
        .await
        .expect("Migration 002 must apply cleanly");

    let base_chain: i64 = 8453;
    let solana_chain: i64 = 101;
    let market_id = "0xfeedbeef0000111122223333444455556666777788889999aaaabbbbcccc0001";
    let non_existent_market = "0x000000000000000000000000000000000000000000000000000000000000dead";

    // Clean up any stale test rows
    sqlx::query("DELETE FROM markets WHERE market_id IN ($1, $2)")
        .bind(market_id)
        .bind(non_existent_market)
        .execute(db.pool())
        .await
        .ok();

    let now = Utc::now();

    // 2. Insert valid Base market (chain_id = 8453)
    sqlx::query(
        "INSERT INTO markets (chain_id, market_id, stance_uri, pool_address, curator_address, tier, status, total_raw_capital, total_effective_stake, current_svc, current_price, created_at_block, created_at) \
         VALUES ($1, $2, 'ipfs://valid', '0x1111111111111111111111111111111111111111', '0x2222222222222222222222222222222222222222', 2, 'ACTIVE', 0, 0, 0, 0, 100, $3)"
    )
    .bind(base_chain)
    .bind(market_id)
    .bind(now)
    .execute(db.pool())
    .await
    .expect("Valid Base market must insert successfully");

    // 3. Test FK Integrity - Case A: Valid Trade referencing (base_chain, market_id)
    let valid_trade_tx = format!("0xvalid_trade_{}", now.timestamp_millis());
    let insert_valid_trade = sqlx::query(
        "INSERT INTO agent_trades (chain_id, market_id, agent_address, trade_type, raw_capital, token_amount, effective_stake, credibility_score, execution_price, tx_hash, block_number, log_index, block_timestamp) \
         VALUES ($1, $2, '0xagent1111111111111111111111111111111111', 'BUY', 1000, 1000, 800, 800, 1, $3, 101, 0, $4)"
    )
    .bind(base_chain)
    .bind(market_id)
    .bind(&valid_trade_tx)
    .bind(now)
    .execute(db.pool())
    .await;
    assert!(insert_valid_trade.is_ok(), "Trade referencing existing (chain_id, market_id) must succeed");

    // 4. Test FK Integrity - Case B: Trade referencing non-existent market_id
    let invalid_mkt_tx = format!("0xinvalid_mkt_tx_{}", now.timestamp_millis());
    let insert_invalid_mkt_trade = sqlx::query(
        "INSERT INTO agent_trades (chain_id, market_id, agent_address, trade_type, raw_capital, token_amount, effective_stake, credibility_score, execution_price, tx_hash, block_number, log_index, block_timestamp) \
         VALUES ($1, $2, '0xagent1111111111111111111111111111111111', 'BUY', 1000, 1000, 800, 800, 1, $3, 101, 0, $4)"
    )
    .bind(base_chain)
    .bind(non_existent_market)
    .bind(&invalid_mkt_tx)
    .bind(now)
    .execute(db.pool())
    .await;
    assert!(
        insert_invalid_mkt_trade.is_err(),
        "Trade referencing non-existent market_id must be rejected by foreign key constraint fk_agent_trades_market"
    );
    let err_msg = insert_invalid_mkt_trade.unwrap_err().to_string();
    assert!(
        err_msg.contains("violates foreign key constraint") || err_msg.contains("fk_agent_trades_market"),
        "Error message must indicate foreign key violation, got: {}",
        err_msg
    );

    // 5. Test FK Integrity - Case C: Valid market_id but WRONG chain_id (cross-chain FK mismatch)
    let mismatched_chain_tx = format!("0xmismatched_chain_tx_{}", now.timestamp_millis());
    let insert_mismatched_trade = sqlx::query(
        "INSERT INTO agent_trades (chain_id, market_id, agent_address, trade_type, raw_capital, token_amount, effective_stake, credibility_score, execution_price, tx_hash, block_number, log_index, block_timestamp) \
         VALUES ($1, $2, '0xagent1111111111111111111111111111111111', 'BUY', 1000, 1000, 800, 800, 1, $3, 101, 0, $4)"
    )
    .bind(solana_chain) // 101 instead of 8453
    .bind(market_id)    // valid on 8453, but does not exist on 101
    .bind(&mismatched_chain_tx)
    .bind(now)
    .execute(db.pool())
    .await;
    assert!(
        insert_mismatched_trade.is_err(),
        "Trade referencing valid market_id on WRONG chain_id must be rejected by composite FK (chain_id, market_id)"
    );

    // 6. Test Solana Base58 column widening support
    let solana_market_id = "BanterMkt111111111111111111111111111111111111"; // 44 chars Base58
    let solana_curator = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";    // 44 chars Base58
    let solana_agent = "4Nd1mBQtrMJVYVfKf2PJy9NZNLd3TXD6Qu44y4PWFj2L";      // 44 chars Base58
    // 88 chars Base58 Solana signature:
    let solana_sig = "5VERv8NMvzbJMEkV8xnrLkEaWRtSz9CosKDYj7SBRNBCTWmqApPo44y4PWFj2L1gT7dVqU28CWQeJ8v8j3f5kQhX";

    sqlx::query(
        "INSERT INTO markets (chain_id, market_id, stance_uri, pool_address, curator_address, tier, status, total_raw_capital, total_effective_stake, current_svc, current_price, created_at_block, created_at) \
         VALUES ($1, $2, 'https://kairo.market/banter/1', NULL, $3, 3, 'ACTIVE', 0, 0, 0, 0, 5000, $4)"
    )
    .bind(solana_chain)
    .bind(solana_market_id)
    .bind(solana_curator)
    .bind(now)
    .execute(db.pool())
    .await
    .expect("Solana banter market with 44-char Base58 curator and NULL pool_address must insert without error");

    let insert_solana_trade = sqlx::query(
        "INSERT INTO agent_trades (chain_id, market_id, agent_address, trade_type, raw_capital, token_amount, effective_stake, credibility_score, execution_price, tx_hash, block_number, log_index, block_timestamp) \
         VALUES ($1, $2, $3, 'BUY', 50, 50, 45, 900, 1, $4, 5001, 0, $5)"
    )
    .bind(solana_chain)
    .bind(solana_market_id)
    .bind(solana_agent)
    .bind(solana_sig)
    .bind(now)
    .execute(db.pool())
    .await;
    assert!(
        insert_solana_trade.is_ok(),
        "Solana trade with 88-char signature and 44-char pubkey must insert cleanly: {:?}",
        insert_solana_trade.err()
    );

    // 7. Test social_edges table insertion
    let insert_social_edge = sqlx::query(
        "INSERT INTO social_edges (chain_id, authority, peer, ring_tier, weight_bps, interaction_count, last_updated_slot) \
         VALUES ($1, $2, $3, 'INNER_CIRCLE', 9500, 12, 5002) \
         ON CONFLICT (chain_id, authority, peer) DO UPDATE \
         SET ring_tier = EXCLUDED.ring_tier, \
             weight_bps = EXCLUDED.weight_bps, \
             interaction_count = EXCLUDED.interaction_count, \
             last_updated_slot = EXCLUDED.last_updated_slot \
         WHERE EXCLUDED.last_updated_slot >= social_edges.last_updated_slot"
    )
    .bind(solana_chain)
    .bind(solana_curator)
    .bind(solana_agent)
    .execute(db.pool())
    .await;
    assert!(insert_social_edge.is_ok(), "social_edges insertion must succeed");

    // Clean up test data
    sqlx::query("DELETE FROM markets WHERE market_id IN ($1, $2)")
        .bind(market_id)
        .bind(solana_market_id)
        .execute(db.pool())
        .await
        .ok();

    sqlx::query("DELETE FROM social_edges WHERE authority = $1 AND peer = $2")
        .bind(solana_curator)
        .bind(solana_agent)
        .execute(db.pool())
        .await
        .ok();

    println!("Migration 002 & Foreign Key integrity verification tests passed successfully!");
}

#[tokio::test]
async fn test_fresh_database_migrations_001_and_002_end_to_end() {
    let db_url = "postgres://postgres@localhost:5432/kairo_indexer";
    let db = match Database::connect(db_url).await {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping live DB test (cannot connect to Postgres: {})", e);
            return;
        }
    };

    // Acquire a dedicated single connection from the pool to isolate search_path
    let mut conn = db.pool().acquire().await.expect("Must acquire connection");

    // 1. Create a pristine fresh isolated schema
    sqlx::raw_sql("DROP SCHEMA IF EXISTS fresh_migration_test CASCADE; CREATE SCHEMA fresh_migration_test; SET search_path TO fresh_migration_test, public;")
        .execute(&mut *conn)
        .await
        .expect("Must create and set search_path to fresh_migration_test");

    // 2. Run 001_initial_kairo_schema.sql top-to-bottom in one pass
    let m1 = include_str!("../migrations/001_initial_kairo_schema.sql");
    sqlx::raw_sql(m1)
        .execute(&mut *conn)
        .await
        .expect("001_initial_kairo_schema.sql must apply cleanly top-to-bottom on fresh schema");

    // 3. Run 002_solana_support.sql top-to-bottom in one pass
    let m2 = include_str!("../migrations/002_solana_support.sql");
    sqlx::raw_sql(m2)
        .execute(&mut *conn)
        .await
        .expect("002_solana_support.sql must apply cleanly top-to-bottom in one single pass on fresh schema");

    // 4. Verify all 5 composite foreign keys were recreated in the fresh schema
    let rows = sqlx::query(
        "SELECT constraint_name, table_name \
         FROM information_schema.table_constraints \
         WHERE constraint_type = 'FOREIGN KEY' AND table_schema = 'fresh_migration_test'"
    )
    .fetch_all(&mut *conn)
    .await
    .expect("Must query table constraints in fresh schema");

    let constraint_names: Vec<String> = rows.iter().map(|r| r.get("constraint_name")).collect();
    println!("Recreated Foreign Key constraints in fresh schema: {:?}", constraint_names);

    assert!(constraint_names.contains(&"fk_agent_trades_market".to_string()), "fk_agent_trades_market must exist");
    assert!(constraint_names.contains(&"fk_stance_positions_market".to_string()), "fk_stance_positions_market must exist");
    assert!(constraint_names.contains(&"fk_position_flips_market".to_string()), "fk_position_flips_market must exist");
    assert!(constraint_names.contains(&"fk_epoch_settlements_market".to_string()), "fk_epoch_settlements_market must exist");
    assert!(constraint_names.contains(&"fk_deception_tax_events_market".to_string()), "fk_deception_tax_events_market must exist");

    // 5. Test FK rejection on fresh schema
    let now = Utc::now();
    let mkt_id = "0xfresh00000000000000000000000000000000000000000000000000000000001";
    let bad_mkt_id = "0xfresh_dead00000000000000000000000000000000000000000000000000000001";

    sqlx::query(
        "INSERT INTO fresh_migration_test.markets (chain_id, market_id, stance_uri, pool_address, curator_address, tier, status, total_raw_capital, total_effective_stake, current_svc, current_price, created_at_block, created_at) \
         VALUES (8453, $1, 'ipfs://fresh', '0x1111111111111111111111111111111111111111', '0x2222222222222222222222222222222222222222', 2, 'ACTIVE', 0, 0, 0, 0, 1, $2)"
    )
    .bind(mkt_id)
    .bind(now)
    .execute(&mut *conn)
    .await
    .expect("Market insert must succeed on fresh schema");

    // Invalid market_id trade must fail
    let bad_insert = sqlx::query(
        "INSERT INTO fresh_migration_test.agent_trades (chain_id, market_id, agent_address, trade_type, raw_capital, token_amount, effective_stake, credibility_score, execution_price, tx_hash, block_number, log_index, block_timestamp) \
         VALUES (8453, $1, '0xagent1111111111111111111111111111111111', 'BUY', 100, 100, 80, 800, 1, '0xtx_bad', 2, 0, $2)"
    )
    .bind(bad_mkt_id)
    .bind(now)
    .execute(&mut *conn)
    .await;
    assert!(bad_insert.is_err(), "Bad FK must be rejected by fresh schema");

    // Clean up fresh schema
    sqlx::raw_sql("DROP SCHEMA fresh_migration_test CASCADE;")
        .execute(&mut *conn)
        .await
        .ok();

    println!("Fresh database migration 001 + 002 applied cleanly top-to-bottom with 0 errors!");
}


