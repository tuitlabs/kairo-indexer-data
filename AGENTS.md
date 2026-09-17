# AGENTS.md — kairo-indexer-data

This file orients any coding agent (Claude Code, Cursor, etc.) working in this
repository. Read this before making changes.

## 1. What Kairo Is

Kairo is a protocol for **Social Markets**: it tokenizes opinions/stances as
tradeable positions whose price is driven by the **credibility-weighted
conviction** of the people (and AI agents) who hold them, not raw popularity.
Where prediction markets ask "what will happen," Social Markets ask "what do
credible participants actually believe, and how strongly."

Four protocol components make this work without an oracle:
- **Stance Registry** — on-chain identity for every position, never force-expires.
- **Dynamic Pricing Engine** — prices positions on capital *and* credibility.
- **Lawful Cheating Framework (LCF)** — taxes manipulative behavior instead of banning strategic bluffing outright.
- **Subjective Epoch Resolution Module** — periodic (24h) checkpoints that settle yield and update reputations without an external data feed.

Markets come in three tiers: **Tier I** (factual, eventually oracle-resolved),
**Tier II** (pure consensus, no external resolution — the most distinctive),
**Tier III** (fast, high-volume "banter" markets).

## 2. This Repo's Role: kairo-indexer-data

`kairo-indexer-data` is the **off-chain data & reputation layer** of the
protocol. It does not execute trades or hold funds — it observes on-chain
activity, computes derived signals, and serves them to the rest of the stack
and to external consumers.

Per the repository ownership matrix, this repo owns:
- **The Graph subgraph** (Base) — indexes EVM contract events.
- **Solana custom indexer** — indexes `kairo-programs-solana` account/program state.
- **Neo4j Graph DB** — stores and queries the social graph / ring topology.
- **Reputation Engine** — computes agent/user credibility scores off-chain.

Owning engineer track (per the 12-week plan): Backend, AI Runtime & Data
Infrastructure lead, alongside `kairo-agent-runtime` and `kairo-core-api`.

### In / Out of scope
- **In scope:** ingestion workers, PostgreSQL schema & queries, Neo4j graph
  model, reputation computation, Stance Volatility Coefficient (SVC)
  computation, Sentiment & Conviction Index API, Merkle root generation for
  on-chain reputation commitments.
- **Out of scope:** smart contracts (`kairo-contracts-evm`), Solana programs
  (`kairo-programs-solana`), agent runtime/LLM decision loop
  (`kairo-agent-runtime`), the public REST/WebSocket/voice gateway
  (`kairo-core-api`), frontend (`kairo-web-app`).

## 3. Tech Stack

- **Language:** Rust (ingestion workers), Python (data/reputation pipelines) — mixed per spec.
- **Indexing:** The Graph subgraph on Base; custom Solana indexer via gRPC account streaming.
- **Storage:** PostgreSQL (block events, trade histories, stance positions, coalition graphs), Neo4j / AuraDB (social graph edges, ring membership).
- **API:** GraphQL for graph queries; REST for the Sentiment & Conviction Index.
- **Cross-chain awareness:** consumes/produces Wormhole message payloads for reputation sync (does not run the relayer itself — that's `kairo-bridge-relayer` / `kairo-contracts-evm`'s `KairoBridge.sol`).

## 4. Data Flow (where this repo sits)

```
Base (EVM WebSockets) ──┐
                         ├──> Rust ingestion workers ──> PostgreSQL ──> Reputation Engine ──> Merkle root
Solana (gRPC streaming) ─┘                               Neo4j graph        │
                                                                             ▼
                                                         KairoReputation.sol.updateReputationRoot()
                                                         (submitted on Base — coordinate with kairo-contracts-evm)
                                                                             │
                                                                             ▼
                                              ReputationCacheSync (Base → Solana via Wormhole)
                                              updates kairo_reputation_cache PDA (Solana)
```

Downstream consumers of this repo's output:
- `kairo-core-api` (profile/pricing REST endpoints, WebSocket feeds)
- `kairo-web-app` (dashboards, leaderboards)
- `KairoCredibilityHook.sol` / `KairoMarket.sol` on-chain, indirectly, via the reputation Merkle root and `verifyReputation()`
- External enterprise consumers of the Sentiment & Conviction Index API

## 5. Roadmap Anchors (12-week plan, this repo's slice)

- **Q1 (weeks 1–3):** Deploy initial Subgraph on The Graph for Base event indexing. Deliver REST endpoints for profiles and pricing queries (may be split with `kairo-core-api`).
- **Q2 (weeks 4–6):** Deploy Neo4j AuraDB instance for social graph edges. Implement off-chain Reputation Computation Engine. Implement WebSocket channels for live updates.
- **Q3 (weeks 7–9):** Optimize Neo4j Cypher query performance. Build the public enterprise Sentiment & Conviction Index API.
- **Q4 (weeks 10–12):** Scale for high concurrency. Ensure reputation calculations complete within the epoch boundary (24h) under load.

## 6. Segment KPIs

- Minimize indexer lag on both Base and Solana.
- Ensure reputation calculations are complete within the epoch boundary (24h cycle, `KairoSettlement.sol`).
- Indexing sync latency target: **< 1.0s** event-to-API visibility (per PRD §8.3).

## 7. Domain Concepts to Know Before Touching Reputation/Indexing Code

See `SKILLS.md` for full formulas. At minimum, understand:
- **Credibility Score (C)** — 0.1–1.0 (or 0–1000 scaled on-chain), domain-specific, decays/slashes on failed positions.
- **Effective Stake (S_eff)** — capital transformed by credibility, not raw capital.
- **Epoch** — 24h non-oracle settlement cycle; reputation is *never* a permanent credential, it's re-tested every epoch.
- **4-tier ring topology** — Inner Circle, Coalition Ring, Social Ring, Public Outer Layer — determines information propagation delay/attenuation in the social graph this repo models in Neo4j.
- **Lawful Cheating Framework (LCF)** — bluffing/strategic behavior is *permitted and priced*, not simply filtered out. Reputation logic should record LCF-flagged behavior, not just penalize it silently.

## 8. Working Conventions

> Fill in once tooling is finalized — placeholders below reflect the spec's stated stack.

- Ingestion workers: Rust, listening to Base EVM WebSockets and Solana gRPC account streams.
- Schema migrations: PostgreSQL (tool TBD — e.g. `sqlx migrate` or `diesel`).
- Graph model changes: document Cypher schema changes in `/schema/neo4j/`.
- Any change to reputation-score computation logic must be cross-checked against `KairoReputation.sol`'s `verifyReputation()` expectations (Merkle proof format, 0–1000 scaled score) — coordinate with the `kairo-contracts-evm` owner before changing score ranges or weighting.
- Any change to API response shape for `/v1/index/*` endpoints should be checked against consumers in `kairo-core-api` and `kairo-web-app`.

## 9. Related Repos (for cross-repo context, not to be edited from here)

| Repo | Owns |
|---|---|
| `kairo-contracts-evm` | `KairoReputation.sol` (consumes Merkle roots this repo produces), `KairoMarket.sol`, `KairoSettlement.sol` |
| `kairo-programs-solana` | `kairo_reputation_cache` PDA (receives synced reputation from this repo's pipeline via Wormhole) |
| `kairo-agent-runtime` | LLM decision engine that reads reputation/index data to act |
| `kairo-core-api` | Public REST/WebSocket/Voice Gateway — a consumer of this repo's data |
| `kairo-web-app` | Dashboards/leaderboards — a consumer of this repo's data |
| `kairo-bridge-relayer` | Wormhole/LayerZero message transport (this repo produces payloads, doesn't relay them) |
