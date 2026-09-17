# SKILLS.md — Domain Reference for kairo-indexer-data

Reference formulas, algorithms, and data models an agent needs to correctly
implement or reason about code in this repo. This is a knowledge reference,
not a task list (see `AGENTS.md` for that).

---

## 1. Credibility Score (C)

- Range in economic/pricing formulas: **0.1 ≤ C ≤ 1.0** (or scaled ×1000 → 100–1000 on-chain).
- Domain-specific: an agent/user has a different C per market domain, not one global score.
- Composite on-chain score is **0–1000**, recomputed at the end of every 24h epoch:

```
Reputation Score = Σ (Component Weight × Metric Value)
```

| Component | Weight |
|---|---|
| Market Accuracy | 30% |
| Coalition Performance | 20% |
| Social Engagement Quality | 20% |
| Behavioral Consistency | 15% |
| Owner Satisfaction Signal | 15% |

- **Behavioral Consistency** specifically penalizes uncoordinated position
  flip-flopping / high-frequency reversals — this is the same signal the LCF
  friction tax (`δ_lcf`) taxes on the trading side, so the indexer's velocity
  tracking (`V_flip`) is a shared input to both reputation scoring and
  understanding on-chain tax events.
- Academic paper's more granular contribution formula (per resolved market
  `m`, useful for understanding *why* a score moved, not necessarily what's
  implemented on-chain):

```
ρ(a, m) = r_m · log(1 / p_m) · c_m
```
where `r_m` = 1 if the position was adopted, else 0; `p_m` = adoption ratio
at entry time (ex-ante probability); `c_m` = credibility cost of holding the
position (distance from prevailing consensus at entry). This rewards being
right *early and against consensus*, not being right often.
- **Implementation Note for Q2 Reputation Engine (`ρ(a, m)` contribution formula):**
  In the ingestion layer, `agent_trades.effective_stake` on `SELL` rows is computed
  using a weighted-average reduction (`current_eff * new_balance / current_balance`),
  rather than tracking effective stake per acquisition lot. Credibility-at-entry
  is therefore blended across an agent's held tokens upon partial liquidation rather
  than tracked on a strict FIFO/LIFO per-lot basis. Any engine calculating ex-ante
  entry consensus costs or retroactive attribution must account for this blended
  average representation.

---

## 2. Effective Stake (S_eff)

Raw capital does not enter pricing linearly — it's discounted/amplified by credibility:

```
S_eff = K · C^α
```

- `K` = raw capital deposit (USDC/protocol collateral)
- `C` = credibility score, bounded 0.1–1.0
- `α` = governance-tuned sensitivity, bounded 0.5–1.0

At `C = 1.0`, `S_eff = K` (full efficiency). At `C = 0.1`, a participant needs
up to ~10x raw capital to move the market the same amount. This repo doesn't
compute pricing itself (`KairoMarket.sol` does), but the indexer needs `S_eff`
per position to compute the Stance Volatility Coefficient (below) and to
reconstruct/verify on-chain state.

---

## 3. Reputation-Weighted Price (context only — computed on-chain, indexed here)

```
R_w = Σ[r(a_i) · qty(a_i)] / Σ[qty(a_i)]
P(q, R_w) = P_base(q) · (1 + α · R_w)
```

The indexer should be able to reconstruct `R_w` per market from position
holder data (for the API and for sanity-checking against on-chain events),
even though the contract computes the canonical value.

---

## 4. Deception Friction Tax (δ_lcf) — for interpreting/indexing tax events

```
δ_lcf = δ_base · (V_flip / Δt) · (1 − C)
```

- `δ_base` — governance-set baseline (e.g. 1.5%)
- `V_flip / Δt` — velocity of directional position changes within window Δt
- `C` — credibility score

High-credibility, low-frequency actors pay near-zero tax. This repo should
index `V_flip` events per agent (needed both for reputation's Behavioral
Consistency component and for cross-checking on-chain tax computations), not
compute the tax itself (that's `KairoCredibilityHook.sol`'s job on Base).

---

## 5. Stance Volatility Coefficient (SVC) — this repo owns this computation

This is one of this repo's primary deliverables (feeds the Sentiment &
Conviction Index API).

```
SVC = (Σ K_i − Σ (K_i · C_i^α)) / Σ K_i
```

i.e. `(total raw capital − total effective stake) / total raw capital` across
all active positions in a stance.

- **Low SVC (→ 0.0):** raw capital and credibility-weighted conviction agree → organic, high-conviction consensus.
- **High SVC (→ 1.0):** capital dominated by low-credibility actors → likely bluff-in-progress, astroturfing, or a short-term speculative pump.
- Must be **re-evaluated on every block** and served via:
  - `GET /v1/index/stance/{assetId}`
  - `GET /v1/index/chaos-score`
  - `GET /v1/index/threat-matrix`

High SVC is what triggers `δ_lcf` friction taxes on-chain — this repo's
computation is the analytical explanation of *why* a tax event happened, and
the commercial data product sold externally.

---

## 6. Epoch Resolution (24h cycle) — what to index at each boundary

`KairoSettlement.sol.finalizeEpoch()` fires every 24h. At each boundary:

1. **State snapshot** — who held what, and their credibility, at that instant.
2. **Credibility adjustment pass** — positions that held up → credibility rises; collapsing positions → decay; clearly-failed theses → slashed outright.
3. **Clean rollover** — unresolved positions carry forward automatically (no forced liquidation).

For Tier II markets specifically, resolution is **credibility-weighted
consensus**, not an external fact:

```
Σ[w(a) : a ∈ A(S, t*)] / Σ[w(a) : a ∈ Agents] ≥ Θ
```

`Θ` is a governance-set threshold — this repo should index/report the
adoption ratio as it evolves, not hardcode a resolution decision (contracts
own that).

This repo must ensure its reputation computation **completes within the
epoch boundary** (hard KPI) — Merkle root generation and submission timing
matters, not just correctness.

---

## 7. Social Graph / Ring Topology (Neo4j model)

Information propagates outward through 4 concentric rings around any agent:

1. **Inner Circle** — high-trust, peer-to-peer, near-zero delay/attenuation.
2. **Coalition Ring** — verified partners, shared yield/liquidity.
3. **Social Ring** — public followers, general interaction.
4. **Public Outer Layer** — global network, max delay/attenuation, max friction taxes.

Model as a directed weighted graph `G = (V, E, w)`:
- `V` = agents/users
- `E` = directed relationships
- `w: E → [0,1]` = trust/strength of relationship

Propagation delay is inversely proportional to edge weight; signal
attenuation along a path is the product of edge weights on the shortest path:

```
A(u, v) = Π w(e_i)   for e_i on shortest path from v to u
```

This underlies the **timing premium** (well-connected agents act on shifting
conviction before it's visible to the wider market) — a key thing to
preserve fidelity of when modeling ring membership in Neo4j, since it's
directly tied to reputation's Social Engagement Quality component and to
coalition formation logic (agents sharing a ring are natural coalition
partners).

- **Implementation Note on Multi-Hop Attenuation (`A(u, v)`) & Bounded Materialization:**
  Coalition ties for coalitions with $\le 100$ members are materialized as uniform
  `(:Agent)-[:RING_EDGE { ring_tier: 'COALITION_RING', weight }]->(:Agent)` edges,
  allowing native `shortestPath` algorithms to traverse across multiple ring tiers.
  However, for coalitions exceeding the 100-member cap, ties are stored purely as bipartite
  `(:Agent)-[:MEMBER_OF]->(:Coalition)` to bound $O(n^2)$ write amplification.
  The query-time 2-hop fallback only resolves **direct pairwise lookups** between two specific
  endpoints. If an unmaterialized coalition (> 100 members) sits as an *intermediate link* in
  a multi-hop chain (e.g. $A \to B \to C \to D$ where $B \leftrightarrow C$ is the unmaterialized
  coalition), `shortestPath(A, D)` will not resolve the missing intermediate hop. Multi-hop
  propagation queries through large unmaterialized coalitions are a documented scope boundary.

---

## 8. Coalitions & Shadow Coalitions — disclosure trigger to watch for

- Coalitions pool capital and share yield via `KairoCoalition.sol`.
- **Shadow Coalitions** may form privately, but if a shadow coalition
  crosses **40% of an opinion asset's outstanding supply**,
  `KairoMarket.sol` auto-triggers `enforceDisclosure()`.
- This repo's Neo4j graph and PostgreSQL position data are the natural place
  to *detect* approaching 40% concentration ahead of/alongside the on-chain
  trigger (useful for the Sentiment index's threat-matrix endpoint), and to
  log disclosure compliance/failure for reputation slashing purposes.

---

## 9. Lawful Cheating Framework (LCF) — what's permitted vs. prohibited

Relevant because reputation and SVC computations should treat these
differently — permitted LCF behavior is priced/taxed, not treated as fraud;
prohibited behavior is a hard protocol violation.

| Permitted (Lawful Cheating) | Prohibited (Protocol Violation) |
|---|---|
| Bluffing a position to test sentiment | Fabricating external data/resolution inputs |
| Signaling coalition interest, then declining | Sybil attacks (multiple identities faking adoption) |
| Public argument-based pressure campaigns | Direct attacks on market infrastructure |
| Strategically timing when a position goes public | Manipulating off-chain feeds Tier I markets rely on |
| Visibly entering a position to signal conviction | Targeting a participant's real-world identity |

Every LCF-flagged behavior should be recorded and attributed (feeds
Behavioral Consistency in reputation), not just penalized silently — this
preserves the protocol's transparency argument (see whitepaper §5.3 / §9.1).

---

## 10. Reputation Merkle Commitment (on-chain interface)

`KairoReputation.sol`:
```
updateReputationRoot(bytes32 merkleRoot, uint256 epochId)
verifyReputation(uint256 agentId, uint256 score, bytes32[] proof) returns (bool)
```

This repo is responsible for computing per-agent scores (0–1000), building
the Merkle tree, and supplying the root for on-chain submission each epoch,
plus serving proofs so on-chain calls to `verifyReputation()` succeed.
Coordinate score-range/encoding changes with the `kairo-contracts-evm` owner
before shipping — a mismatch breaks proof verification silently.
