// ==============================================================================
// Kairo Protocol: Neo4j Social Graph & 4-Tier Ring Topology Schema
// Migration: 001_initial_social_graph.cypher
//
// DISCLAIMER & GOVERNANCE CALIBRATION NOTE:
// The numeric weight bands and materialization bounds referenced below:
//   - Inner Circle:                         0.90 <= w <= 1.00
//   - Coalition Ring:                       0.50 <= w <= 0.90 (w = base_weight + share_scale * combined_share)
//   - Social Ring:                          0.30 <= w <  0.60
//   - Public Outer Layer:                   0.00 <= w <  0.30
//   - MAX_COALITION_MATERIALIZE_MEMBERS:    100 members
// are PROVISIONAL, GOVERNANCE-TUNABLE DEFAULTS derived as starting points to
// satisfy w: E -> [0, 1] per SKILLS.md. They are unvalidated against live market
// data and subject to calibration by the Kairo Protocol DAO.
//
// HYBRID STORAGE & TRAVERSAL ARCHITECTURE:
// 1. Canonical Bipartite Storage:
//    Coalition membership is stored canonically as (:Agent)-[:MEMBER_OF]->(:Coalition).
// 2. Bounded Materialization (MAX_COALITION_MATERIALIZE_MEMBERS = 100):
//    For coalitions with <= 100 members, pairwise peer edges are materialized as
//    (:Agent)-[:RING_EDGE { ring_tier: 'COALITION_RING', weight }]->(:Agent).
//    Weights are clamped within [base_weight, max_weight] (0.50..0.90).
// 3. Fallback for Large Coalitions (> 100 members):
//    Coalitions exceeding the materialization threshold do not materialize O(n^2) edges;
//    instead, path-finding queries fall back to a 2-hop query-time evaluation via
//    (:Agent)-[:MEMBER_OF]->(:Coalition)<-[:MEMBER_OF]-(:Agent), ensuring large
//    broadcast syndicates remain discoverable without graph explosion.
// 4. Shadow Coalition Disclosure:
//    Coalitions with status IN ['ACTIVE', 'DISCLOSED'] maintain active ring ties.
//    Disclosed shadow coalitions do not dissolve upon crossing the 40% supply threshold;
//    they remain active economic partnerships with enforced disclosure.
// ==============================================================================

// 1. Constraints (Node Uniqueness)
CREATE CONSTRAINT agent_address_unique IF NOT EXISTS
FOR (a:Agent) REQUIRE a.address IS UNIQUE;

CREATE CONSTRAINT coalition_id_unique IF NOT EXISTS
FOR (c:Coalition) REQUIRE c.coalition_id IS UNIQUE;

// 2. Indexes for Query Performance
CREATE INDEX agent_address_idx IF NOT EXISTS
FOR (a:Agent) ON (a.address);

CREATE INDEX coalition_id_idx IF NOT EXISTS
FOR (c:Coalition) ON (c.coalition_id);

CREATE INDEX ring_edge_tier_idx IF NOT EXISTS
FOR ()-[r:RING_EDGE]-() ON (r.ring_tier);

CREATE INDEX ring_edge_weight_idx IF NOT EXISTS
FOR ()-[r:RING_EDGE]-() ON (r.weight);

// ==============================================================================
// Reference Cypher Queries
// ==============================================================================

// Upsert Agent Node
// MERGE (a:Agent { address: $address })
// ON CREATE SET a.first_seen_at = datetime(), a.updated_at = datetime()
// ON MATCH SET a.updated_at = datetime();

// Upsert Coalition Node
// MERGE (c:Coalition { coalition_id: $coalition_id })
// ON CREATE SET c.yield_bps = $yield_bps, c.status = $status, c.created_at_block = $created_at_block, c.updated_at = datetime()
// ON MATCH SET c.yield_bps = $yield_bps, c.status = $status, c.updated_at = datetime();

// Upsert Coalition Membership (O(1) write per member)
// MATCH (a:Agent { address: $address })
// MATCH (c:Coalition { coalition_id: $coalition_id })
// MERGE (a)-[m:MEMBER_OF]->(c)
// ON CREATE SET m.share_bps = $share_bps, m.joined_at_block = $joined_at_block, m.joined_at = datetime($joined_at)
// ON MATCH SET m.share_bps = $share_bps;

// Shortest-Path Attenuation Traversal across uniform RING_EDGEs:
// A(u, v) = product of weights on shortest path from u to v
// MATCH path = shortestPath((from:Agent { address: $from_address })-[r:RING_EDGE*1..6]->(to:Agent { address: $to_address }))
// RETURN [rel in relationships(path) | rel.weight] AS weights,
//        [node in nodes(path) | node.address] AS path_nodes;

// Dynamic Pairwise Coalition Weight with Explicit Clamping (Fallback for Coalitions > 100 members)
// MATCH (a:Agent { address: $agent_a })-[m1:MEMBER_OF]->(c:Coalition)<-[m2:MEMBER_OF]-(b:Agent { address: $agent_b })
// WHERE c.status IN ['ACTIVE', 'DISCLOSED']
// WITH c, $base_weight + $share_scale * (toFloat(m1.share_bps + m2.share_bps) / 10000.0) AS raw_weight
// RETURN c.coalition_id AS coalition_id,
//        CASE
//          WHEN raw_weight < $base_weight THEN $base_weight
//          WHEN raw_weight > $max_weight THEN $max_weight
//          ELSE raw_weight
//        END AS dynamic_weight
// ORDER BY dynamic_weight DESC LIMIT 1;
