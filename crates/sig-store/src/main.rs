//! sig-store — the bridge's HTTP gateway over the Postgres source of truth.
//!
//! Backed by `bridge-db` (Postgres): it owns transaction history (submissions +
//! signatures + lifecycle status) and the two allowlists. N validators POST
//! their signatures here; the keeper GETs merged records, then POSTs back the
//! claim tx; operators manage the allowlists. The DB enforces the same
//! trust boundary the old file store did (id<->params binding + signature auth).
//!
//!   GET    /health
//!
//!   # signature store / transaction history
//!   POST   /submissions                  -> upsert a record + its signature(s)
//!   GET    /submissions                  -> all records (params + signatures)
//!   GET    /submissions/:id              -> one record (404 if unknown)
//!   POST   /submissions/:id/claimed      -> mark claimed (body: {"claim_tx": "0x.."})
//!   GET    /history                      -> history view (status, counts, timestamps)
//!   GET    /swaps?chain_id=&limit=       -> same-chain swap history (newest first)
//!
//!   # refund path (two-phase: burn on the destination, then repay on the source)
//!   POST   /submissions/:id/attestations -> a validator's cancel/refund signature
//!                                           (body: {"kind":"cancel"|"refund",
//!                                                   "signer":"0x..","signature":"0x.."})
//!   GET    /refund-candidates            -> submissions a refund relayer should
//!                                           examine (still requires on-chain checks)
//!
//! The `cancelled`/`refunded` lifecycle has NO write route: those states gate the
//! refund-candidate list, so they are set only by the indexer from observed
//! on-chain `Cancelled`/`Refunded` events, never on a caller's word (a forged
//! "refunded" would hide a stuck transfer from the relayers).
//!
//!   # allowlists
//!   GET    /allowed/tokens               -> whitelisted tokens
//!   POST   /allowed/tokens               -> add (body: {"chain_id":..,"token":"0x..","symbol":".."})
//!   DELETE /allowed/tokens/:chain/:token -> remove
//!   GET    /allowed/chains               -> whitelisted source->target pairs
//!   POST   /allowed/chains               -> add (body: {"chain_id_from":..,"chain_id_to":..})
//!   DELETE /allowed/chains/:from/:to     -> remove

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bridge_core::allow::{
    AddTokenRequest, AllowedChain, AllowedToken, AttestationRequest, ClaimedRequest,
    SubmissionHistory, SwapRecord,
};
use bridge_core::auth::{require_scope, Auth, Scope};
use bridge_core::ratelimit::{enforce as rate_limit, RateLimit};
use bridge_core::store::{SigKind, SignerSig, SubmissionRecord};
use bridge_db::{Db, DbError};
use clap::Parser;
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(about = "Postgres-backed signature store + allowlists for the bridge")]
struct Args {
    /// Address to bind, e.g. 0.0.0.0:8080
    #[arg(long, env = "SIG_STORE_BIND", default_value = "0.0.0.0:8080")]
    bind: String,
    /// Postgres connection string, e.g. postgres://bridge:bridge@localhost:5432/bridge
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,
    /// LEGACY all-scopes secret. Still honoured so existing deployments keep
    /// working, but it grants read + sign + relay + admin to whoever holds it —
    /// the blast radius finding L-5 is about. Prefer the per-service tokens below.
    #[arg(long, env = "SIG_STORE_TOKEN")]
    auth_token: Option<String>,
    /// Validators: read + deposit signatures/attestations. Cannot mark claimed or
    /// edit the allowlist.
    #[arg(long, env = "SIG_STORE_VALIDATOR_TOKEN")]
    validator_token: Option<String>,
    /// Keeper: read + record a claim tx. Cannot deposit signatures.
    #[arg(long, env = "SIG_STORE_KEEPER_TOKEN")]
    keeper_token: Option<String>,
    /// Read-only consumers (the GraphQL API). Grants nothing that writes — this is
    /// the whole point of the split, since it is the most exposed component.
    #[arg(long, env = "SIG_STORE_READER_TOKEN")]
    reader_token: Option<String>,
    /// Operators: allowlist mutations, itself a security control.
    #[arg(long, env = "SIG_STORE_ADMIN_TOKEN")]
    admin_token: Option<String>,
    /// Sustained write requests per second, per bearer token (L-1).
    #[arg(long, env = "SIG_STORE_RATE_PER_SECOND", default_value_t = 50.0)]
    rate_per_second: f64,
    /// How many write requests one credential may send back to back (L-1).
    #[arg(long, env = "SIG_STORE_RATE_BURST", default_value_t = 200)]
    rate_burst: u32,
    /// Largest accepted request body, in bytes.
    #[arg(long, env = "SIG_STORE_MAX_BODY_BYTES", default_value_t = 256 * 1024)]
    max_body_bytes: usize,
    /// Serve with NO authentication when no token is configured. Dev only.
    ///
    /// Without this the process refuses to bind rather than exposing an open
    /// store, because "no token" is far more often a lost secret mount than a
    /// deliberate choice — and the open failure mode is world-writable
    /// signatures, claim status, and the allowlist that IS the incident
    /// kill-switch. Requiring the operator to say it out loud makes the
    /// dangerous configuration the one that takes an extra argument.
    #[arg(long, env = "SIG_STORE_ALLOW_UNAUTHENTICATED", default_value_t = false)]
    allow_unauthenticated: bool,
}

impl Args {
    /// Assemble the scoped token set. Absent/empty tokens are dropped by
    /// [`Auth::new`], so an unset variable can never authenticate a request.
    fn auth(&self) -> Auth {
        let mut entries: Vec<(String, std::collections::HashSet<Scope>)> = Vec::new();
        if let Some(t) = self.auth_token.clone().filter(|t| !t.is_empty()) {
            warn!(
                "SIG_STORE_TOKEN grants ALL scopes to every holder (read+sign+relay+admin). \
                 Prefer SIG_STORE_{{VALIDATOR,KEEPER,READER,ADMIN}}_TOKEN so a leak from one \
                 component cannot write on behalf of the others."
            );
            entries.push((t, Scope::all()));
        }
        if let Some(t) = self.validator_token.clone() {
            entries.push((t, [Scope::Read, Scope::Sign].into_iter().collect()));
        }
        if let Some(t) = self.keeper_token.clone() {
            entries.push((t, [Scope::Read, Scope::Relay].into_iter().collect()));
        }
        if let Some(t) = self.reader_token.clone() {
            entries.push((t, [Scope::Read].into_iter().collect()));
        }
        if let Some(t) = self.admin_token.clone() {
            entries.push((t, [Scope::Read, Scope::Admin].into_iter().collect()));
        }
        Auth::new(entries)
    }
}

#[derive(Clone)]
struct AppState {
    db: Db,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sig_store=info,bridge_db=info".into()),
        )
        .init();

    let args = Args::parse();
    let auth = args.auth();
    let db = Db::connect(&args.database_url).await?;
    info!("connected to Postgres and applied schema");

    let state = AppState { db };

    // L-5: each route group demands the NARROWEST scope that lets it work, so a
    // credential leaked from one component cannot act as another.
    //
    // NOTE: there is deliberately no write route for the `cancelled`/`refunded`
    // lifecycle at ANY scope. Those states gate the refund-candidate list, so a
    // forged "refunded" would permanently hide a genuinely stuck transfer from the
    // relayers. They are written ONLY by the indexer, from observed on-chain
    // `Cancelled`/`Refunded` events — never on a caller's word.
    let read = Router::new()
        .route("/submissions", get(list_submissions))
        .route("/submissions/:id", get(get_submission))
        .route("/refund-candidates", get(get_refund_candidates))
        .route("/history", get(get_history))
        .route("/swaps", get(get_swaps))
        .route("/allowed/tokens", get(list_tokens))
        .route("/allowed/chains", get(list_chains))
        .route_layer(middleware::from_fn_with_state((auth.clone(), Scope::Read), require_scope));

    // L-1: a per-credential token bucket on every route that WRITES.
    //
    // The binding rules make a forged record impossible, but they do not require a
    // record to describe a transfer that ever happened on a chain — so a holder of
    // a `Sign`-scoped token could mint well-formed junk at line rate and grow the
    // table without limit. Keyed on the bearer token, because the thing worth
    // bounding is what ONE credential can do; every writer here is a service
    // holding a scoped token, and several of them may share an ingress address.
    let writes = RateLimit::new(args.rate_burst, args.rate_per_second);
    info!(
        burst = args.rate_burst,
        per_second = args.rate_per_second,
        max_body_bytes = args.max_body_bytes,
        "write rate limit active (per bearer token)"
    );

    // Validators deposit signatures and cancel/refund attestations.
    let sign = Router::new()
        .route("/submissions", post(post_submission))
        .route("/submissions/:id/attestations", post(post_attestation))
        .route_layer(middleware::from_fn_with_state(writes.clone(), rate_limit))
        .route_layer(middleware::from_fn_with_state((auth.clone(), Scope::Sign), require_scope));

    // The keeper records a claim tx. Note this is NOT authoritative for the
    // lifecycle the indexer owns — it only annotates the row.
    let relay = Router::new()
        .route("/submissions/:id/claimed", post(post_claimed))
        .route_layer(middleware::from_fn_with_state(writes.clone(), rate_limit))
        .route_layer(middleware::from_fn_with_state((auth.clone(), Scope::Relay), require_scope));

    // The allowlists are a security control, so they get their own scope.
    let admin = Router::new()
        .route("/allowed/tokens", post(add_token))
        .route("/allowed/tokens/:chain/:token", delete(remove_token))
        .route("/allowed/chains", post(add_chain))
        .route("/allowed/chains/:from/:to", delete(remove_chain))
        .route_layer(middleware::from_fn_with_state(writes.clone(), rate_limit))
        .route_layer(middleware::from_fn_with_state((auth.clone(), Scope::Admin), require_scope));

    // FAIL CLOSED. `Auth::new` drops empty tokens, so an unset (or wiped) secret
    // leaves nothing configured — and an unconfigured `Auth` grants every scope to
    // everyone. Warning about that and serving anyway meant one lost env var
    // silently opened the whole store; the log line was the only difference
    // between a correct deployment and an open one.
    require_credentials(&auth, args.allow_unauthenticated)?;

    let app = Router::new()
        .route("/health", get(health))
        .merge(read)
        .merge(sign)
        .merge(relay)
        .merge(admin)
        // A submission with its signatures is a few kB; the default 2 MB let a
        // caller make the server allocate far more than any real request needs.
        .layer(DefaultBodyLimit::max(args.max_body_bytes))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    info!(bind = %args.bind, "sig-store listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

/// FAIL CLOSED: refuse to serve an unauthenticated store unless told to.
///
/// [`Auth::new`] drops empty tokens, so an unset — or wiped — secret leaves
/// nothing configured, and an unconfigured `Auth` grants EVERY scope to everyone.
/// This used to warn and serve anyway, which made one lost env var
/// indistinguishable from a correct deployment except in the log, with
/// world-writable signatures, claim status and the allowlist (the incident
/// kill-switch) as the result.
///
/// Compose passes the tokens as `${VAR:?}` so it could never reach this state; a
/// systemd unit, a bare binary or a Kubernetes manifest that loses a secret mount
/// absolutely could.
fn require_credentials(auth: &Auth, allow_unauthenticated: bool) -> anyhow::Result<()> {
    if auth.is_enforced() {
        info!(tokens = auth.token_count(), "auth enabled: scoped bearer tokens required");
        return Ok(());
    }
    if allow_unauthenticated {
        warn!(
            "--allow-unauthenticated: serving with NO authentication (signatures, claim \
             status and the allowlist are all world-writable). Never do this on a \
             networked deployment."
        );
        return Ok(());
    }
    anyhow::bail!(
        "refusing to start: no bearer token is configured, which would leave signatures, \
         claim status and the allowlist world-writable. Set at least one of \
         SIG_STORE_VALIDATOR_TOKEN / _KEEPER_TOKEN / _READER_TOKEN / _ADMIN_TOKEN (or the \
         legacy SIG_STORE_TOKEN), or pass --allow-unauthenticated to accept an open store \
         on a trusted local network."
    )
}

/// Map a DbError to an HTTP error, distinguishing caller faults (4xx) from
/// server faults (5xx) so a forged signature reads as 400, not 500.
fn db_err(e: DbError) -> (StatusCode, String) {
    let code = if e.is_client_error() {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (code, e.to_string())
}

// --- signature store / transaction history -------------------------------

/// Upsert a record. The body carries the submission params plus one (or more)
/// signatures in `signatures`; each is merged into the stored record, deduped
/// by signer. Returns the merged record.
async fn post_submission(
    State(s): State<AppState>,
    Json(record): Json<SubmissionRecord>,
) -> Result<Json<SubmissionRecord>, (StatusCode, String)> {
    let sigs = record.signatures.clone();
    let mut base = record;
    base.signatures = Vec::new();

    if sigs.is_empty() {
        // No signature attached: just report the current state (if any).
        let existing = s.db.load(&base.submission_id).await.map_err(db_err)?;
        return Ok(Json(existing.unwrap_or(base)));
    }

    let mut merged = base;
    for sig in sigs {
        let signer = sig.signer.clone();
        merged = s.db.upsert_signature(merged, sig).await.map_err(db_err)?;
        info!(submission_id = %merged.submission_id, %signer, sigs = merged.signatures.len(), "stored signature");
    }
    Ok(Json(merged))
}

/// Query for [`list_submissions`].
///
/// With no parameters this returns the whole table, which is what the GraphQL API
/// and the operator tooling want. `pending` narrows it to a keeper's work queue,
/// filtered in SQL on the lifecycle the indexer maintains — see
/// `Db::pending_claims` for why polling the whole table every tick did not scale.
#[derive(serde::Deserialize)]
struct SubmissionQuery {
    /// `"claims"` (needs `chain_id_to`) or `"refunds"` (needs `chain_id_from`).
    pending: Option<String>,
    chain_id_to: Option<u64>,
    chain_id_from: Option<u64>,
}

async fn list_submissions(
    State(s): State<AppState>,
    Query(q): Query<SubmissionQuery>,
) -> Result<Json<Vec<SubmissionRecord>>, (StatusCode, String)> {
    let records = match (q.pending.as_deref(), q.chain_id_to, q.chain_id_from) {
        (None, _, _) => s.db.load_all().await,
        (Some("claims"), Some(to), _) => s.db.pending_claims(to).await,
        (Some("refunds"), _, Some(from)) => s.db.pending_refunds(from).await,
        (Some(kind), _, _) => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "pending={kind:?} needs its chain filter: \
                     pending=claims&chain_id_to=N, or pending=refunds&chain_id_from=N"
                ),
            ))
        }
    };
    Ok(Json(records.map_err(db_err)?))
}

async fn get_submission(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SubmissionRecord>, (StatusCode, String)> {
    match s.db.load(&id).await.map_err(db_err)? {
        Some(rec) => Ok(Json(rec)),
        None => Err((StatusCode::NOT_FOUND, "unknown submissionId".into())),
    }
}

async fn post_claimed(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ClaimedRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    s.db.mark_claimed(&id, &body.claim_tx).await.map_err(db_err)?;
    info!(submission_id = %id, claim_tx = %body.claim_tx, "marked claimed");
    Ok(StatusCode::NO_CONTENT)
}

async fn get_history(
    State(s): State<AppState>,
) -> Result<Json<Vec<SubmissionHistory>>, (StatusCode, String)> {
    Ok(Json(s.db.history().await.map_err(db_err)?))
}

/// Query for [`get_swaps`]. Both fields optional: no `chain_id` means every
/// chain, and `limit` defaults to 100 and is capped so one request cannot ask
/// the database for the whole table.
#[derive(serde::Deserialize)]
struct SwapQuery {
    chain_id: Option<u64>,
    limit: Option<u64>,
}

/// Same-chain swap history. Read scope, like `/history` — it exists so the
/// GraphQL API can serve `swapHistory` with its read-only bearer token instead
/// of a Postgres credential of its own.
async fn get_swaps(
    State(s): State<AppState>,
    Query(q): Query<SwapQuery>,
) -> Result<Json<Vec<SwapRecord>>, (StatusCode, String)> {
    let limit = q.limit.unwrap_or(100).min(1000) as i64;
    Ok(Json(s.db.list_swaps(q.chain_id, limit).await.map_err(db_err)?))
}

// --- refund path ----------------------------------------------------------

/// Store one validator's cancel/refund attestation.
///
/// The signature is checked against the digest for `kind` specifically, so a
/// transfer signature posted here as a `cancel` recovers to the wrong address
/// and is rejected — the three quorums stay independent.
async fn post_attestation(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AttestationRequest>,
) -> Result<Json<SubmissionRecord>, (StatusCode, String)> {
    let kind = SigKind::parse(&req.kind)
        .filter(|k| *k != SigKind::Transfer)
        .ok_or((StatusCode::BAD_REQUEST, format!("unknown attestation kind {:?}", req.kind)))?;

    let rec = s
        .db
        .upsert_attestation(&id, kind, SignerSig { signer: req.signer, signature: req.signature })
        .await
        .map_err(db_err)?;

    let count = match kind {
        SigKind::Cancel => rec.cancel_signatures.len(),
        SigKind::Refund => rec.refund_signatures.len(),
        SigKind::Transfer => unreachable!("filtered above"),
    };
    info!(submission_id = %rec.submission_id, kind = kind.as_str(), count, "stored attestation");
    Ok(Json(rec))
}

async fn get_refund_candidates(
    State(s): State<AppState>,
) -> Result<Json<Vec<SubmissionRecord>>, (StatusCode, String)> {
    Ok(Json(s.db.refund_candidates().await.map_err(db_err)?))
}

// `cancelled`/`refunded` are written only by the indexer from observed on-chain
// events (see the router note), so there are intentionally no HTTP handlers for
// them here.

// --- allowlists -----------------------------------------------------------

async fn list_tokens(
    State(s): State<AppState>,
) -> Result<Json<Vec<AllowedToken>>, (StatusCode, String)> {
    Ok(Json(s.db.list_allowed_tokens().await.map_err(db_err)?))
}

async fn add_token(
    State(s): State<AppState>,
    Json(req): Json<AddTokenRequest>,
) -> Result<Json<AllowedToken>, (StatusCode, String)> {
    let added = s
        .db
        .add_allowed_token(req.chain_id, &req.token, req.symbol.as_deref())
        .await
        .map_err(db_err)?;
    info!(chain_id = added.chain_id, token = %added.token, debridge_id = %added.debridge_id, "allowed token");
    Ok(Json(added))
}

async fn remove_token(
    State(s): State<AppState>,
    Path((chain, token)): Path<(u64, String)>,
) -> Result<StatusCode, (StatusCode, String)> {
    let removed = s.db.remove_allowed_token(chain, &token).await.map_err(db_err)?;
    Ok(if removed { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND })
}

async fn list_chains(
    State(s): State<AppState>,
) -> Result<Json<Vec<AllowedChain>>, (StatusCode, String)> {
    Ok(Json(s.db.list_allowed_chains().await.map_err(db_err)?))
}

async fn add_chain(
    State(s): State<AppState>,
    Json(req): Json<AllowedChain>,
) -> Result<Json<AllowedChain>, (StatusCode, String)> {
    let added = s
        .db
        .add_allowed_chain(req.chain_id_from, req.chain_id_to)
        .await
        .map_err(db_err)?;
    info!(from = added.chain_id_from, to = added.chain_id_to, "allowed chain pair");
    Ok(Json(added))
}

async fn remove_chain(
    State(s): State<AppState>,
    Path((from, to)): Path<(u64, u64)>,
) -> Result<StatusCode, (StatusCode, String)> {
    let removed = s.db.remove_allowed_chain(from, to).await.map_err(db_err)?;
    Ok(if removed { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request};
    use axum::routing::post;
    use tower::ServiceExt; // for `oneshot`

    const VAL: &str = "val-token";
    const KEEP: &str = "keeper-token";
    const READ: &str = "reader-token";
    const ADMIN: &str = "admin-token";

    fn test_auth() -> Auth {
        Auth::new([
            (VAL.to_string(), [Scope::Read, Scope::Sign].into_iter().collect()),
            (KEEP.to_string(), [Scope::Read, Scope::Relay].into_iter().collect()),
            (READ.to_string(), [Scope::Read].into_iter().collect()),
            (ADMIN.to_string(), [Scope::Read, Scope::Admin].into_iter().collect()),
        ])
    }

    /// The same scope layering main() uses, with stub handlers so the test
    /// exercises the AUTH wiring rather than the database.
    fn app() -> Router {
        app_limited(RateLimit::new(1_000, 1_000.0))
    }

    fn app_limited(writes: RateLimit) -> Router {
        let auth = test_auth();
        let read = Router::new()
            .route("/submissions", get(|| async { "list" }))
            .route("/allowed/tokens", get(|| async { "tokens" }))
            .route_layer(middleware::from_fn_with_state(
                (auth.clone(), Scope::Read),
                require_scope,
            ));
        let sign = Router::new()
            .route("/submissions", post(|| async { "signed" }))
            .route_layer(middleware::from_fn_with_state(writes.clone(), rate_limit))
            .route_layer(middleware::from_fn_with_state(
                (auth.clone(), Scope::Sign),
                require_scope,
            ));
        let relay = Router::new()
            .route("/submissions/:id/claimed", post(|| async { "claimed" }))
            .route_layer(middleware::from_fn_with_state(
                (auth.clone(), Scope::Relay),
                require_scope,
            ));
        let admin = Router::new()
            .route("/allowed/tokens", post(|| async { "added" }))
            .route_layer(middleware::from_fn_with_state(
                (auth.clone(), Scope::Admin),
                require_scope,
            ));
        Router::new()
            .route("/health", get(health))
            .merge(read)
            .merge(sign)
            .merge(relay)
            .merge(admin)
    }

    async fn status(method: &str, uri: &str, bearer: Option<&str>) -> StatusCode {
        status_on(app(), method, uri, bearer).await
    }

    async fn status_on(app: Router, method: &str, uri: &str, bearer: Option<&str>) -> StatusCode {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(t) = bearer {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        app.oneshot(b.body(Body::empty()).unwrap()).await.unwrap().status()
    }

    // --- L-1: a credential cannot write at line rate --------------------------

    /// The binding rules stop a FORGED record, not a well-formed useless one: an
    /// id must hash its own params and a signature must recover to its signer, but
    /// nothing requires the transfer to have happened on any chain. Without a rate
    /// limit a `Sign`-scoped token could grow the table without bound.
    #[tokio::test]
    async fn writes_are_rate_limited_per_credential() {
        let limit = RateLimit::new(2, 0.001); // two, then effectively none
        let app = app_limited(limit);

        assert_eq!(status_on(app.clone(), "POST", "/submissions", Some(VAL)).await, StatusCode::OK);
        assert_eq!(status_on(app.clone(), "POST", "/submissions", Some(VAL)).await, StatusCode::OK);
        assert_eq!(
            status_on(app.clone(), "POST", "/submissions", Some(VAL)).await,
            StatusCode::TOO_MANY_REQUESTS,
            "a credential over its budget must be refused"
        );
    }

    /// Reads are not limited — the GraphQL API polls them and a read cannot grow
    /// the table.
    #[tokio::test]
    async fn reads_are_not_rate_limited() {
        let app = app_limited(RateLimit::new(1, 0.001));
        for _ in 0..5 {
            assert_eq!(status_on(app.clone(), "GET", "/submissions", Some(READ)).await, StatusCode::OK);
        }
    }

    /// The limiter sits INSIDE the auth layer, so an unauthenticated flood is
    /// rejected as 401 without consuming a legitimate credential's budget.
    #[tokio::test]
    async fn an_unauthenticated_flood_does_not_consume_a_real_budget() {
        let app = app_limited(RateLimit::new(2, 0.001));
        for _ in 0..10 {
            assert_eq!(
                status_on(app.clone(), "POST", "/submissions", Some("bogus")).await,
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            status_on(app.clone(), "POST", "/submissions", Some(VAL)).await,
            StatusCode::OK,
            "the real credential must still have its full budget"
        );
    }

    #[tokio::test]
    async fn health_needs_no_token() {
        assert_eq!(status("GET", "/health", None).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn missing_or_wrong_token_is_rejected() {
        assert_eq!(status("GET", "/submissions", None).await, StatusCode::UNAUTHORIZED);
        assert_eq!(status("GET", "/submissions", Some("nope")).await, StatusCode::UNAUTHORIZED);
    }

    /// THE L-5 property, end to end through the router: the read-only credential
    /// the GraphQL API carries — the most exposed component — must be unable to
    /// write anything. Under the old single shared token it could do everything.
    #[tokio::test]
    async fn the_read_only_token_cannot_write_anything() {
        assert_eq!(status("GET", "/submissions", Some(READ)).await, StatusCode::OK);

        assert_eq!(
            status("POST", "/submissions", Some(READ)).await,
            StatusCode::UNAUTHORIZED,
            "a reader must not deposit signatures"
        );
        assert_eq!(
            status("POST", "/submissions/0xabc/claimed", Some(READ)).await,
            StatusCode::UNAUTHORIZED,
            "a reader must not mark transfers claimed"
        );
        assert_eq!(
            status("POST", "/allowed/tokens", Some(READ)).await,
            StatusCode::UNAUTHORIZED,
            "a reader must not edit the allowlist"
        );
    }

    /// Components cannot act as one another.
    #[tokio::test]
    async fn scopes_are_not_interchangeable_over_http() {
        // A validator signs, but does not relay or administer.
        assert_eq!(status("POST", "/submissions", Some(VAL)).await, StatusCode::OK);
        assert_eq!(
            status("POST", "/submissions/0xabc/claimed", Some(VAL)).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(status("POST", "/allowed/tokens", Some(VAL)).await, StatusCode::UNAUTHORIZED);

        // The keeper relays, but cannot deposit signatures.
        assert_eq!(status("POST", "/submissions/0xabc/claimed", Some(KEEP)).await, StatusCode::OK);
        assert_eq!(status("POST", "/submissions", Some(KEEP)).await, StatusCode::UNAUTHORIZED);

        // The operator administers, but does not sign.
        assert_eq!(status("POST", "/allowed/tokens", Some(ADMIN)).await, StatusCode::OK);
        assert_eq!(status("POST", "/submissions", Some(ADMIN)).await, StatusCode::UNAUTHORIZED);
    }

    /// Every component still reads — the shared capability.
    #[tokio::test]
    async fn every_service_token_can_read() {
        for t in [VAL, KEEP, READ, ADMIN] {
            assert_eq!(status("GET", "/submissions", Some(t)).await, StatusCode::OK, "{t}");
        }
    }

    // --- M-1: the store must not come up open by accident ------------------

    /// THE regression. An unset (or wiped) token variable leaves `Auth`
    /// unenforced, and an unenforced `Auth` grants every scope to everyone. The
    /// process used to log a warning and serve, so a lost secret mount looked
    /// exactly like a healthy deployment.
    #[test]
    fn refuses_to_start_with_no_credentials() {
        let err = require_credentials(&Auth::new([]), false).unwrap_err().to_string();
        assert!(err.contains("refusing to start"), "{err}");
    }

    /// An unset variable must not become a usable empty credential either — that
    /// is the same open store by a different route.
    #[test]
    fn an_empty_token_is_not_a_credential() {
        let empty = Auth::new([(String::new(), Scope::all())]);
        assert!(require_credentials(&empty, false).is_err());
    }

    /// The dangerous configuration is the one that takes an extra argument.
    #[test]
    fn an_open_store_requires_saying_so_explicitly() {
        assert!(require_credentials(&Auth::new([]), true).is_ok());
    }

    #[test]
    fn a_configured_token_starts_normally() {
        assert!(require_credentials(&test_auth(), false).is_ok());
    }

    // `ct_eq` and the scope table itself are covered by bridge_core::auth's tests.
}
