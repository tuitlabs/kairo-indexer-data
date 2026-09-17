use std::collections::HashMap;
use chrono::{DateTime, Utc};
use sqlx::Row;
use crate::db::Database;
use crate::graph::client::Neo4jClient;
use crate::error::{IndexerError, Result};
use crate::config::{
    PROVISIONAL_COALITION_BASE_WEIGHT,
    PROVISIONAL_COALITION_MAX_WEIGHT,
    PROVISIONAL_COALITION_SHARE_SCALE,
    MAX_COALITION_MATERIALIZE_MEMBERS,
};

#[derive(Debug, Clone, Default)]
pub struct CoalitionSyncStats {
    pub coalitions_synced: usize,
    pub members_synced: usize,
    pub peer_ring_edges_materialized: usize,
}

/// Syncs canonical coalition state from PostgreSQL into Neo4j:
/// 1. Always maintains O(1) writes per member by upserting (:Agent)-[:MEMBER_OF]->(:Coalition).
/// 2. For coalitions with <= MAX_COALITION_MATERIALIZE_MEMBERS, also materializes uniform
///    (:Agent)-[:RING_EDGE { ring_tier: 'COALITION_RING' }]->(:Agent) so Neo4j's native
///    shortestPath / attenuation algorithms can walk across coalition links seamlessly.
pub async fn sync_coalition_graph(
    db: &Database,
    neo4j: &Neo4jClient,
) -> Result<CoalitionSyncStats> {
    // 1. Fetch all coalitions from Postgres
    let coalition_rows = sqlx::query(
        "SELECT coalition_id, yield_bps, status, created_at_block, created_at FROM coalitions"
    )
    .fetch_all(db.pool())
    .await
    .map_err(|e| IndexerError::Database(format!("Failed to fetch coalitions for sync: {}", e)))?;

    let mut coalitions_count = 0;
    for row in coalition_rows {
        let cid: String = row.get("coalition_id");
        let yield_bps: i32 = row.get("yield_bps");
        let status: String = row.get("status");
        let block: i64 = row.get("created_at_block");

        neo4j.upsert_coalition(&cid, yield_bps, &status, block).await?;
        coalitions_count += 1;
    }

    // 2. Fetch all coalition members from Postgres
    let member_rows = sqlx::query(
        "SELECT coalition_id, agent_address, share_bps, joined_at_block, joined_at FROM coalition_members"
    )
    .fetch_all(db.pool())
    .await
    .map_err(|e| IndexerError::Database(format!("Failed to fetch coalition_members for sync: {}", e)))?;

    let mut members_count = 0;
    let mut coalition_members_map: HashMap<String, Vec<(String, i32)>> = HashMap::new();

    for row in member_rows {
        let cid: String = row.get("coalition_id");
        let agent: String = row.get("agent_address");
        let share: i32 = row.get("share_bps");
        let block: i64 = row.get("joined_at_block");
        let joined_at: DateTime<Utc> = row.get("joined_at");

        neo4j.upsert_coalition_member(
            &cid,
            &agent,
            share,
            block,
            &joined_at.to_rfc3339(),
        ).await?;
        members_count += 1;

        coalition_members_map.entry(cid).or_default().push((agent, share));
    }

    // 3. Materialize direct RING_EDGEs for coalitions within threshold limit
    // Enables uniform shortestPath traversal across both RING_EDGE and coalition peers
    let mut peer_edges_count = 0;
    for (cid, members) in coalition_members_map {
        if members.len() <= MAX_COALITION_MATERIALIZE_MEMBERS {
            let count = neo4j.materialize_coalition_peer_edges(
                &cid,
                &members,
                PROVISIONAL_COALITION_BASE_WEIGHT,
                PROVISIONAL_COALITION_SHARE_SCALE,
                PROVISIONAL_COALITION_MAX_WEIGHT,
            ).await?;
            peer_edges_count += count;
        }
    }

    Ok(CoalitionSyncStats {
        coalitions_synced: coalitions_count,
        members_synced: members_count,
        peer_ring_edges_materialized: peer_edges_count,
    })
}
