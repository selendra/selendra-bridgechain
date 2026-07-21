//! bridge-db — the Postgres source of truth for the bridge.
//!
//! Replaces the file-per-id JSON store (`bridge-core::store`) with a real
//! database that holds **transaction history** (`submissions` + `signatures`,
//! with a lifecycle `status`) and the two **allowlists** (`allowed_tokens`,
//! `allowed_chains`). The HTTP `sig-store` service is the only process that
//! talks to it; validators/keepers/graphql reach it through that service.
//!
//! The same trust-boundary checks the file store enforced are reused verbatim
//! from `bridge-core::store` (the `abi` feature): a record's `submission_id`
//! must equal the keccak of its own params, params are immutable once stored,
//! and every signature must recover to its claimed signer.

use std::str::FromStr;

use alloy_primitives::{Address, U256};
use bridge_core::allow::{AllowedChain, AllowedToken, SubmissionHistory};
use bridge_core::store::{self, SignerSig, StoreError, SubmissionRecord};
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, PgPool};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// A trust-boundary violation (bad id, param conflict, forged signature).
    /// These map to HTTP 4xx — the caller sent something invalid.
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("db: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("bad field: {0}")]
    BadField(&'static str),
}

/// Canonical form of a submissionId used as the DB key everywhere: lowercase,
/// always `0x`-prefixed. Callers reach us with either form (the validator stores
/// `0x…`; the keeper/graphql strip the `0x` before putting it in a URL), so we
/// must normalize on every lookup or an `UPDATE`/`SELECT` silently misses.
fn norm_id(s: &str) -> String {
    let s = s.strip_prefix("0x").unwrap_or(s);
    format!("0x{}", s.to_ascii_lowercase())
}

impl DbError {
    /// True for caller-input errors (HTTP 4xx); false for server/IO faults (5xx).
    pub fn is_client_error(&self) -> bool {
        match self {
            DbError::Store(e) => !matches!(e, StoreError::Io(_) | StoreError::Json(_)),
            DbError::BadField(_) => true,
            DbError::Sqlx(_) => false,
        }
    }
}

/// Columns of the `submissions` table.
#[derive(FromRow)]
struct SubmissionRow {
    submission_id: String,
    debridge_id: String,
    amount: String,
    chain_id_from: i64,
    chain_id_to: i64,
    nonce: i64,
    receiver: String,
    auto_params: String,
    native_sender: String,
    status: String,
    claim_tx: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(FromRow)]
struct SigRow {
    submission_id: String,
    signer: String,
    signature: String,
}

impl SubmissionRow {
    fn into_record(self, signatures: Vec<SignerSig>) -> SubmissionRecord {
        SubmissionRecord {
            submission_id: self.submission_id,
            debridge_id: self.debridge_id,
            amount: self.amount,
            chain_id_from: self.chain_id_from as u64,
            chain_id_to: self.chain_id_to as u64,
            nonce: self.nonce as u64,
            receiver: self.receiver,
            auto_params: self.auto_params,
            native_sender: self.native_sender,
            signatures,
        }
    }

    fn into_history(self, signature_count: i64) -> SubmissionHistory {
        SubmissionHistory {
            submission_id: self.submission_id,
            debridge_id: self.debridge_id,
            amount: self.amount,
            chain_id_from: self.chain_id_from as u64,
            chain_id_to: self.chain_id_to as u64,
            nonce: self.nonce as u64,
            receiver: self.receiver,
            status: self.status,
            claim_tx: self.claim_tx,
            signature_count,
            created_at: self.created_at.to_rfc3339(),
            updated_at: self.updated_at.to_rfc3339(),
        }
    }
}

/// A handle to the bridge database (cheap to clone — wraps a connection pool).
#[derive(Clone)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Connect (with a small pool) and ensure the schema exists. Retries for a
    /// short window so the service tolerates Postgres still starting up (docker
    /// healthcheck races, `initdb`'s bootstrap restart, compose ordering).
    pub async fn connect(url: &str) -> Result<Db, DbError> {
        let mut last: Option<sqlx::Error> = None;
        for attempt in 1..=30 {
            match PgPoolOptions::new().max_connections(10).connect(url).await {
                Ok(pool) => {
                    let db = Db { pool };
                    db.migrate().await?;
                    return Ok(db);
                }
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "Postgres not ready; retrying in 500ms");
                    last = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        }
        Err(DbError::Sqlx(last.expect("loop ran at least once")))
    }

    /// Apply the idempotent schema. Safe to call on every startup.
    pub async fn migrate(&self) -> Result<(), DbError> {
        sqlx::raw_sql(include_str!("../schema.sql")).execute(&self.pool).await?;
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Signature store (source of truth) — same contract as bridge-core::store.
    // ---------------------------------------------------------------------

    /// Insert/merge a record + one signature, enforcing the trust boundary.
    /// Returns the resulting record with all known signatures.
    pub async fn upsert_signature(
        &self,
        record: SubmissionRecord,
        sig: SignerSig,
    ) -> Result<SubmissionRecord, DbError> {
        // Guard the id before it is used as a key (and as a sig-store URL segment).
        if !store::is_valid_submission_id(&record.submission_id) {
            return Err(StoreError::BadField("submission_id").into());
        }
        // (1) id <-> params binding and (3) signature authenticity.
        let computed = store::canonical_submission_id(&record)?;
        let claimed = alloy_primitives::B256::from_str(&record.submission_id)
            .map_err(|_| StoreError::BadField("submission_id"))?;
        if computed != claimed {
            return Err(StoreError::IdMismatch {
                claimed: format!("{claimed:#x}"),
                computed: format!("{computed:#x}"),
            }
            .into());
        }
        store::verify_signature(computed, &sig)?;

        let id = norm_id(&record.submission_id);
        let mut tx = self.pool.begin().await?;

        // (2) params are immutable: insert once; on a re-POST verify the stored
        // params still match, else reject (poisoning defense).
        let existing: Option<SubmissionRow> =
            sqlx::query_as("SELECT * FROM submissions WHERE submission_id = $1")
                .bind(&id)
                .fetch_optional(&mut *tx)
                .await?;

        if let Some(row) = existing {
            // Reuse bridge-core's param-equality check rather than re-deriving the
            // field list here. Compare against a copy of `record` whose id is
            // normalized the same way `row.submission_id` is (both `0x`-prefixed,
            // lowercase) — `record.submission_id` itself may lack the prefix.
            let mut incoming = record.clone();
            incoming.submission_id = id.clone();
            if !store::same_params(&row.into_record(Vec::new()), &incoming) {
                return Err(StoreError::ParamsConflict(record.submission_id.clone()).into());
            }
        } else {
            sqlx::query(
                "INSERT INTO submissions \
                 (submission_id, debridge_id, amount, chain_id_from, chain_id_to, nonce, \
                  receiver, auto_params, native_sender) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
            )
            .bind(&id)
            .bind(record.debridge_id.to_ascii_lowercase())
            .bind(&record.amount)
            .bind(record.chain_id_from as i64)
            .bind(record.chain_id_to as i64)
            .bind(record.nonce as i64)
            .bind(record.receiver.to_ascii_lowercase())
            .bind(record.auto_params.to_ascii_lowercase())
            .bind(record.native_sender.to_ascii_lowercase())
            .execute(&mut *tx)
            .await?;
        }

        // Merge the signature, deduped by signer.
        let inserted = sqlx::query(
            "INSERT INTO signatures (submission_id, signer, signature) VALUES ($1,$2,$3) \
             ON CONFLICT (submission_id, signer) DO NOTHING",
        )
        .bind(&id)
        .bind(sig.signer.to_ascii_lowercase())
        .bind(&sig.signature)
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() > 0 {
            sqlx::query("UPDATE submissions SET updated_at = now() WHERE submission_id = $1")
                .bind(&id)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        self.load(&id)
            .await?
            .ok_or(DbError::BadField("submission_id"))
    }

    /// Load one record (params + signatures) by submissionId.
    pub async fn load(&self, submission_id: &str) -> Result<Option<SubmissionRecord>, DbError> {
        if !store::is_valid_submission_id(submission_id) {
            return Ok(None);
        }
        let id = norm_id(submission_id);
        let row: Option<SubmissionRow> =
            sqlx::query_as("SELECT * FROM submissions WHERE submission_id = $1")
                .bind(&id)
                .fetch_optional(&self.pool)
                .await?;
        let Some(row) = row else { return Ok(None) };

        let sigs: Vec<SigRow> = sqlx::query_as(
            "SELECT submission_id, signer, signature FROM signatures \
             WHERE submission_id = $1 ORDER BY signer",
        )
        .bind(&id)
        .fetch_all(&self.pool)
        .await?;
        let sigs = sigs
            .into_iter()
            .map(|s| SignerSig { signer: s.signer, signature: s.signature })
            .collect();
        Ok(Some(row.into_record(sigs)))
    }

    /// Load every record (params + signatures). Two queries + an in-memory join,
    /// so the keeper's poll is one round trip per table rather than N+1.
    pub async fn load_all(&self) -> Result<Vec<SubmissionRecord>, DbError> {
        let rows: Vec<SubmissionRow> =
            sqlx::query_as("SELECT * FROM submissions ORDER BY created_at").fetch_all(&self.pool).await?;
        let sigs: Vec<SigRow> = sqlx::query_as(
            "SELECT submission_id, signer, signature FROM signatures ORDER BY submission_id, signer",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut by_id: std::collections::HashMap<String, Vec<SignerSig>> = std::collections::HashMap::new();
        for s in sigs {
            by_id
                .entry(s.submission_id)
                .or_default()
                .push(SignerSig { signer: s.signer, signature: s.signature });
        }
        Ok(rows
            .into_iter()
            .map(|r| {
                let sigs = by_id.remove(&r.submission_id).unwrap_or_default();
                r.into_record(sigs)
            })
            .collect())
    }

    // ---------------------------------------------------------------------
    // Transaction history (status).
    // ---------------------------------------------------------------------

    /// Mark a submission `claimed`, recording the target-chain claim tx hash.
    pub async fn mark_claimed(&self, submission_id: &str, claim_tx: &str) -> Result<(), DbError> {
        if !store::is_valid_submission_id(submission_id) {
            return Err(DbError::BadField("submission_id"));
        }
        let id = norm_id(submission_id);
        sqlx::query(
            "UPDATE submissions SET status = 'claimed', claim_tx = $2, updated_at = now() \
             WHERE submission_id = $1",
        )
        .bind(&id)
        .bind(claim_tx)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The transaction-history view: every submission with its status, claim tx,
    /// signature count, and timestamps. Newest first.
    pub async fn history(&self) -> Result<Vec<SubmissionHistory>, DbError> {
        let rows: Vec<SubmissionRow> =
            sqlx::query_as("SELECT * FROM submissions ORDER BY created_at DESC").fetch_all(&self.pool).await?;
        let counts: Vec<(String, i64)> = sqlx::query_as(
            "SELECT submission_id, COUNT(*)::BIGINT FROM signatures GROUP BY submission_id",
        )
        .fetch_all(&self.pool)
        .await?;
        let counts: std::collections::HashMap<String, i64> = counts.into_iter().collect();
        Ok(rows
            .into_iter()
            .map(|r| {
                let n = counts.get(&r.submission_id).copied().unwrap_or(0);
                r.into_history(n)
            })
            .collect())
    }

    // ---------------------------------------------------------------------
    // Allowlists.
    // ---------------------------------------------------------------------

    /// Whitelist a token. `debridge_id = keccak256(chain_id, token)` is derived
    /// here so a caller can't pin a token onto the wrong hash. Upsert by key.
    pub async fn add_allowed_token(
        &self,
        chain_id: u64,
        token: &str,
        symbol: Option<&str>,
    ) -> Result<AllowedToken, DbError> {
        let addr = Address::from_str(token.trim()).map_err(|_| DbError::BadField("token"))?;
        let token_lc = format!("{addr:#x}");
        let debridge_id = format!("{:#x}", bridge_core::debridge_id(U256::from(chain_id), addr));
        sqlx::query(
            "INSERT INTO allowed_tokens (chain_id, token_address, debridge_id, symbol) \
             VALUES ($1,$2,$3,$4) \
             ON CONFLICT (chain_id, token_address) \
             DO UPDATE SET debridge_id = EXCLUDED.debridge_id, symbol = EXCLUDED.symbol",
        )
        .bind(chain_id as i64)
        .bind(&token_lc)
        .bind(&debridge_id)
        .bind(symbol)
        .execute(&self.pool)
        .await?;
        Ok(AllowedToken {
            chain_id,
            token: token_lc,
            debridge_id,
            symbol: symbol.map(str::to_string),
        })
    }

    /// Remove a token from the allowlist. Returns true if a row was deleted.
    pub async fn remove_allowed_token(&self, chain_id: u64, token: &str) -> Result<bool, DbError> {
        let addr = Address::from_str(token.trim()).map_err(|_| DbError::BadField("token"))?;
        let res = sqlx::query("DELETE FROM allowed_tokens WHERE chain_id = $1 AND token_address = $2")
            .bind(chain_id as i64)
            .bind(format!("{addr:#x}"))
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn list_allowed_tokens(&self) -> Result<Vec<AllowedToken>, DbError> {
        let rows: Vec<(i64, String, String, Option<String>)> = sqlx::query_as(
            "SELECT chain_id, token_address, debridge_id, symbol FROM allowed_tokens \
             ORDER BY chain_id, token_address",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(chain_id, token, debridge_id, symbol)| AllowedToken {
                chain_id: chain_id as u64,
                token,
                debridge_id,
                symbol,
            })
            .collect())
    }

    pub async fn add_allowed_chain(&self, from: u64, to: u64) -> Result<AllowedChain, DbError> {
        sqlx::query(
            "INSERT INTO allowed_chains (chain_id_from, chain_id_to) VALUES ($1,$2) \
             ON CONFLICT (chain_id_from, chain_id_to) DO NOTHING",
        )
        .bind(from as i64)
        .bind(to as i64)
        .execute(&self.pool)
        .await?;
        Ok(AllowedChain { chain_id_from: from, chain_id_to: to })
    }

    pub async fn remove_allowed_chain(&self, from: u64, to: u64) -> Result<bool, DbError> {
        let res =
            sqlx::query("DELETE FROM allowed_chains WHERE chain_id_from = $1 AND chain_id_to = $2")
                .bind(from as i64)
                .bind(to as i64)
                .execute(&self.pool)
                .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn list_allowed_chains(&self) -> Result<Vec<AllowedChain>, DbError> {
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT chain_id_from, chain_id_to FROM allowed_chains ORDER BY chain_id_from, chain_id_to",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(chain_id_from, chain_id_to)| AllowedChain {
                chain_id_from: chain_id_from as u64,
                chain_id_to: chain_id_to as u64,
            })
            .collect())
    }
}
