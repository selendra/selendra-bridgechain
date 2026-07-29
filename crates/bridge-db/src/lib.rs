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
use bridge_core::allow::{AllowedChain, AllowedToken, SubmissionHistory, SwapBridgeInfo, SwapRecord};
use bridge_core::store::{self, SigKind, SignerSig, StoreError, SubmissionRecord};
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
    refund_status: String,
    refund_tx: Option<String>,
    cancel_tx: Option<String>,
    token: Option<String>,
}

#[derive(FromRow)]
struct SwapBridgeRow {
    submission_id: String,
    token_in: String,
    amount_in: String,
    stable_out: String,
    final_token: String,
    final_receiver: String,
    finalize_tx: Option<String>,
    finalize_amount_out: Option<String>,
    finalize_fallback: Option<bool>,
    finalized_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl SwapBridgeRow {
    fn into_info(self) -> SwapBridgeInfo {
        SwapBridgeInfo {
            token_in: self.token_in,
            amount_in: self.amount_in,
            stable_out: self.stable_out,
            final_token: self.final_token,
            final_receiver: self.final_receiver,
            finalize_tx: self.finalize_tx,
            finalize_amount_out: self.finalize_amount_out,
            finalize_fallback: self.finalize_fallback,
            finalized_at: self.finalized_at.map(|t| t.to_rfc3339()),
        }
    }
}

#[derive(FromRow)]
struct SwapRow {
    chain_id: i64,
    tx_hash: String,
    log_index: i32,
    sender: String,
    receiver: String,
    token_in: String,
    token_out: String,
    amount_in: String,
    amount_out: String,
    block_number: i64,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl SwapRow {
    fn into_record(self) -> SwapRecord {
        SwapRecord {
            chain_id: self.chain_id as u64,
            tx_hash: self.tx_hash,
            log_index: self.log_index as i64,
            sender: self.sender,
            receiver: self.receiver,
            token_in: self.token_in,
            token_out: self.token_out,
            amount_in: self.amount_in,
            amount_out: self.amount_out,
            block_number: self.block_number as u64,
            created_at: self.created_at.to_rfc3339(),
        }
    }
}

#[derive(FromRow)]
struct SigRow {
    submission_id: String,
    /// `transfer` (from the `signatures` table) | `cancel` | `refund`.
    kind: String,
    signer: String,
    signature: String,
}

impl SubmissionRow {
    fn into_record(self, sigs: Attestations) -> SubmissionRecord {
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
            token: self.token.unwrap_or_default(),
            signatures: sigs.transfer,
            cancel_signatures: sigs.cancel,
            refund_signatures: sigs.refund,
        }
    }

    fn into_history(
        self,
        signature_count: i64,
        cancel_signature_count: i64,
        refund_signature_count: i64,
        swap_intent: Option<SwapBridgeInfo>,
    ) -> SubmissionHistory {
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
            stuck: self.refund_status != "none",
            refund_status: self.refund_status,
            refund_tx: self.refund_tx,
            cancel_tx: self.cancel_tx,
            token: self.token,
            cancel_signature_count,
            refund_signature_count,
            swap_intent,
        }
    }
}

/// A submission's signatures split by the domain they authorise.
#[derive(Default)]
struct Attestations {
    transfer: Vec<SignerSig>,
    cancel: Vec<SignerSig>,
    refund: Vec<SignerSig>,
}

impl Attestations {
    fn push(&mut self, kind: &str, sig: SignerSig) {
        match SigKind::parse(kind) {
            Some(SigKind::Transfer) => self.transfer.push(sig),
            Some(SigKind::Cancel) => self.cancel.push(sig),
            Some(SigKind::Refund) => self.refund.push(sig),
            // An unrecognized kind must NOT fall into a quorum bucket — least of
            // all `transfer`, which authorises a claim. Today unreachable (the
            // write path constrains `kind` to cancel/refund and the signatures
            // table contributes the literal 'transfer'), so this is defensive
            // against a future writer or a hand-edited row.
            None => {
                tracing::warn!(kind, "ignoring signature with unrecognized attestation kind");
            }
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
        // `token` is not covered by the submissionId, so it gets its own exact
        // binding (debridge_id == keccak(chain_id_from, token)) before storage.
        store::verify_token_binding(&record)?;

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
            if !store::same_params(&row.into_record(Attestations::default()), &incoming) {
                return Err(StoreError::ParamsConflict(record.submission_id.clone()).into());
            }
            // Backfill `token` if this row predates it (e.g. inserted by an older
            // indexer). Verified above, and only ever written once — never
            // overwritten, so it stays as immutable as the rest of the params.
            if !record.token.is_empty() {
                sqlx::query(
                    "UPDATE submissions SET token = $2 WHERE submission_id = $1 AND token IS NULL",
                )
                .bind(&id)
                .bind(record.token.to_ascii_lowercase())
                .execute(&mut *tx)
                .await?;
            }
        } else {
            // ON CONFLICT makes the first-insert concurrency-safe: when several
            // validators POST the same brand-new submissionId at once they all
            // SELECT no existing row and race here. Without this the loser hits a
            // duplicate-key error (a 500 that drops its signature). The id⇄params
            // binding guarantees any existing row has identical params, so the
            // conflict path only ever backfills `token` (never other fields).
            sqlx::query(
                "INSERT INTO submissions \
                 (submission_id, debridge_id, amount, chain_id_from, chain_id_to, nonce, \
                  receiver, auto_params, native_sender, token) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) \
                 ON CONFLICT (submission_id) DO UPDATE \
                   SET token = COALESCE(submissions.token, EXCLUDED.token)",
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
            .bind(Some(record.token.to_ascii_lowercase()).filter(|t| !t.is_empty()))
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
            "SELECT submission_id, 'transfer' AS kind, signer, signature FROM signatures \
               WHERE submission_id = $1 \
             UNION ALL \
             SELECT submission_id, kind, signer, signature FROM attestations \
               WHERE submission_id = $1 \
             ORDER BY kind, signer",
        )
        .bind(&id)
        .fetch_all(&self.pool)
        .await?;

        let mut collected = Attestations::default();
        for s in sigs {
            collected.push(&s.kind, SignerSig { signer: s.signer, signature: s.signature });
        }
        Ok(Some(row.into_record(collected)))
    }

    /// Load every record (params + signatures). Two queries + an in-memory join,
    /// so the keeper's poll is one round trip per table rather than N+1.
    pub async fn load_all(&self) -> Result<Vec<SubmissionRecord>, DbError> {
        let rows: Vec<SubmissionRow> =
            sqlx::query_as("SELECT * FROM submissions ORDER BY created_at").fetch_all(&self.pool).await?;
        let sigs: Vec<SigRow> = sqlx::query_as(
            "SELECT submission_id, 'transfer' AS kind, signer, signature FROM signatures \
             UNION ALL \
             SELECT submission_id, kind, signer, signature FROM attestations \
             ORDER BY submission_id, kind, signer",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut by_id: std::collections::HashMap<String, Attestations> = std::collections::HashMap::new();
        for s in sigs {
            by_id
                .entry(s.submission_id)
                .or_default()
                .push(&s.kind, SignerSig { signer: s.signer, signature: s.signature });
        }
        Ok(rows
            .into_iter()
            .map(|r| {
                let sigs = by_id.remove(&r.submission_id).unwrap_or_default();
                r.into_record(sigs)
            })
            .collect())
    }

    /// Merge a cancel/refund attestation into an existing submission.
    ///
    /// Trust boundary, exactly like [`Db::upsert_signature`] but for the other
    /// two domains: the signature must recover to its claimed signer over
    /// `kind`'s own digest. That is what stops a validator's transfer signature
    /// from being replayed to burn (`cancel`) or claw back (`refund`) a healthy
    /// transfer — those are different messages entirely.
    ///
    /// Never creates a submission: an attestation about a transfer we have never
    /// observed is meaningless, and the id⇄params binding stays the only way a
    /// row comes into existence.
    pub async fn upsert_attestation(
        &self,
        submission_id: &str,
        kind: SigKind,
        sig: SignerSig,
    ) -> Result<SubmissionRecord, DbError> {
        if !store::is_valid_submission_id(submission_id) {
            return Err(StoreError::BadField("submission_id").into());
        }
        if kind == SigKind::Transfer {
            // Transfer signatures carry the full params and go through the
            // id<->params binding; they must not sneak in via this route.
            return Err(DbError::BadField("kind"));
        }
        let id = norm_id(submission_id);

        let existing = self.load(&id).await?.ok_or(DbError::BadField("submission_id"))?;
        let parsed = alloy_primitives::B256::from_str(&existing.submission_id)
            .map_err(|_| StoreError::BadField("submission_id"))?;
        store::verify_attestation(parsed, kind, &sig)?;

        sqlx::query(
            "INSERT INTO attestations (submission_id, kind, signer, signature) \
             VALUES ($1,$2,$3,$4) ON CONFLICT (submission_id, kind, signer) DO NOTHING",
        )
        .bind(&id)
        .bind(kind.as_str())
        .bind(sig.signer.to_ascii_lowercase())
        .bind(&sig.signature)
        .execute(&self.pool)
        .await?;

        self.load(&id).await?.ok_or(DbError::BadField("submission_id"))
    }

    // ---------------------------------------------------------------------
    // Transaction history (status).
    // ---------------------------------------------------------------------

    /// Mark a submission `claimed`, recording the target-chain claim tx hash.
    ///
    /// Also clears an `eligible` refund flag: a transfer that sat past the
    /// timeout and then got claimed after all is not stuck, and leaving it
    /// flagged would show a permanent false "refund eligible" in the UI and keep
    /// validators considering it for a cancel attestation. `cancelled`/`refunded`
    /// are terminal and never reset — a claim cannot follow them (the on-chain
    /// `executed` flag makes that impossible), so seeing one would mean the two
    /// chains disagree, and quietly overwriting it would hide that.
    pub async fn mark_claimed(&self, submission_id: &str, claim_tx: &str) -> Result<(), DbError> {
        if !store::is_valid_submission_id(submission_id) {
            return Err(DbError::BadField("submission_id"));
        }
        let id = norm_id(submission_id);
        sqlx::query(
            "UPDATE submissions SET status = 'claimed', claim_tx = $2, updated_at = now(), \
                    refund_status = CASE WHEN refund_status = 'eligible' THEN 'none' \
                                         ELSE refund_status END \
             WHERE submission_id = $1",
        )
        .bind(&id)
        .bind(claim_tx)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record that the destination gate burned this transfer (`Gate.Cancelled`).
    /// This is the state that unlocks refund attestations, so it is only ever
    /// written from an observed on-chain event, never from a relayer's say-so.
    pub async fn mark_cancelled(&self, submission_id: &str, cancel_tx: &str) -> Result<(), DbError> {
        if !store::is_valid_submission_id(submission_id) {
            return Err(DbError::BadField("submission_id"));
        }
        let id = norm_id(submission_id);
        sqlx::query(
            "UPDATE submissions SET refund_status = 'cancelled', cancel_tx = $2, updated_at = now() \
             WHERE submission_id = $1 AND refund_status <> 'refunded'",
        )
        .bind(&id)
        .bind(cancel_tx)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record that the source gate returned the funds (`Gate.Refunded`).
    pub async fn mark_refunded(&self, submission_id: &str, refund_tx: &str) -> Result<(), DbError> {
        if !store::is_valid_submission_id(submission_id) {
            return Err(DbError::BadField("submission_id"));
        }
        let id = norm_id(submission_id);
        sqlx::query(
            "UPDATE submissions SET refund_status = 'refunded', refund_tx = $2, updated_at = now() \
             WHERE submission_id = $1",
        )
        .bind(&id)
        .bind(refund_tx)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The transaction-history view: every submission with its status, claim tx,
    /// signature count, refund eligibility, swap intent (if any), and timestamps.
    /// Newest first.
    pub async fn history(&self) -> Result<Vec<SubmissionHistory>, DbError> {
        let rows: Vec<SubmissionRow> =
            sqlx::query_as("SELECT * FROM submissions ORDER BY created_at DESC").fetch_all(&self.pool).await?;
        let counts: Vec<(String, i64)> = sqlx::query_as(
            "SELECT submission_id, COUNT(*)::BIGINT FROM signatures GROUP BY submission_id",
        )
        .fetch_all(&self.pool)
        .await?;
        let counts: std::collections::HashMap<String, i64> = counts.into_iter().collect();

        // Cancel/refund quorum progress, counted per domain so the UI can show
        // how far a stuck transfer has got through the refund path.
        let att_counts: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT submission_id, kind, COUNT(*)::BIGINT FROM attestations \
             GROUP BY submission_id, kind",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut cancel_counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        let mut refund_counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        for (id, kind, n) in att_counts {
            match SigKind::parse(&kind) {
                Some(SigKind::Cancel) => {
                    cancel_counts.insert(id, n);
                }
                Some(SigKind::Refund) => {
                    refund_counts.insert(id, n);
                }
                _ => {}
            }
        }

        let swap_bridges: Vec<SwapBridgeRow> = sqlx::query_as("SELECT * FROM swap_bridges")
            .fetch_all(&self.pool)
            .await?;
        let mut swap_bridges: std::collections::HashMap<String, SwapBridgeInfo> = swap_bridges
            .into_iter()
            .map(|r| (r.submission_id.clone(), r.into_info()))
            .collect();

        Ok(rows
            .into_iter()
            .map(|r| {
                let n = counts.get(&r.submission_id).copied().unwrap_or(0);
                let c = cancel_counts.get(&r.submission_id).copied().unwrap_or(0);
                let f = refund_counts.get(&r.submission_id).copied().unwrap_or(0);
                let intent = swap_bridges.remove(&r.submission_id);
                r.into_history(n, c, f, intent)
            })
            .collect())
    }

    // ---------------------------------------------------------------------
    // Indexer support: observe on-chain events independently of validator
    // signing, so a transfer is visible even before (or without) any signature.
    // ---------------------------------------------------------------------

    /// Insert a submission row on first observation of its `Sent` event, before
    /// any signature exists. Idempotent — a later `upsert_signature` call for the
    /// same id just adds signatures to this row. Enforces the same id<->params
    /// binding as `upsert_signature`, but never touches an existing row (params
    /// are immutable; if a row already exists there is nothing to update here).
    pub async fn observe_submission(&self, record: SubmissionRecord) -> Result<(), DbError> {
        if !store::is_valid_submission_id(&record.submission_id) {
            return Err(StoreError::BadField("submission_id").into());
        }
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

        store::verify_token_binding(&record)?;

        let id = norm_id(&record.submission_id);
        let token = Some(record.token.to_ascii_lowercase()).filter(|t| !t.is_empty());
        sqlx::query(
            "INSERT INTO submissions \
             (submission_id, debridge_id, amount, chain_id_from, chain_id_to, nonce, \
              receiver, auto_params, native_sender, token) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) \
             ON CONFLICT (submission_id) DO UPDATE SET token = COALESCE(submissions.token, EXCLUDED.token)",
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
        .bind(token)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record a completed same-chain swap (`SwapPool.Swapped`). Idempotent on
    /// `(chain_id, tx_hash, log_index)` so a re-scanned block range is harmless.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_swap(
        &self,
        chain_id: u64,
        tx_hash: &str,
        log_index: i64,
        sender: &str,
        receiver: &str,
        token_in: &str,
        token_out: &str,
        amount_in: &str,
        amount_out: &str,
        block_number: u64,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO swaps \
             (chain_id, tx_hash, log_index, sender, receiver, token_in, token_out, \
              amount_in, amount_out, block_number) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) \
             ON CONFLICT (chain_id, tx_hash, log_index) DO NOTHING",
        )
        .bind(chain_id as i64)
        .bind(tx_hash.to_ascii_lowercase())
        .bind(log_index as i32)
        .bind(sender.to_ascii_lowercase())
        .bind(receiver.to_ascii_lowercase())
        .bind(token_in.to_ascii_lowercase())
        .bind(token_out.to_ascii_lowercase())
        .bind(amount_in)
        .bind(amount_out)
        .bind(block_number as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_swaps(&self, chain_id: Option<u64>, limit: i64) -> Result<Vec<SwapRecord>, DbError> {
        let rows: Vec<SwapRow> = match chain_id {
            Some(c) => {
                sqlx::query_as(
                    "SELECT * FROM swaps WHERE chain_id = $1 ORDER BY created_at DESC LIMIT $2",
                )
                .bind(c as i64)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as("SELECT * FROM swaps ORDER BY created_at DESC LIMIT $1")
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        Ok(rows.into_iter().map(SwapRow::into_record).collect())
    }

    /// Record the source-leg swap intent of a `SwapRouter.swapAndBridge` call
    /// (the `SwapBridged` event), keyed by the bridge submission it produced.
    /// The submission row itself must already exist (inserted by
    /// `observe_submission` from the paired `Sent` event in the same tx).
    pub async fn record_swap_bridge_intent(
        &self,
        submission_id: &str,
        token_in: &str,
        amount_in: &str,
        stable_out: &str,
        final_token: &str,
        final_receiver: &str,
    ) -> Result<(), DbError> {
        let id = norm_id(submission_id);
        sqlx::query(
            "INSERT INTO swap_bridges \
             (submission_id, token_in, amount_in, stable_out, final_token, final_receiver) \
             VALUES ($1,$2,$3,$4,$5,$6) \
             ON CONFLICT (submission_id) DO NOTHING",
        )
        .bind(&id)
        .bind(token_in.to_ascii_lowercase())
        .bind(amount_in)
        .bind(stable_out)
        .bind(final_token.to_ascii_lowercase())
        .bind(final_receiver.to_ascii_lowercase())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record the destination-leg outcome (`Finalized` / `FinalizeFallback`) of a
    /// swap-bridge. `fallback = true` means the destination swap failed and the
    /// stable was delivered directly instead.
    pub async fn record_finalized(
        &self,
        submission_id: &str,
        finalize_tx: &str,
        amount_out: &str,
        fallback: bool,
    ) -> Result<(), DbError> {
        let id = norm_id(submission_id);
        sqlx::query(
            "UPDATE swap_bridges SET finalize_tx = $2, finalize_amount_out = $3, \
             finalize_fallback = $4, finalized_at = now() WHERE submission_id = $1",
        )
        .bind(&id)
        .bind(finalize_tx)
        .bind(amount_out)
        .bind(fallback)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Flip `refund_status` from `'none'` to `'eligible'` for submissions that
    /// are still unclaimed and older than `timeout`.
    ///
    /// This only nominates candidates — it moves no funds and authorises nothing.
    /// A validator independently re-checks the destination gate (`executed` must
    /// still be false) before it will attest a cancel, so a wrong or manipulated
    /// timestamp here can at most cause a needless look, never a payout.
    pub async fn sweep_refund_eligible(&self, timeout: chrono::Duration) -> Result<u64, DbError> {
        let cutoff = chrono::Utc::now() - timeout;
        let res = sqlx::query(
            "UPDATE submissions SET refund_status = 'eligible', updated_at = now() \
             WHERE status <> 'claimed' AND refund_status = 'none' AND created_at < $1",
        )
        .bind(cutoff)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// Submissions a refund relayer should look at: flagged stuck, not yet
    /// refunded, and destined for / originating from the given chain.
    ///
    /// Deliberately returns candidates rather than decisions — the caller must
    /// still verify both chains on-chain before signing anything.
    pub async fn refund_candidates(&self) -> Result<Vec<SubmissionRecord>, DbError> {
        let rows: Vec<SubmissionRow> = sqlx::query_as(
            "SELECT * FROM submissions \
             WHERE status <> 'claimed' AND refund_status IN ('eligible','cancelled') \
             ORDER BY created_at",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let id = row.submission_id.clone();
            // reuse `load` so each record arrives with all three signature sets
            if let Some(rec) = self.load(&id).await? {
                out.push(rec);
            }
        }
        Ok(out)
    }

    /// Resume cursor for one chain (indexer). `None` if never persisted.
    pub async fn get_cursor(&self, chain_id: u64) -> Result<Option<u64>, DbError> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT last_block FROM indexer_cursors WHERE chain_id = $1")
                .bind(chain_id as i64)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(b,)| b as u64))
    }

    pub async fn set_cursor(&self, chain_id: u64, last_block: u64) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO indexer_cursors (chain_id, last_block, updated_at) VALUES ($1,$2,now()) \
             ON CONFLICT (chain_id) DO UPDATE SET last_block = EXCLUDED.last_block, updated_at = now()",
        )
        .bind(chain_id as i64)
        .bind(last_block as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
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
