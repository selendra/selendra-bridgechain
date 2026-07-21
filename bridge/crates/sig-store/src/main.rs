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
use bridge_core::allow::{AddTokenRequest, AllowedChain, AllowedToken, ClaimedRequest, SubmissionHistory};
use bridge_core::auth::require_auth;
use bridge_core::store::SubmissionRecord;
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
    /// Shared secret required as `Authorization: Bearer <token>` on every route
    /// except `/health`. When unset the API is UNAUTHENTICATED (dev only) — the
    /// allowlist (a security control) and history become world-writable, so set
    /// this in any networked deployment.
    #[arg(long, env = "SIG_STORE_TOKEN")]
    auth_token: Option<String>,
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
    let db = Db::connect(&args.database_url).await?;
    info!("connected to Postgres and applied schema");

    let state = AppState { db };

    // Everything but /health sits behind the bearer-token gate (when configured).
    let mut protected = Router::new()
        .route("/submissions", post(post_submission).get(list_submissions))
        .route("/submissions/:id", get(get_submission))
        .route("/submissions/:id/claimed", post(post_claimed))
        .route("/history", get(get_history))
        .route("/allowed/tokens", get(list_tokens).post(add_token))
        .route("/allowed/tokens/:chain/:token", delete(remove_token))
        .route("/allowed/chains", get(list_chains).post(add_chain))
        .route("/allowed/chains/:from/:to", delete(remove_chain));

    match args.auth_token.filter(|t| !t.is_empty()) {
        Some(token) => {
            info!("auth enabled: bearer token required on all routes except /health");
            protected = protected.route_layer(middleware::from_fn_with_state(
                Arc::new(token),
                require_auth,
            ));
        }
        None => warn!(
            "SIG_STORE_TOKEN is unset — API is UNAUTHENTICATED (the allowlist and \
             history are world-writable). Set a token for any networked deployment."
        ),
    }

    let app = Router::new()
        .route("/health", get(health))
        .merge(protected)
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
    use tower::ServiceExt; // for `oneshot`

    // The same wiring main() uses: /health open, everything else behind the gate.
    fn app(token: &str) -> Router {
        let protected = Router::new()
            .route("/allowed/tokens", get(|| async { "tokens" }))
            .route_layer(middleware::from_fn_with_state(
                Arc::new(token.to_string()),
                require_auth,
            ));
        Router::new().route("/health", get(health)).merge(protected)
    }

    async fn status_of(req: Request<Body>) -> StatusCode {
        app("s3cret").oneshot(req).await.unwrap().status()
    }

    fn req(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }
    fn req_auth(uri: &str, bearer: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn health_needs_no_token() {
        assert_eq!(status_of(req("/health")).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn protected_route_rejects_missing_token() {
        assert_eq!(status_of(req("/allowed/tokens")).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn protected_route_rejects_wrong_token() {
        assert_eq!(
            status_of(req_auth("/allowed/tokens", "nope")).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn protected_route_accepts_correct_token() {
        assert_eq!(
            status_of(req_auth("/allowed/tokens", "s3cret")).await,
            StatusCode::OK
        );
    }

    // `ct_eq` itself is covered by bridge_core::auth's own tests.
}
