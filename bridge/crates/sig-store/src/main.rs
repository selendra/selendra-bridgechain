//! sig-store — the shared signature store as an HTTP service (Phase 7).
//!
//! Replaces the local file-per-id directory the validator/keeper shared in
//! Phases 4–6. N independent validators POST their signatures here; the keeper
//! GETs the merged records. Same `SubmissionRecord` shape as `bridge-core::store`,
//! which also backs this service on disk (so it survives a restart).
//!
//!   GET  /health
//!   POST /submissions          -> upsert a record + its signature(s); dedupe by signer
//!   GET  /submissions          -> all records
//!   GET  /submissions/:id      -> one record (404 if unknown)
//!
//! Writes are serialized by a mutex (each upsert is read-modify-write of a file).

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use bridge_core::store::{self, SubmissionRecord};
use clap::Parser;
use tokio::sync::Mutex;
use tracing::info;

#[derive(Parser, Debug)]
#[command(about = "HTTP signature store for the bridge")]
struct Args {
    /// Address to bind, e.g. 0.0.0.0:8080
    #[arg(long, env = "SIG_STORE_BIND", default_value = "0.0.0.0:8080")]
    bind: String,
    /// Directory backing the store on disk
    #[arg(long, env = "SIG_STORE_DIR", default_value = "sig-store-data")]
    dir: PathBuf,
}

#[derive(Clone)]
struct AppState {
    dir: PathBuf,
    write_lock: Arc<Mutex<()>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sig_store=info".into()),
        )
        .init();

    let args = Args::parse();
    store::ensure_dir(&args.dir)?;

    let state = AppState { dir: args.dir.clone(), write_lock: Arc::new(Mutex::new(())) };

    let app = Router::new()
        .route("/health", get(health))
        .route("/submissions", post(post_submission).get(list_submissions))
        .route("/submissions/:id", get(get_submission))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    info!(bind = %args.bind, dir = %args.dir.display(), "sig-store listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

/// Upsert a record. The body carries the submission params plus one (or more)
/// signatures in `signatures`; each is merged into the stored record, deduped
/// by signer. Returns the merged record.
async fn post_submission(
    State(s): State<AppState>,
    Json(record): Json<SubmissionRecord>,
) -> Result<Json<SubmissionRecord>, (StatusCode, String)> {
    let _guard = s.write_lock.lock().await;

    let sigs = record.signatures.clone();
    let mut base = record;
    base.signatures = Vec::new();

    if sigs.is_empty() {
        // No signature attached: just ensure the record exists.
        let existing = store::load(&s.dir, &base.submission_id).map_err(internal)?;
        let merged = existing.unwrap_or(base);
        return Ok(Json(merged));
    }

    let mut merged = base;
    for sig in sigs {
        let signer = sig.signer.clone();
        merged = store::upsert_signature(&s.dir, merged, sig).map_err(internal)?;
        info!(submission_id = %merged.submission_id, %signer, sigs = merged.signatures.len(), "stored signature");
    }
    Ok(Json(merged))
}

async fn list_submissions(
    State(s): State<AppState>,
) -> Result<Json<Vec<SubmissionRecord>>, (StatusCode, String)> {
    let all = store::load_all(&s.dir).map_err(internal)?;
    Ok(Json(all))
}

async fn get_submission(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SubmissionRecord>, (StatusCode, String)> {
    match store::load(&s.dir, &id).map_err(internal)? {
        Some(rec) => Ok(Json(rec)),
        None => Err((StatusCode::NOT_FOUND, "unknown submissionId".into())),
    }
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
