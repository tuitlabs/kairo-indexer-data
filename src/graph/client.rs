use std::sync::Arc;
use neo4rs::{Graph, ConfigBuilder, query};
use crate::error::{IndexerError, Result};

#[derive(Clone)]
pub struct Neo4jClient {
    graph: Arc<Graph>,
}

impl Neo4jClient {
    pub async fn connect(uri: &str, user: &str, pass: &str, database: Option<&str>) -> Result<Self> {
        let mut builder = ConfigBuilder::default()
            .uri(uri)
            .user(user)
            .password(pass);

        if let Some(db) = database {
            builder = builder.db(db);
        }

        let config = builder
            .build()
            .map_err(|e| IndexerError::Database(format!("Invalid Neo4j config: {}", e)))?;

        let graph = Graph::connect(config)
            .await
            .map_err(|e| IndexerError::Database(format!("Failed to connect to Neo4j: {}", e)))?;

        Ok(Self {
            graph: Arc::new(graph),
        })
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// Initializes schema constraints and indexes in Neo4j
    pub async fn initialize_schema(&self) -> Result<()> {
        let statements = [
            // Node Constraints
            "CREATE CONSTRAINT agent_address_unique IF NOT EXISTS FOR (a:Agent) REQUIRE a.address IS UNIQUE",
            "CREATE CONSTRAINT coalition_id_unique IF NOT EXISTS FOR (c:Coalition) REQUIRE c.coalition_id IS UNIQUE",
            // Indexes
            "CREATE INDEX agent_address_idx IF NOT EXISTS FOR (a:Agent) ON (a.address)",
            "CREATE INDEX coalition_id_idx IF NOT EXISTS FOR (c:Coalition) ON (c.coalition_id)",
            "CREATE INDEX ring_edge_tier_idx IF NOT EXISTS FOR ()-[r:RING_EDGE]-() ON (r.ring_tier)",
            "CREATE INDEX ring_edge_weight_idx IF NOT EXISTS FOR ()-[r:RING_EDGE]-() ON (r.weight)",
        ];

        for stmt in statements {
            self.graph.run(query(stmt))
                .await
                .map_err(|e| IndexerError::Database(format!("Failed to execute Cypher constraint/index '{}': {}", stmt, e)))?;
        }

        Ok(())
    }

    /// Upsert an Agent node
    pub async fn upsert_agent(&self, address: &str) -> Result<()> {
        let q = query(
            "MERGE (a:Agent { address: $address }) \
             ON CREATE SET a.first_seen_at = datetime(), a.updated_at = datetime() \
             ON MATCH SET a.updated_at = datetime()"
        )
        .param("address", address.to_lowercase());

        self.graph.run(q).await
            .map_err(|e| IndexerError::Database(format!("Neo4j upsert_agent failed: {}", e)))?;

        Ok(())
    }

    /// Upsert a Coalition node
    pub async fn upsert_coalition(
        &self,
        coalition_id: &str,
        yield_bps: i32,
        status: &str,
        created_at_block: i64,
    ) -> Result<()> {
        let q = query(
            "MERGE (c:Coalition { coalition_id: $coalition_id }) \
             ON CREATE SET c.yield_bps = $yield_bps, c.status = $status, c.created_at_block = $created_at_block, c.updated_at = datetime() \
             ON MATCH SET c.yield_bps = $yield_bps, c.status = $status, c.updated_at = datetime()"
        )
        .param("coalition_id", coalition_id)
        .param("yield_bps", yield_bps as i64)
        .param("status", status)
        .param("created_at_block", created_at_block);

        self.graph.run(q).await
            .map_err(|e| IndexerError::Database(format!("Neo4j upsert_coalition failed: {}", e)))?;

        Ok(())
    }

    /// Upsert a Coalition Member edge (:Agent)-[:MEMBER_OF]->(:Coalition)
    pub async fn upsert_coalition_member(
        &self,
        coalition_id: &str,
        agent_address: &str,
        share_bps: i32,
        joined_at_block: i64,
        joined_at_rfc3339: &str,
    ) -> Result<()> {
        let q = query(
            "MERGE (a:Agent { address: $address }) \
             ON CREATE SET a.first_seen_at = datetime(), a.updated_at = datetime() \
             WITH a \
             MATCH (c:Coalition { coalition_id: $coalition_id }) \
             MERGE (a)-[m:MEMBER_OF]->(c) \
             ON CREATE SET m.share_bps = $share_bps, m.joined_at_block = $joined_at_block, m.joined_at = datetime($joined_at) \
             ON MATCH SET m.share_bps = $share_bps"
        )
        .param("address", agent_address.to_lowercase())
        .param("coalition_id", coalition_id)
        .param("share_bps", share_bps as i64)
        .param("joined_at_block", joined_at_block)
        .param("joined_at", joined_at_rfc3339);

        self.graph.run(q).await
            .map_err(|e| IndexerError::Database(format!("Neo4j upsert_coalition_member failed: {}", e)))?;

        Ok(())
    }

    /// Upsert a direct peer-to-peer RING_EDGE (used for Inner Circle and Social Ring)
    pub async fn upsert_ring_edge(
        &self,
        from_agent: &str,
        to_agent: &str,
        ring_tier: &str,
        weight: f64,
        source_type: &str,
        source_id: &str,
    ) -> Result<()> {
        let q = query(
            "MERGE (from:Agent { address: $from_address }) \
             MERGE (to:Agent { address: $to_address }) \
             MERGE (from)-[r:RING_EDGE { ring_tier: $ring_tier, source_id: $source_id }]->(to) \
             SET r.weight = $weight, r.source_type = $source_type, r.updated_at = datetime()"
        )
        .param("from_address", from_agent.to_lowercase())
        .param("to_address", to_agent.to_lowercase())
        .param("ring_tier", ring_tier)
        .param("weight", weight)
        .param("source_type", source_type)
        .param("source_id", source_id);

        self.graph.run(q).await
            .map_err(|e| IndexerError::Database(format!("Neo4j upsert_ring_edge failed: {}", e)))?;

        Ok(())
    }

    /// Delete a direct peer-to-peer RING_EDGE (used when an edge transitions to the Public tier)
    pub async fn delete_ring_edge(
        &self,
        from_agent: &str,
        to_agent: &str,
        source_id: &str,
    ) -> Result<()> {
        let q = query(
            "MATCH (from:Agent { address: $from_address })-[r:RING_EDGE { source_id: $source_id }]->(to:Agent { address: $to_address }) \
             DELETE r"
        )
        .param("from_address", from_agent.to_lowercase())
        .param("to_address", to_agent.to_lowercase())
        .param("source_id", source_id);

        self.graph.run(q).await
            .map_err(|e| IndexerError::Database(format!("Neo4j delete_ring_edge failed: {}", e)))?;

        Ok(())
    }

    /// Materialize pairwise peer edges for a coalition (bounded by MAX_COALITION_MATERIALIZE_MEMBERS)
    pub async fn materialize_coalition_peer_edges(
        &self,
        coalition_id: &str,
        members: &[(String, i32)], // (address, share_bps)
        base_weight: f64,
        share_scale: f64,
        max_weight: f64,
    ) -> Result<usize> {
        let mut count = 0;
        let n = members.len();

        for i in 0..n {
            for j in 0..n {
                if i == j {
                    continue;
                }
                let (addr_a, share_a) = &members[i];
                let (addr_b, share_b) = &members[j];

                let combined_ratio = (*share_a + *share_b) as f64 / 10000.0;
                let raw_weight = base_weight + share_scale * combined_ratio;
                let weight = raw_weight.clamp(base_weight, max_weight);

                self.upsert_ring_edge(
                    addr_a,
                    addr_b,
                    "COALITION_RING",
                    weight,
                    "ON_CHAIN_COALITION",
                    coalition_id,
                ).await?;
                count += 1;
            }
        }

        Ok(count)
    }

    /// Calculate dynamic pairwise coalition weight at query time between two agents.
    /// Note: Includes both ACTIVE and DISCLOSED coalitions. Clamped between [base_weight, max_weight].
    pub async fn get_dynamic_coalition_peer_weight(
        &self,
        agent_a: &str,
        agent_b: &str,
        base_weight: f64,
        share_scale: f64,
        max_weight: f64,
    ) -> Result<Option<(String, f64)>> {
        let mut result = self.graph.execute(
            query(
                "MATCH (a:Agent { address: $agent_a })-[m1:MEMBER_OF]->(c:Coalition)<-[m2:MEMBER_OF]-(b:Agent { address: $agent_b }) \
                 WHERE c.status IN ['ACTIVE', 'DISCLOSED'] \
                 WITH c, $base_weight + $share_scale * (toFloat(m1.share_bps + m2.share_bps) / 10000.0) AS raw_weight \
                 RETURN c.coalition_id AS coalition_id, \
                        CASE \
                          WHEN raw_weight < $base_weight THEN $base_weight \
                          WHEN raw_weight > $max_weight THEN $max_weight \
                          ELSE raw_weight \
                        END AS weight \
                 ORDER BY weight DESC \
                 LIMIT 1"
            )
            .param("agent_a", agent_a.to_lowercase())
            .param("agent_b", agent_b.to_lowercase())
            .param("base_weight", base_weight)
            .param("share_scale", share_scale)
            .param("max_weight", max_weight)
        )
        .await
        .map_err(|e| IndexerError::Database(format!("Query dynamic coalition weight failed: {}", e)))?;

        if let Ok(Some(row)) = result.next().await {
            let coalition_id: String = row.get("coalition_id")
                .map_err(|e| IndexerError::Database(format!("Failed to get coalition_id: {}", e)))?;
            let weight: f64 = row.get("weight")
                .map_err(|e| IndexerError::Database(format!("Failed to get weight: {}", e)))?;
            Ok(Some((coalition_id, weight)))
        } else {
            Ok(None)
        }
    }

    /// Calculate signal attenuation A(u, v) = product of edge weights on the shortest path
    /// per SKILLS.md Section 7. Walks across uniform RING_EDGE relationships.
    /// Falls back to 2-hop dynamic coalition evaluation for large/unmaterialized coalitions (> 100 members).
    ///
    /// ARCHITECTURAL SCOPE & LIMITATION NOTE:
    /// The query-time fallback only resolves direct pairwise queries between the two specific endpoints
    /// (`from_agent` and `to_agent`). If an unmaterialized coalition (> 100 members) sits as an intermediate
    /// hop in a longer multi-hop chain (e.g. A -> B -> C -> D where B <-> C is an unmaterialized coalition tie),
    /// native `shortestPath(A, D)` cannot traverse through the missing intermediate edge and will return None.
    /// Multi-hop propagation queries that route through large unmaterialized coalitions as intermediate links
    /// are not currently resolvable; only direct pairwise lookups are guaranteed.
    pub async fn calculate_path_attenuation(
        &self,
        from_agent: &str,
        to_agent: &str,
        max_hops: usize,
        fallback_base_weight: f64,
        fallback_share_scale: f64,
        fallback_max_weight: f64,
    ) -> Result<Option<PathAttenuation>> {
        let max_hops = std::cmp::min(max_hops, 10);
        let cypher = format!(
            "MATCH path = shortestPath((from:Agent {{ address: $from_address }})-[r:RING_EDGE*1..{}]->(to:Agent {{ address: $to_address }})) \
             RETURN [rel in relationships(path) | rel.weight] AS weights, \
                    [node in nodes(path) | node.address] AS nodes",
            max_hops
        );

        let mut result = self.graph.execute(
            query(&cypher)
                .param("from_address", from_agent.to_lowercase())
                .param("to_address", to_agent.to_lowercase())
        )
        .await
        .map_err(|e| IndexerError::Database(format!("Shortest path query failed: {}", e)))?;

        if let Ok(Some(row)) = result.next().await {
            let weights: Vec<f64> = row.get("weights")
                .map_err(|e| IndexerError::Database(format!("Failed to parse weights: {}", e)))?;
            let nodes: Vec<String> = row.get("nodes")
                .map_err(|e| IndexerError::Database(format!("Failed to parse nodes: {}", e)))?;

            let mut attenuation = 1.0f64;
            for w in &weights {
                attenuation *= *w;
            }

            return Ok(Some(PathAttenuation {
                from: from_agent.to_lowercase(),
                to: to_agent.to_lowercase(),
                attenuation,
                hops: weights.len(),
                path_nodes: nodes,
                weights,
            }));
        }

        // Fallback: If no direct materialized path exists (e.g. agents are in an unmaterialized
        // coalition exceeding MAX_COALITION_MATERIALIZE_MEMBERS), check 2-hop query-time coalition tie.
        if let Ok(Some((_cid, weight))) = self.get_dynamic_coalition_peer_weight(
            from_agent,
            to_agent,
            fallback_base_weight,
            fallback_share_scale,
            fallback_max_weight,
        ).await {
            return Ok(Some(PathAttenuation {
                from: from_agent.to_lowercase(),
                to: to_agent.to_lowercase(),
                attenuation: weight,
                hops: 1,
                path_nodes: vec![from_agent.to_lowercase(), to_agent.to_lowercase()],
                weights: vec![weight],
            }));
        }

        Ok(None)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathAttenuation {
    pub from: String,
    pub to: String,
    pub attenuation: f64,
    pub hops: usize,
    pub path_nodes: Vec<String>,
    pub weights: Vec<f64>,
}
