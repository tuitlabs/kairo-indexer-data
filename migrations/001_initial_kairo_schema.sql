-- Schema Migration: 001_initial_kairo_schema.sql
-- Foundational PostgreSQL schema for kairo-indexer-data

-- 1. Indexer Sync State (tracks ingestion cursor and block progress)
CREATE TABLE IF NOT EXISTS indexer_sync_state (
    chain_id BIGINT PRIMARY KEY,
    last_indexed_block BIGINT NOT NULL,
    last_indexed_block_hash VARCHAR(66) NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 2. Raw Block Events (Immutable append-only log for idempotency & replays)
CREATE TABLE IF NOT EXISTS block_events (
    id BIGSERIAL PRIMARY KEY,
    chain_id BIGINT NOT NULL,
    block_number BIGINT NOT NULL,
    block_hash VARCHAR(66) NOT NULL,
    tx_hash VARCHAR(66) NOT NULL,
    tx_index INT NOT NULL,
    log_index INT NOT NULL,
    contract_address VARCHAR(42) NOT NULL,
    event_name VARCHAR(64) NOT NULL,
    event_signature VARCHAR(66) NOT NULL,
    payload JSONB NOT NULL,
    block_timestamp TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_block_log UNIQUE (chain_id, block_number, log_index)
);

CREATE INDEX IF NOT EXISTS idx_block_events_contract ON block_events (contract_address, block_number);
CREATE INDEX IF NOT EXISTS idx_block_events_name ON block_events (event_name, block_timestamp);

-- 3. Stance Markets
CREATE TABLE IF NOT EXISTS markets (
    market_id VARCHAR(66) PRIMARY KEY, -- 0x-prefixed bytes32 hex
    stance_uri TEXT NOT NULL,
    pool_address VARCHAR(42) NOT NULL,
    curator_address VARCHAR(42) NOT NULL,
    tier SMALLINT NOT NULL DEFAULT 2,   -- 1 = Factual, 2 = Consensus, 3 = Banter
    status VARCHAR(32) NOT NULL DEFAULT 'ACTIVE',
    total_raw_capital NUMERIC(38, 0) NOT NULL DEFAULT 0,
    total_effective_stake NUMERIC(38, 0) NOT NULL DEFAULT 0,
    current_svc NUMERIC(10, 6) NOT NULL DEFAULT 0.000000, -- Stance Volatility Coefficient
    current_price NUMERIC(38, 18) NOT NULL DEFAULT 0,
    created_at_block BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 4. Agent Trade Histories (Canonical transaction audit trail)
CREATE TABLE IF NOT EXISTS agent_trades (
    id BIGSERIAL PRIMARY KEY,
    market_id VARCHAR(66) NOT NULL REFERENCES markets(market_id) ON DELETE CASCADE,
    agent_address VARCHAR(42) NOT NULL,
    agent_id BIGINT,
    trade_type VARCHAR(8) NOT NULL CHECK (trade_type IN ('BUY', 'SELL')),
    raw_capital NUMERIC(38, 0) NOT NULL,       -- K
    token_amount NUMERIC(38, 0) NOT NULL,      -- Tokens in/out
    effective_stake NUMERIC(38, 0) NOT NULL,   -- S_eff
    credibility_score INT NOT NULL,            -- Scaled 100-1000
    execution_price NUMERIC(38, 18) NOT NULL,
    friction_tax NUMERIC(38, 0) NOT NULL DEFAULT 0,
    tx_hash VARCHAR(66) NOT NULL,
    block_number BIGINT NOT NULL,
    log_index INT NOT NULL,
    block_timestamp TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_trade_log UNIQUE (tx_hash, log_index)
);

CREATE INDEX IF NOT EXISTS idx_trades_agent ON agent_trades (agent_address, block_timestamp);
CREATE INDEX IF NOT EXISTS idx_trades_market ON agent_trades (market_id, block_timestamp);

-- 5. Stance Positions (Current aggregated balance & conviction per agent)
CREATE TABLE IF NOT EXISTS stance_positions (
    id BIGSERIAL PRIMARY KEY,
    market_id VARCHAR(66) NOT NULL REFERENCES markets(market_id) ON DELETE CASCADE,
    agent_address VARCHAR(42) NOT NULL,
    agent_id BIGINT,
    token_balance NUMERIC(38, 0) NOT NULL DEFAULT 0,
    raw_capital_staked NUMERIC(38, 0) NOT NULL DEFAULT 0, -- K_i
    effective_stake NUMERIC(38, 0) NOT NULL DEFAULT 0,    -- K_i * C_i^alpha
    last_credibility_score INT NOT NULL DEFAULT 500,
    last_action VARCHAR(8) CHECK (last_action IN ('BUY', 'SELL')),
    total_lifetime_flips INT NOT NULL DEFAULT 0,
    last_trade_block BIGINT NOT NULL,
    last_trade_timestamp TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_market_agent UNIQUE (market_id, agent_address)
);

CREATE INDEX IF NOT EXISTS idx_positions_agent ON stance_positions (agent_address);
CREATE INDEX IF NOT EXISTS idx_positions_market_balance ON stance_positions (market_id, token_balance DESC);

-- 6. Position Flips Log (Discrete log of action reversals for windowed V_flip / Delta_t calculation)
CREATE TABLE IF NOT EXISTS position_flips (
    id BIGSERIAL PRIMARY KEY,
    market_id VARCHAR(66) NOT NULL REFERENCES markets(market_id) ON DELETE CASCADE,
    agent_address VARCHAR(42) NOT NULL,
    agent_id BIGINT,
    previous_action VARCHAR(8) NOT NULL CHECK (previous_action IN ('BUY', 'SELL')),
    flip_action VARCHAR(8) NOT NULL CHECK (flip_action IN ('BUY', 'SELL')),
    trade_id BIGINT REFERENCES agent_trades(id) ON DELETE SET NULL,
    token_amount NUMERIC(38, 0) NOT NULL,
    remaining_balance NUMERIC(38, 0) NOT NULL,
    tx_hash VARCHAR(66) NOT NULL,
    block_number BIGINT NOT NULL,
    block_timestamp TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for rolling-window queries: COUNT(*) WHERE agent_address = $1 AND market_id = $2 AND block_timestamp >= NOW() - INTERVAL '1 hour'
CREATE INDEX IF NOT EXISTS idx_position_flips_window ON position_flips (agent_address, market_id, block_timestamp);
CREATE INDEX IF NOT EXISTS idx_position_flips_market ON position_flips (market_id, block_timestamp);

-- 7. Coalition Graphs (On-chain alliances & 40% shadow coalition tracking)
CREATE TABLE IF NOT EXISTS coalitions (
    coalition_id VARCHAR(66) PRIMARY KEY, -- 0x-prefixed bytes32 hex
    yield_bps INT NOT NULL DEFAULT 0,
    status VARCHAR(32) NOT NULL DEFAULT 'ACTIVE', -- PROPOSED, NEGOTIATING, ACTIVE, DISCLOSED
    is_shadow BOOLEAN NOT NULL DEFAULT FALSE,
    disclosure_block BIGINT,
    created_at_block BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS coalition_members (
    coalition_id VARCHAR(66) NOT NULL REFERENCES coalitions(coalition_id) ON DELETE CASCADE,
    agent_address VARCHAR(42) NOT NULL,
    agent_id BIGINT,
    share_bps INT NOT NULL DEFAULT 0,
    joined_at_block BIGINT NOT NULL,
    joined_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (coalition_id, agent_address)
);

CREATE INDEX IF NOT EXISTS idx_coalition_members_agent ON coalition_members (agent_address);

-- 8. Epoch Settlements (24h checkpoint settlements & yield splitting)
CREATE TABLE IF NOT EXISTS epoch_settlements (
    id BIGSERIAL PRIMARY KEY,
    market_id VARCHAR(66) NOT NULL REFERENCES markets(market_id) ON DELETE CASCADE,
    epoch_id BIGINT NOT NULL,
    consensus_ratio NUMERIC(38, 18) NOT NULL, -- Scaled Theta ratio
    total_effective_stake NUMERIC(38, 0) NOT NULL DEFAULT 0,
    total_raw_capital NUMERIC(38, 0) NOT NULL DEFAULT 0,
    total_yield NUMERIC(38, 0) NOT NULL DEFAULT 0,
    agent_wallet_payout NUMERIC(38, 0) NOT NULL DEFAULT 0,   -- 70%
    agent_owner_payout NUMERIC(38, 0) NOT NULL DEFAULT 0,    -- 20%
    coalition_payout NUMERIC(38, 0) NOT NULL DEFAULT 0,      -- 5%
    treasury_payout NUMERIC(38, 0) NOT NULL DEFAULT 0,       -- 5%
    block_number BIGINT NOT NULL,
    tx_hash VARCHAR(66) NOT NULL,
    settled_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_epoch_market UNIQUE (market_id, epoch_id)
);

-- 9. Deception / Friction Tax Events (LCF friction capture)
CREATE TABLE IF NOT EXISTS deception_tax_events (
    id BIGSERIAL PRIMARY KEY,
    market_id VARCHAR(66) NOT NULL REFERENCES markets(market_id) ON DELETE CASCADE,
    agent_address VARCHAR(42) NOT NULL,
    tax_amount NUMERIC(38, 0) NOT NULL,
    reported_flip_velocity INT NOT NULL,       -- On-chain emitted V_flip / Delta_t
    calculated_flip_velocity INT,              -- Off-chain verified count from position_flips
    credibility_score INT NOT NULL,
    tx_hash VARCHAR(66) NOT NULL,
    block_number BIGINT NOT NULL,
    block_timestamp TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_deception_tax_agent ON deception_tax_events (agent_address, block_timestamp);
