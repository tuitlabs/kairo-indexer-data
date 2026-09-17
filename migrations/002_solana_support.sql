-- Schema Migration: 002_solana_support.sql
-- Adds multi-chain support, chain_id discriminators, and Solana-native social edge storage.
--
-- ARCHITECTURAL / NUMERICAL CONVENTION NOTE ON chain_id:
-- The `chain_id` column across these tables represents the network discriminator
-- (Base Mainnet = 8453, Base Sepolia = 84532, and Solana = 101 as an internal indexer convention).
--
-- IMPORTANT / WORMHOLE PROTOCOL RECONCILIATION:
-- This internal indexer convention (101) does NOT match Wormhole's wire-format emitterChainId.
-- In the Wormhole core protocol (and as used by KairoBridge.sol / kairo_bridge_receiver):
--   * Wormhole Solana Chain ID = 1
--   * Wormhole Base Chain ID = 30
--
-- DO NOT compare `chain_id` directly against raw Wormhole VAA `emitterChainId` fields.
-- Any Wormhole VAA ingestion or verification must translate between Wormhole's wire IDs
-- (Solana: 1, Base: 30) and these internal database IDs (Solana: 101, Base: 8453/84532)
-- using the protocol mapping helper (e.g. wormhole_to_internal_chain_id).

-- 1. Add chain_id to markets and widen curator/pool columns
ALTER TABLE markets 
    ADD COLUMN IF NOT EXISTS chain_id BIGINT NOT NULL DEFAULT 8453,
    ALTER COLUMN pool_address DROP NOT NULL, -- Solana banter markets do not use EVM pool addresses
    ALTER COLUMN curator_address TYPE VARCHAR(64);

-- 2. Add chain_id and widen address/hash columns on all referencing tables BEFORE recreating FKs
ALTER TABLE agent_trades
    ADD COLUMN IF NOT EXISTS chain_id BIGINT NOT NULL DEFAULT 8453,
    ALTER COLUMN agent_address TYPE VARCHAR(64),
    ALTER COLUMN tx_hash TYPE VARCHAR(90);

ALTER TABLE stance_positions
    ADD COLUMN IF NOT EXISTS chain_id BIGINT NOT NULL DEFAULT 8453,
    ALTER COLUMN agent_address TYPE VARCHAR(64);

ALTER TABLE position_flips
    ADD COLUMN IF NOT EXISTS chain_id BIGINT NOT NULL DEFAULT 8453,
    ALTER COLUMN agent_address TYPE VARCHAR(64),
    ALTER COLUMN tx_hash TYPE VARCHAR(90);

ALTER TABLE epoch_settlements
    ADD COLUMN IF NOT EXISTS chain_id BIGINT NOT NULL DEFAULT 8453,
    ALTER COLUMN tx_hash TYPE VARCHAR(90);

ALTER TABLE deception_tax_events
    ADD COLUMN IF NOT EXISTS chain_id BIGINT NOT NULL DEFAULT 8453,
    ALTER COLUMN agent_address TYPE VARCHAR(64),
    ALTER COLUMN tx_hash TYPE VARCHAR(90);

-- 3. Add chain_id and widen columns on non-market tables
ALTER TABLE coalitions
    ADD COLUMN IF NOT EXISTS chain_id BIGINT NOT NULL DEFAULT 8453;

ALTER TABLE coalition_members
    ALTER COLUMN agent_address TYPE VARCHAR(64);

ALTER TABLE block_events
    ALTER COLUMN contract_address TYPE VARCHAR(64),
    ALTER COLUMN tx_hash TYPE VARCHAR(90);

-- 4. Reconfigure markets primary key to composite (chain_id, market_id)
-- Note: DROP CONSTRAINT ... CASCADE drops all 5 legacy single-column FKs referencing markets(market_id)
-- from agent_trades, stance_positions, position_flips, epoch_settlements, and deception_tax_events.
ALTER TABLE markets DROP CONSTRAINT IF EXISTS markets_pkey CASCADE;
ALTER TABLE markets ADD CONSTRAINT markets_pkey PRIMARY KEY (chain_id, market_id);

-- 5. Re-create composite foreign keys on all 5 referencing tables to restore full referential integrity
ALTER TABLE agent_trades DROP CONSTRAINT IF EXISTS fk_agent_trades_market;
ALTER TABLE agent_trades
    ADD CONSTRAINT fk_agent_trades_market
    FOREIGN KEY (chain_id, market_id)
    REFERENCES markets(chain_id, market_id)
    ON DELETE CASCADE;

ALTER TABLE stance_positions DROP CONSTRAINT IF EXISTS fk_stance_positions_market;
ALTER TABLE stance_positions
    ADD CONSTRAINT fk_stance_positions_market
    FOREIGN KEY (chain_id, market_id)
    REFERENCES markets(chain_id, market_id)
    ON DELETE CASCADE;

ALTER TABLE position_flips DROP CONSTRAINT IF EXISTS fk_position_flips_market;
ALTER TABLE position_flips
    ADD CONSTRAINT fk_position_flips_market
    FOREIGN KEY (chain_id, market_id)
    REFERENCES markets(chain_id, market_id)
    ON DELETE CASCADE;

ALTER TABLE epoch_settlements DROP CONSTRAINT IF EXISTS fk_epoch_settlements_market;
ALTER TABLE epoch_settlements
    ADD CONSTRAINT fk_epoch_settlements_market
    FOREIGN KEY (chain_id, market_id)
    REFERENCES markets(chain_id, market_id)
    ON DELETE CASCADE;

ALTER TABLE deception_tax_events DROP CONSTRAINT IF EXISTS fk_deception_tax_events_market;
ALTER TABLE deception_tax_events
    ADD CONSTRAINT fk_deception_tax_events_market
    FOREIGN KEY (chain_id, market_id)
    REFERENCES markets(chain_id, market_id)
    ON DELETE CASCADE;

-- 6. Update unique constraints to be chain-aware
ALTER TABLE stance_positions DROP CONSTRAINT IF EXISTS uq_market_agent;
ALTER TABLE stance_positions DROP CONSTRAINT IF EXISTS uq_chain_market_agent;
ALTER TABLE stance_positions ADD CONSTRAINT uq_chain_market_agent UNIQUE (chain_id, market_id, agent_address);

ALTER TABLE epoch_settlements DROP CONSTRAINT IF EXISTS uq_epoch_market;
ALTER TABLE epoch_settlements DROP CONSTRAINT IF EXISTS uq_chain_epoch_market;
ALTER TABLE epoch_settlements ADD CONSTRAINT uq_chain_epoch_market UNIQUE (chain_id, market_id, epoch_id);

-- 7. Dedicated Table for Solana Social Edges (kairo_social)
-- Seeds: [b"social_edge", authority.key().as_ref(), peer.key().as_ref()]
CREATE TABLE IF NOT EXISTS social_edges (
    chain_id BIGINT NOT NULL DEFAULT 101, -- Internal Solana discriminator (Wormhole wire ID = 1)
    authority VARCHAR(64) NOT NULL,
    peer VARCHAR(64) NOT NULL,
    ring_tier VARCHAR(32) NOT NULL,       -- 'INNER_CIRCLE', 'SOCIAL_RING', 'PUBLIC'
    weight_bps INT NOT NULL DEFAULT 0,
    interaction_count BIGINT NOT NULL DEFAULT 0,
    last_updated_slot BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (chain_id, authority, peer)
);

CREATE INDEX IF NOT EXISTS idx_social_edges_peer ON social_edges (chain_id, peer);
CREATE INDEX IF NOT EXISTS idx_social_edges_tier ON social_edges (chain_id, ring_tier);
