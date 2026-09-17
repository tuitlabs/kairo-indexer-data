use chrono::Utc;
use sqlx::Row;
use sha2::{Digest, Sha256};
use kairo_indexer_data::db::Database;
use kairo_indexer_data::solana::{
    SocialEdge, RingTier, BanterMarket, BanterMarketStatus,
    BanterBetPlaced, SolanaProcessor, SOLANA_INTERNAL_CHAIN_ID,
};

#[test]
fn test_anchor_discriminators() {
    // 1. SocialEdge account discriminator: sha256("account:SocialEdge")[0..8]
    let mut h1 = Sha256::new();
    h1.update(b"account:SocialEdge");
    let res1 = h1.finalize();
    assert_eq!(SocialEdge::discriminator(), res1[..8]);

    // 2. BanterMarket account discriminator: sha256("account:BanterMarket")[0..8]
    let mut h2 = Sha256::new();
    h2.update(b"account:BanterMarket");
    let res2 = h2.finalize();
    assert_eq!(BanterMarket::discriminator(), res2[..8]);

    // 3. BanterBetPlaced event discriminator: sha256("event:BanterBetPlaced")[0..8]
    let mut h3 = Sha256::new();
    h3.update(b"event:BanterBetPlaced");
    let res3 = h3.finalize();
    assert_eq!(BanterBetPlaced::discriminator(), res3[..8]);
}

#[test]
fn test_social_edge_borsh_roundtrip_and_clamping() {
    let authority = [1u8; 32];
    let peer = [2u8; 32];

    // Case 1: InnerCircle with 5000 bps (50%) -> should clamp to 0.95
    let edge_inner = SocialEdge {
        authority,
        peer,
        ring_tier: RingTier::InnerCircle,
        weight_bps: 5000,
        interaction_count: 42,
        last_updated_slot: 1234567,
        bump: 254,
    };

    let serialized = edge_inner.to_bytes().expect("Failed to serialize SocialEdge");
    assert_eq!(serialized[..8], SocialEdge::discriminator());

    let deserialized = SocialEdge::from_bytes(&serialized).expect("Failed to deserialize SocialEdge");
    assert_eq!(deserialized, edge_inner);
    assert_eq!(deserialized.clamped_weight(), 0.95);
    assert!(deserialized.should_materialize_in_graph());

    // Boundary check InnerCircle [0.90, 1.00]
    let edge_inner_min = SocialEdge { weight_bps: 0, ..edge_inner.clone() };
    assert_eq!(edge_inner_min.clamped_weight(), 0.90);
    let edge_inner_max = SocialEdge { weight_bps: 10000, ..edge_inner.clone() };
    assert_eq!(edge_inner_max.clamped_weight(), 1.00);

    // Case 2: SocialRing with 5000 bps -> should clamp to 0.40
    let edge_social = SocialEdge {
        ring_tier: RingTier::SocialRing,
        weight_bps: 5000,
        ..edge_inner.clone()
    };
    assert_eq!(edge_social.clamped_weight(), 0.40);
    assert!(edge_social.should_materialize_in_graph());

    // Boundary check SocialRing [0.30, 0.50)
    let edge_social_min = SocialEdge { ring_tier: RingTier::SocialRing, weight_bps: 0, ..edge_inner.clone() };
    assert_eq!(edge_social_min.clamped_weight(), 0.30);
    let edge_social_max = SocialEdge { ring_tier: RingTier::SocialRing, weight_bps: 10000, ..edge_inner.clone() };
    assert!(edge_social_max.clamped_weight() < 0.50);

    // Case 3: Public -> 0.00, should not materialize
    let edge_public = SocialEdge {
        ring_tier: RingTier::Public,
        weight_bps: 8000,
        ..edge_inner.clone()
    };
    assert_eq!(edge_public.clamped_weight(), 0.00);
    assert!(!edge_public.should_materialize_in_graph());
}

#[test]
fn test_pda_derivation_and_verification() {
    let program_id = [9u8; 32];
    let authority = [3u8; 32];
    let peer = [4u8; 32];
    let bump = 253;

    let pda = SocialEdge::derive_pda_address(&program_id, &authority, &peer, bump);

    let edge = SocialEdge {
        authority,
        peer,
        ring_tier: RingTier::InnerCircle,
        weight_bps: 1000,
        interaction_count: 10,
        last_updated_slot: 1000,
        bump,
    };

    // Correct verification
    assert!(edge.verify_pda(&pda, &program_id).is_ok());

    // Mismatched pubkey or program ID fails
    let fake_pda = [0u8; 32];
    assert!(edge.verify_pda(&fake_pda, &program_id).is_err());

    // BanterMarket PDA check
    let market_id = [7u8; 32];
    let market_pda = BanterMarket::derive_pda_address(&program_id, &authority, &market_id, bump);
    let market = BanterMarket {
        curator: authority,
        market_id,
        stance_uri: "ipfs://test-stance".to_string(),
        status: BanterMarketStatus::Active,
        total_raw_capital: 1000000,
        total_effective_stake: 800000,
        created_at_slot: 500,
        created_at_timestamp: 1700000000,
        bump,
    };
    assert!(market.verify_pda(&market_pda, &program_id).is_ok());
    assert!(market.verify_pda(&fake_pda, &program_id).is_err());
}

#[test]
fn test_banter_bet_log_extraction() {
    let event = BanterBetPlaced {
        market_id: [11u8; 32],
        agent: [12u8; 32],
        trade_type: 0, // BUY
        raw_capital: 10000,
        token_amount: 5000,
        effective_stake: 8500,
        credibility_score: 850, // Scaled 100-1000
        execution_price: 2,
        friction_tax: 150,
    };

    let log_line = event.to_log_string().expect("Failed to serialize to log string");
    assert!(log_line.starts_with("Program data: "));

    // Parse single log line
    let extracted = BanterBetPlaced::from_log_line(&log_line)
        .expect("Failed to parse log line")
        .expect("Expected Some(event)");
    assert_eq!(extracted, event);

    // Parse simulated transaction log array with noise
    let logs = vec![
        "Program 11111111111111111111111111111111 invoke [1]".to_string(),
        "Program log: Instruction: PlaceBanterBet".to_string(),
        log_line.clone(),
        "Program 11111111111111111111111111111111 success".to_string(),
    ];

    let extracted_all = BanterBetPlaced::extract_from_logs(&logs).expect("Failed to extract logs");
    assert_eq!(extracted_all.len(), 1);
    assert_eq!(extracted_all[0], event);
}

#[tokio::test]
async fn test_solana_processor_postgres_end_to_end() {
    let db_url = "postgres://postgres@localhost:5432/kairo_indexer";
    let db = match Database::connect(db_url).await {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping live DB test (cannot connect to Postgres: {})", e);
            return;
        }
    };

    let processor = SolanaProcessor::new(db.clone(), None);
    assert_eq!(processor.chain_id(), SOLANA_INTERNAL_CHAIN_ID);

    let auth_bytes = [101u8; 32];
    let peer_bytes = [102u8; 32];
    let auth_b58 = bs58::encode(&auth_bytes).into_string();
    let peer_b58 = bs58::encode(&peer_bytes).into_string();

    let market_id_bytes = [77u8; 32];
    let market_id_hex = format!("0x{}", hex::encode(market_id_bytes));
    let agent_bytes = [88u8; 32];
    let agent_b58 = bs58::encode(&agent_bytes).into_string();

    // Clean up test data
    sqlx::query("DELETE FROM markets WHERE chain_id = $1 AND market_id = $2")
        .bind(SOLANA_INTERNAL_CHAIN_ID as i64)
        .bind(&market_id_hex)
        .execute(db.pool())
        .await
        .ok();

    sqlx::query("DELETE FROM social_edges WHERE chain_id = $1 AND authority = $2 AND peer = $3")
        .bind(SOLANA_INTERNAL_CHAIN_ID as i64)
        .bind(&auth_b58)
        .bind(&peer_b58)
        .execute(db.pool())
        .await
        .ok();

    // 1. Ingest SocialEdge at slot 100
    let edge_v1 = SocialEdge {
        authority: auth_bytes,
        peer: peer_bytes,
        ring_tier: RingTier::InnerCircle,
        weight_bps: 4000,
        interaction_count: 5,
        last_updated_slot: 100,
        bump: 255,
    };
    processor.process_social_edge("unused_pubkey", &edge_v1, 100).await
        .expect("Failed to process social edge v1");

    // Verify row in social_edges
    let row = sqlx::query("SELECT ring_tier, weight_bps, interaction_count, last_updated_slot FROM social_edges WHERE chain_id = $1 AND authority = $2 AND peer = $3")
        .bind(SOLANA_INTERNAL_CHAIN_ID as i64)
        .bind(&auth_b58)
        .bind(&peer_b58)
        .fetch_one(db.pool())
        .await
        .expect("Failed to fetch social_edges row");

    assert_eq!(row.get::<String, _>("ring_tier"), "INNER_CIRCLE");
    assert_eq!(row.get::<i32, _>("weight_bps"), 4000);
    assert_eq!(row.get::<i64, _>("last_updated_slot"), 100);

    // 2. Test slot gating: out-of-order slot 90 should NOT overwrite slot 100
    let edge_stale = SocialEdge {
        weight_bps: 9999,
        last_updated_slot: 90,
        ..edge_v1.clone()
    };
    processor.process_social_edge("unused_pubkey", &edge_stale, 90).await
        .expect("Slot gating execution failed");

    let row_stale_check = sqlx::query("SELECT weight_bps, last_updated_slot FROM social_edges WHERE chain_id = $1 AND authority = $2 AND peer = $3")
        .bind(SOLANA_INTERNAL_CHAIN_ID as i64)
        .bind(&auth_b58)
        .bind(&peer_b58)
        .fetch_one(db.pool())
        .await
        .expect("Failed to fetch row");
    assert_eq!(row_stale_check.get::<i32, _>("weight_bps"), 4000);
    assert_eq!(row_stale_check.get::<i64, _>("last_updated_slot"), 100);

    // 3. Newer slot 120 DOES overwrite
    let edge_v2 = SocialEdge {
        ring_tier: RingTier::SocialRing,
        weight_bps: 6500,
        interaction_count: 8,
        last_updated_slot: 120,
        ..edge_v1.clone()
    };
    processor.process_social_edge("unused_pubkey", &edge_v2, 120).await
        .expect("Failed to process social edge v2");

    let row_v2 = sqlx::query("SELECT ring_tier, weight_bps, last_updated_slot FROM social_edges WHERE chain_id = $1 AND authority = $2 AND peer = $3")
        .bind(SOLANA_INTERNAL_CHAIN_ID as i64)
        .bind(&auth_b58)
        .bind(&peer_b58)
        .fetch_one(db.pool())
        .await
        .expect("Failed to fetch row");
    assert_eq!(row_v2.get::<String, _>("ring_tier"), "SOCIAL_RING");
    assert_eq!(row_v2.get::<i32, _>("weight_bps"), 6500);
    assert_eq!(row_v2.get::<i64, _>("last_updated_slot"), 120);

    // 4. Ingest BanterMarket at slot 100
    let now = Utc::now();
    let market = BanterMarket {
        curator: auth_bytes,
        market_id: market_id_bytes,
        stance_uri: "ipfs://solana-banter-1".to_string(),
        status: BanterMarketStatus::Active,
        total_raw_capital: 0,
        total_effective_stake: 0,
        created_at_slot: 100,
        created_at_timestamp: now.timestamp(),
        bump: 254,
    };
    processor.process_banter_market("unused_pubkey", &market, 100, now).await
        .expect("Failed to process banter market");

    let m_row = sqlx::query("SELECT chain_id, tier, status, pool_address FROM markets WHERE chain_id = $1 AND market_id = $2")
        .bind(SOLANA_INTERNAL_CHAIN_ID as i64)
        .bind(&market_id_hex)
        .fetch_one(db.pool())
        .await
        .expect("Failed to fetch market row");
    assert_eq!(m_row.get::<i64, _>("chain_id"), SOLANA_INTERNAL_CHAIN_ID as i64);
    assert_eq!(m_row.get::<i16, _>("tier"), 3); // Tier III Banter
    assert_eq!(m_row.get::<String, _>("status"), "ACTIVE");
    assert!(m_row.get::<Option<String>, _>("pool_address").is_none());

    // 5. Ingest BUY BanterBetPlaced trade
    let sig1 = format!("sig_buy_{}", now.timestamp_millis());
    let bet_buy = BanterBetPlaced {
        market_id: market_id_bytes,
        agent: agent_bytes,
        trade_type: 0, // BUY
        raw_capital: 1000,
        token_amount: 500,
        effective_stake: 800,
        credibility_score: 800, // NON-NULL score from event
        execution_price: 2,
        friction_tax: 20,
    };
    processor.process_banter_bet(&bet_buy, &sig1, 101, 0, now).await
        .expect("Failed to process BUY bet");

    // Check agent_trades row
    let t_row = sqlx::query("SELECT credibility_score, raw_capital, token_amount, trade_type FROM agent_trades WHERE chain_id = $1 AND market_id = $2 AND tx_hash = $3")
        .bind(SOLANA_INTERNAL_CHAIN_ID as i64)
        .bind(&market_id_hex)
        .bind(&sig1)
        .fetch_one(db.pool())
        .await
        .expect("Failed to fetch trade row");
    assert_eq!(t_row.get::<i32, _>("credibility_score"), 800);
    assert_eq!(t_row.get::<String, _>("trade_type"), "BUY");

    // Check stance_positions row
    let pos_row = sqlx::query("SELECT token_balance, last_action, total_lifetime_flips FROM stance_positions WHERE chain_id = $1 AND market_id = $2 AND agent_address = $3")
        .bind(SOLANA_INTERNAL_CHAIN_ID as i64)
        .bind(&market_id_hex)
        .bind(&agent_b58)
        .fetch_one(db.pool())
        .await
        .expect("Failed to fetch stance_positions row");
    assert_eq!(pos_row.get::<String, _>("last_action"), "BUY");
    assert_eq!(pos_row.get::<i32, _>("total_lifetime_flips"), 0);

    // 6. Ingest SELL BanterBetPlaced trade -> Action reversal (flip)
    let sig2 = format!("sig_sell_{}", now.timestamp_millis());
    let bet_sell = BanterBetPlaced {
        market_id: market_id_bytes,
        agent: agent_bytes,
        trade_type: 1, // SELL
        raw_capital: 400,
        token_amount: 200,
        effective_stake: 320,
        credibility_score: 800,
        execution_price: 2,
        friction_tax: 10,
    };
    processor.process_banter_bet(&bet_sell, &sig2, 102, 0, now + chrono::Duration::seconds(5)).await
        .expect("Failed to process SELL bet");

    // Check position_flips table
    let flip_row = sqlx::query("SELECT previous_action, flip_action, token_amount FROM position_flips WHERE chain_id = $1 AND market_id = $2 AND agent_address = $3")
        .bind(SOLANA_INTERNAL_CHAIN_ID as i64)
        .bind(&market_id_hex)
        .bind(&agent_b58)
        .fetch_one(db.pool())
        .await
        .expect("Failed to fetch flip row");
    assert_eq!(flip_row.get::<String, _>("previous_action"), "BUY");
    assert_eq!(flip_row.get::<String, _>("flip_action"), "SELL");

    // Check updated stance_positions
    let pos_row2 = sqlx::query("SELECT last_action, total_lifetime_flips FROM stance_positions WHERE chain_id = $1 AND market_id = $2 AND agent_address = $3")
        .bind(SOLANA_INTERNAL_CHAIN_ID as i64)
        .bind(&market_id_hex)
        .bind(&agent_b58)
        .fetch_one(db.pool())
        .await
        .expect("Failed to fetch stance_positions row");
    assert_eq!(pos_row2.get::<String, _>("last_action"), "SELL");
    assert_eq!(pos_row2.get::<i32, _>("total_lifetime_flips"), 1);

    // 7. Check recalculated SVC on markets table
    let m_svc_row = sqlx::query("SELECT total_raw_capital, total_effective_stake, current_svc FROM markets WHERE chain_id = $1 AND market_id = $2")
        .bind(SOLANA_INTERNAL_CHAIN_ID as i64)
        .bind(&market_id_hex)
        .fetch_one(db.pool())
        .await
        .expect("Failed to fetch market svc");
    let svc: bigdecimal::BigDecimal = m_svc_row.get("current_svc");
    assert!(svc >= bigdecimal::BigDecimal::from(0));
}
