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

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bridge_core::allow::{
    AddTokenRequest, AllowedChain, AllowedToken, AttestationRequest, ClaimedRequest,
    SubmissionHistory,
};
use bridge_core::auth::{require_scope, Auth, Scope};
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
        .route("/allowed/tokens", get(list_tokens))
        .route("/allowed/chains", get(list_chains))
        .route_layer(middleware::from_fn_with_state((auth.clone(), Scope::Read), require_scope));

    // Validators deposit signatures and cancel/refund attestations.
    let sign = Router::new()
        .route("/submissions", post(post_submission))
        .route("/submissions/:id/attestations", post(post_attestation))
        .route_layer(middleware::from_fn_with_state((auth.clone(), Scope::Sign), require_scope));

    // The keeper records a claim tx. Note this is NOT authoritative for the
    // lifecycle the indexer owns — it only annotates the row.
    let relay = Router::new()
        .route("/submissions/:id/claimed", post(post_claimed))
        .route_layer(middleware::from_fn_with_state((auth.clone(), Scope::Relay), require_scope));

    // The allowlists are a security control, so they get their own scope.
    let admin = Router::new()
        .route("/allowed/tokens", post(add_token))
        .route("/allowed/tokens/:chain/:token", delete(remove_token))
        .route("/allowed/chains", post(add_chain))
        .route("/allowed/chains/:from/:to", delete(remove_chain))
        .route_layer(middleware::from_fn_with_state((auth.clone(), Scope::Admin), require_scope));

    if auth.is_enforced() {
        info!(tokens = auth.token_count(), "auth enabled: scoped bearer tokens required");
    } else {
        warn!(
            "no tokens configured — the API is UNAUTHENTICATED (signatures, claim \
             status and the allowlist are all world-writable). Set at least \
             SIG_STORE_VALIDATOR_TOKEN / _KEEPER_TOKEN / _READER_TOKEN / _ADMIN_TOKEN \
             for any networked deployment."
        );
    }

    let app = Router::new()
        .route("/health", get(health))
        .merge(read)
        .merge(sign)
        .merge(relay)
        .merge(admin)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    info!(bind = %args.bind, "sig-store listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
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

async fn list_submissions(
    State(s): State<AppState>,
) -> Result<Json<Vec<SubmissionRecord>>, (StatusCode, String)> {
    Ok(Json(s.db.load_all().await.map_err(db_err)?))
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
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(t) = bearer {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        app().oneshot(b.body(Body::empty()).unwrap()).await.unwrap().status()
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

    // `ct_eq` and the scope table itself are covered by bridge_core::auth's tests.
}
