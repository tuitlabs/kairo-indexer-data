use kairo_indexer_data::config::*;
use kairo_indexer_data::graph::client::Neo4jClient;
use kairo_indexer_data::graph::sync::sync_coalition_graph;
use kairo_indexer_data::db::Database;
use kairo_indexer_data::abi::events::{CoalitionFormedEvent, CoalitionMemberJoinedEvent};
use chrono::Utc;

#[test]
fn test_provisional_graph_weight_constants() {
    assert_eq!(PROVISIONAL_COALITION_BASE_WEIGHT, 0.50);
    assert_eq!(PROVISIONAL_COALITION_MAX_WEIGHT, 0.90);
    assert_eq!(PROVISIONAL_COALITION_SHARE_SCALE, 0.40);
    assert_eq!(PROVISIONAL_INNER_CIRCLE_MIN_WEIGHT, 0.90);
    assert_eq!(PROVISIONAL_SOCIAL_RING_MIN_WEIGHT, 0.30);
    assert_eq!(PROVISIONAL_PUBLIC_OUTER_MIN_WEIGHT, 0.00);
    assert_eq!(PROVISIONAL_MAX_COALITION_MATERIALIZE_MEMBERS, 100);
    assert_eq!(MAX_COALITION_MATERIALIZE_MEMBERS, 100);

    // Test dynamic formula: w = base + scale * (share_a + share_b)/10000
    let share_a = 2500;
    let share_b = 2500;
    let combined_ratio = (share_a + share_b) as f64 / 10000.0;
    let expected_weight = PROVISIONAL_COALITION_BASE_WEIGHT + PROVISIONAL_COALITION_SHARE_SCALE * combined_ratio;
    assert!((expected_weight - 0.70).abs() < 1e-6);

    // Test upper clamp: even if shares sum to 20,000 bps (unconstrained input), weight is clamped to 0.90
    let runaway_ratio = 20000.0 / 10000.0;
    let unclamped = PROVISIONAL_COALITION_BASE_WEIGHT + PROVISIONAL_COALITION_SHARE_SCALE * runaway_ratio;
    let clamped = unclamped.clamp(PROVISIONAL_COALITION_BASE_WEIGHT, PROVISIONAL_COALITION_MAX_WEIGHT);
    assert_eq!(clamped, 0.90);
}

#[tokio::test]
async fn test_neo4j_schema_and_sync_flow() {
    let neo4j_uri = std::env::var("NEO4J_URI").unwrap_or_else(|_| "bolt://localhost:7687".to_string());
    let neo4j_user = std::env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string());
    let neo4j_pass = std::env::var("NEO4J_PASSWORD").unwrap_or_else(|_| "password".to_string());

    // Quick probe to check if Neo4j port is listening, avoiding neo4rs backoff retry delay
    let host_port = if let Some(stripped) = neo4j_uri.strip_prefix("bolt://") {
        stripped
    } else {
        "127.0.0.1:7687"
    };
    let reachable = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        tokio::net::TcpStream::connect(host_port),
    ).await;

    if reachable.is_err() || reachable.unwrap().is_err() {
        println!("Skipping live Neo4j integration test (no reachable Neo4j instance at {})", neo4j_uri);
        return;
    }

    let neo4j = match Neo4jClient::connect(&neo4j_uri, &neo4j_user, &neo4j_pass, None).await {
        Ok(client) => client,
        Err(e) => {
            println!("Skipping live Neo4j integration test (cannot connect to {}: {})", neo4j_uri, e);
            return;
        }
    };

    // 1. Initialize constraints and indexes
    neo4j.initialize_schema().await.expect("Failed to initialize Neo4j schema");

    // 2. Connect to Postgres to seed test coalition data
    let db_url = "postgres://postgres@localhost:5432/kairo_indexer";
    let db = match Database::connect(db_url).await {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping sync part (cannot connect to Postgres: {})", e);
            return;
        }
    };

    let test_cid = "0xcoalition_test_sync_999";
    let agent1 = "0x1111111111111111111111111111111111111111";
    let agent2 = "0x2222222222222222222222222222222222222222";
    let now = Utc::now();

    // Clean up Postgres test rows
    sqlx::query("DELETE FROM coalition_members WHERE coalition_id = $1")
        .bind(test_cid)
        .execute(db.pool())
        .await
        .ok();
    sqlx::query("DELETE FROM coalitions WHERE coalition_id = $1")
        .bind(test_cid)
        .execute(db.pool())
        .await
        .ok();

    // Record coalition in Postgres
    let formed = CoalitionFormedEvent {
        coalition_id: test_cid.to_string(),
        agent_ids: vec![1, 2],
        yield_bps: 1000,
        timestamp: 1700000000,
    };
    db.record_coalition_formed(&formed, 500, now).await.unwrap();

    let m1 = CoalitionMemberJoinedEvent {
        coalition_id: test_cid.to_string(),
        agent_address: agent1.to_string(),
        agent_id: 1,
        share_bps: 4000,
        timestamp: 1700000010,
    };
    db.record_coalition_member_joined(&m1, 501, now).await.unwrap();

    let m2 = CoalitionMemberJoinedEvent {
        coalition_id: test_cid.to_string(),
        agent_address: agent2.to_string(),
        agent_id: 2,
        share_bps: 6000,
        timestamp: 1700000020,
    };
    db.record_coalition_member_joined(&m2, 502, now).await.unwrap();

    // 3. Sync from Postgres into Neo4j
    let stats = sync_coalition_graph(&db, &neo4j).await.expect("Sync should succeed");
    assert!(stats.coalitions_synced >= 1);
    assert!(stats.members_synced >= 2);
    assert!(stats.peer_ring_edges_materialized >= 2, "Must materialize peer RING_EDGEs for coalition <= 100 members");

    // 4. Query dynamic coalition peer weight with explicit clamping
    let peer_weight = neo4j.get_dynamic_coalition_peer_weight(
        agent1,
        agent2,
        PROVISIONAL_COALITION_BASE_WEIGHT,
        PROVISIONAL_COALITION_SHARE_SCALE,
        PROVISIONAL_COALITION_MAX_WEIGHT,
    ).await.expect("Dynamic weight query failed");

    assert!(peer_weight.is_some());
    let (cid, weight) = peer_weight.unwrap();
    assert_eq!(cid, test_cid);
    // Combined share = 4000 + 6000 = 10000 bps = 1.0 -> weight = 0.50 + 0.40 * 1.0 = 0.90
    assert!((weight - 0.90).abs() < 1e-4);

    // 5. Query path attenuation via uniform shortestPath traversal
    let attenuation_opt = neo4j.calculate_path_attenuation(
        agent1,
        agent2,
        6,
        PROVISIONAL_COALITION_BASE_WEIGHT,
        PROVISIONAL_COALITION_SHARE_SCALE,
        PROVISIONAL_COALITION_MAX_WEIGHT,
    ).await.expect("Shortest-path attenuation traversal failed");
    assert!(attenuation_opt.is_some(), "Must find path across materialized coalition RING_EDGE");
    let att = attenuation_opt.unwrap();
    assert_eq!(att.hops, 1);
    assert!((att.attenuation - 0.90).abs() < 1e-4);

    // 6. Test shadow coalition disclosure: status becomes DISCLOSED
    neo4j.upsert_coalition(test_cid, 1000, "DISCLOSED", 505)
        .await
        .expect("Failed to update status to DISCLOSED");

    let disclosed_peer_weight = neo4j.get_dynamic_coalition_peer_weight(
        agent1,
        agent2,
        PROVISIONAL_COALITION_BASE_WEIGHT,
        PROVISIONAL_COALITION_SHARE_SCALE,
        PROVISIONAL_COALITION_MAX_WEIGHT,
    ).await.expect("Dynamic weight query on DISCLOSED coalition failed");

    assert!(disclosed_peer_weight.is_some(), "DISCLOSED shadow coalitions must remain valid coalition rings");
    assert_eq!(disclosed_peer_weight.unwrap().0, test_cid);

    // 7. Test large/unmaterialized coalition fallback (> 100 members simulation):
    // Create an unmaterialized coalition in Neo4j without peer RING_EDGEs
    let large_cid = "0xcoalition_large_unmaterialized_101";
    let agent_x = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let agent_y = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    neo4j.upsert_coalition(large_cid, 500, "ACTIVE", 600).await.unwrap();
    // Intentionally pass shares summing to 15,000 to simultaneously test the Cypher clamp!
    neo4j.upsert_coalition_member(large_cid, agent_x, 8000, 601, &now.to_rfc3339()).await.unwrap();
    neo4j.upsert_coalition_member(large_cid, agent_y, 7000, 602, &now.to_rfc3339()).await.unwrap();

    // calculate_path_attenuation has no direct RING_EDGE between agent_x and agent_y,
    // so it must trigger the query-time 2-hop fallback!
    let fallback_att = neo4j.calculate_path_attenuation(
        agent_x,
        agent_y,
        6,
        PROVISIONAL_COALITION_BASE_WEIGHT,
        PROVISIONAL_COALITION_SHARE_SCALE,
        PROVISIONAL_COALITION_MAX_WEIGHT,
    ).await.expect("Fallback path attenuation failed");

    assert!(fallback_att.is_some(), "Fallback must resolve unmaterialized coalition ties");
    let f_att = fallback_att.unwrap();
    assert_eq!(f_att.hops, 1);
    // 8000 + 7000 = 15000 bps -> raw = 0.50 + 0.40 * 1.5 = 1.10 -> clamped to 0.90
    assert_eq!(f_att.attenuation, 0.90, "Weight must be clamped to 0.90 even with runaway shares");

    // Clean up Postgres
    sqlx::query("DELETE FROM coalition_members WHERE coalition_id = $1")
        .bind(test_cid)
        .execute(db.pool())
        .await
        .ok();
    sqlx::query("DELETE FROM coalitions WHERE coalition_id = $1")
        .bind(test_cid)
        .execute(db.pool())
        .await
        .ok();

    println!("Neo4j live integration test passed: attenuation, clamping, and unmaterialized fallback verified!");
}
