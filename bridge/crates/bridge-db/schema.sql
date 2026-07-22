-- bridge-db schema. Idempotent: re-run on every startup (CREATE ... IF NOT EXISTS).
-- The DB is the single source of truth for transaction history + allowlists.

-- One row per cross-chain transfer (the off-chain mirror of a `Sent` event),
-- plus its lifecycle status. Parameters are IMMUTABLE once written; only the
-- status / claim_tx / updated_at columns ever change after insert.
CREATE TABLE IF NOT EXISTS submissions (
    submission_id   TEXT PRIMARY KEY,
    debridge_id     TEXT        NOT NULL,
    amount          TEXT        NOT NULL,          -- uint256 as decimal string
    chain_id_from   BIGINT      NOT NULL,
    chain_id_to     BIGINT      NOT NULL,
    nonce           BIGINT      NOT NULL,
    receiver        TEXT        NOT NULL,
    auto_params     TEXT        NOT NULL DEFAULT '0x',
    native_sender   TEXT        NOT NULL DEFAULT '0x',
    status          TEXT        NOT NULL DEFAULT 'signed',  -- 'signed' | 'claimed'
    claim_tx        TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Refund groundwork (see crates/indexer): 'none' | 'eligible' | 'refunded'.
    -- Flipped to 'eligible' by the indexer's periodic sweep once a transfer is
    -- past the refund timeout and still unclaimed. Purely informational today —
    -- no on-chain refund mechanism exists yet; 'refunded'/refund_tx are reserved
    -- for that follow-up.
    refund_status   TEXT        NOT NULL DEFAULT 'none',
    refund_tx       TEXT
);
CREATE INDEX IF NOT EXISTS idx_submissions_to     ON submissions (chain_id_to);
CREATE INDEX IF NOT EXISTS idx_submissions_status ON submissions (status);
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS refund_status TEXT NOT NULL DEFAULT 'none';
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS refund_tx TEXT;

-- Collected validator signatures, deduped by signer per submission.
CREATE TABLE IF NOT EXISTS signatures (
    submission_id   TEXT        NOT NULL REFERENCES submissions (submission_id) ON DELETE CASCADE,
    signer          TEXT        NOT NULL,
    signature       TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (submission_id, signer)
);

-- Allowlist: which ERC-20s may bridge. `debridge_id` is keccak256(chain_id, token),
-- precomputed so the validator/keeper match a `Sent` event by one hash lookup.
CREATE TABLE IF NOT EXISTS allowed_tokens (
    chain_id        BIGINT      NOT NULL,
    token_address   TEXT        NOT NULL,
    debridge_id     TEXT        NOT NULL,
    symbol          TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (chain_id, token_address)
);
CREATE INDEX IF NOT EXISTS idx_allowed_tokens_debridge ON allowed_tokens (debridge_id);

-- Allowlist: which directed source->target chain pairs may bridge.
CREATE TABLE IF NOT EXISTS allowed_chains (
    chain_id_from   BIGINT      NOT NULL,
    chain_id_to     BIGINT      NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (chain_id_from, chain_id_to)
);

-- Same-chain swaps (SwapPool.Swapped), mirrored by the indexer. Swaps are
-- atomic (revert on failure), so this only ever holds completed swaps.
CREATE TABLE IF NOT EXISTS swaps (
    id              BIGSERIAL   PRIMARY KEY,
    chain_id        BIGINT      NOT NULL,
    tx_hash         TEXT        NOT NULL,
    log_index       INT         NOT NULL,
    sender          TEXT        NOT NULL,
    receiver        TEXT        NOT NULL,
    token_in        TEXT        NOT NULL,
    token_out       TEXT        NOT NULL,
    amount_in       TEXT        NOT NULL,
    amount_out      TEXT        NOT NULL,
    block_number    BIGINT      NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (chain_id, tx_hash, log_index)
);
CREATE INDEX IF NOT EXISTS idx_swaps_chain  ON swaps (chain_id);
CREATE INDEX IF NOT EXISTS idx_swaps_sender ON swaps (sender);

-- Swap intent + destination outcome for a SwapRouter.swapAndBridge transfer,
-- one row per bridge submission. Populated by the indexer from SwapBridged
-- (source leg) and Finalized/FinalizeFallback (destination leg).
CREATE TABLE IF NOT EXISTS swap_bridges (
    submission_id       TEXT        PRIMARY KEY REFERENCES submissions (submission_id) ON DELETE CASCADE,
    token_in            TEXT        NOT NULL,
    amount_in           TEXT        NOT NULL,
    stable_out          TEXT        NOT NULL,
    final_token         TEXT        NOT NULL,
    final_receiver      TEXT        NOT NULL,
    finalize_tx         TEXT,
    finalize_amount_out TEXT,
    finalize_fallback   BOOLEAN,
    finalized_at        TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Per-chain resume cursor for the indexer (keeps it stateless/restartable,
-- unlike the validator's local-file cursor).
CREATE TABLE IF NOT EXISTS indexer_cursors (
    chain_id        BIGINT      PRIMARY KEY,
    last_block      BIGINT      NOT NULL,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
