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
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_submissions_to     ON submissions (chain_id_to);
CREATE INDEX IF NOT EXISTS idx_submissions_status ON submissions (status);

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
