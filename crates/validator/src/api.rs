//! Operator HTTP API (mirrors this repo's `AppController`).
//!
//! Single source (back-compat, what phase6.sh drives):
//!   GET  /status              -> scanner cursor, pause state, nonce map
//!   POST /pause               -> pause scanning
//!   POST /resume              -> clear pause
//!   POST /rescan {from_block} -> reset cursor and re-scan from a block
//!
//! Multiple sources: the bare routes still work when exactly one source is
//! configured; otherwise address a specific chain:
//!   GET  /status              -> array of every source's status
//!   GET  /status/{chain_id}
//!   POST /pause/{chain_id}  /resume/{chain_id}  /rescan/{chain_id}
//!
//! Each source's state is shared with its scan loop via `Arc<Mutex<Runtime>>`.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{get, post};
use axum::{Json, Router};
use bridge_core::auth::require_auth;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::state::Runtime;

#[derive(Clone)]
pub struct ApiState {
    /// One runtime per watched source chain, keyed by chain_id.
    pub sources: Arc<BTreeMap<u64, Arc<Mutex<Runtime>>>>,
    pub validator: String,
    /// Shared secret required as `Authorization: Bearer <token>` on the state-
    /// changing routes (pause/resume/rescan). `None` => unauthenticated (dev).
    pub token: Option<String>,
}

#[derive(Serialize)]
struct StatusResponse {
    validator: String,
    chain_id: u64,
    paused: bool,
    pause_reason: Option<String>,
    last_block: u64,
    next_block: u64,
    /// Last accepted nonce per corridor: `{ "<chain_from>": { "<chain_to>": n } }`.
    nonces: BTreeMap<u64, BTreeMap<u64, u64>>,
}

#[derive(Deserialize)]
struct RescanRequest {
    from_block: u64,
}

pub fn router(state: ApiState) -> Router {
    // Read-only status stays open for monitoring; the state-changing routes
    // (which can halt the validator — a DoS vector) require the bearer token.
    let status = Router::new()
        .route("/status", get(status_all))
        .route("/status/:chain_id", get(status_one));

    let mut control = Router::new()
        .route("/pause", post(pause_bare))
        .route("/pause/:chain_id", post(pause_one))
        .route("/resume", post(resume_bare))
        .route("/resume/:chain_id", post(resume_one))
        .route("/rescan", post(rescan_bare))
        .route("/rescan/:chain_id", post(rescan_one));

    match state.token.clone().filter(|t| !t.is_empty()) {
        Some(token) => {
            info!("operator API auth enabled: bearer token required for pause/resume/rescan");
            control = control
                .route_layer(middleware::from_fn_with_state(Arc::new(token), require_auth));
        }
        None => warn!(
            "VALIDATOR_API_TOKEN is unset — operator API pause/resume/rescan are \
             UNAUTHENTICATED (anyone who can reach it can halt this validator)."
        ),
    }

    status.merge(control).with_state(state)
}

/// Spawn the operator API on `bind`. Returns once the listener is bound.
pub async fn serve(bind: &str, state: ApiState) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    info!(%bind, "operator API listening");
    axum::serve(listener, router(state)).await?;
    Ok(())
}

/// The single configured chain_id, or an error if there are several (used to make
/// the bare routes unambiguous in the back-compat single-source case).
fn only_chain(s: &ApiState) -> Result<u64, (StatusCode, Json<Value>)> {
    if s.sources.len() == 1 {
        Ok(*s.sources.keys().next().unwrap())
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "multiple sources configured; address one with /{action}/{chain_id}",
                "chains": s.sources.keys().copied().collect::<Vec<_>>(),
            })),
        ))
    }
}

fn runtime_of<'a>(
    s: &'a ApiState,
    chain_id: u64,
) -> Result<&'a Arc<Mutex<Runtime>>, (StatusCode, Json<Value>)> {
    s.sources.get(&chain_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": format!("no source for chain_id {chain_id}") })),
        )
    })
}

async fn status_for(validator: &str, chain_id: u64, rt: &Arc<Mutex<Runtime>>) -> StatusResponse {
    let rt = rt.lock().await;
    StatusResponse {
        validator: validator.to_string(),
        chain_id,
        paused: rt.paused(),
        pause_reason: rt.pause_reason().map(|r| r.as_str()),
        last_block: rt.persist.last_block,
        next_block: rt.next_block(),
        nonces: rt.persist.nonces.clone(),
    }
}

/// One source -> the legacy single object (keeps phase6.sh working). Several ->
/// an array, one entry per chain.
async fn status_all(State(s): State<ApiState>) -> Json<Value> {
    if s.sources.len() == 1 {
        let (cid, rt) = s.sources.iter().next().unwrap();
        return Json(json!(status_for(&s.validator, *cid, rt).await));
    }
    let mut out = Vec::with_capacity(s.sources.len());
    for (cid, rt) in s.sources.iter() {
        out.push(status_for(&s.validator, *cid, rt).await);
    }
    Json(json!(out))
}

async fn status_one(
    State(s): State<ApiState>,
    Path(chain_id): Path<u64>,
) -> Result<Json<StatusResponse>, (StatusCode, Json<Value>)> {
    let rt = runtime_of(&s, chain_id)?;
    Ok(Json(status_for(&s.validator, chain_id, rt).await))
}

async fn pause_one(
    State(s): State<ApiState>,
    Path(chain_id): Path<u64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let rt = runtime_of(&s, chain_id)?;
    rt.lock().await.pause(crate::state::PauseReason::Operator);
    info!(chain_id, "scanner paused via operator API");
    Ok(Json(json!({ "ok": true, "chain_id": chain_id, "paused": true })))
}

async fn pause_bare(State(s): State<ApiState>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let cid = only_chain(&s)?;
    pause_one(State(s), Path(cid)).await
}

async fn resume_one(
    State(s): State<ApiState>,
    Path(chain_id): Path<u64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let rt = runtime_of(&s, chain_id)?;
    {
        let mut rt = rt.lock().await;
        rt.resume();
        let _ = rt.save();
    }
    info!(chain_id, "scanner resumed via operator API");
    Ok(Json(json!({ "ok": true, "chain_id": chain_id, "paused": false })))
}

async fn resume_bare(State(s): State<ApiState>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let cid = only_chain(&s)?;
    resume_one(State(s), Path(cid)).await
}

async fn rescan_one(
    State(s): State<ApiState>,
    Path(chain_id): Path<u64>,
    Json(req): Json<RescanRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let rt = runtime_of(&s, chain_id)?;
    let next_block = {
        let mut rt = rt.lock().await;
        rt.rescan_from(req.from_block);
        let _ = rt.save();
        rt.next_block()
    };
    info!(chain_id, from_block = req.from_block, "rescan requested via operator API");
    Ok(Json(
        json!({ "ok": true, "chain_id": chain_id, "rescan_from": req.from_block, "next_block": next_block }),
    ))
}

async fn rescan_bare(
    State(s): State<ApiState>,
    body: Json<RescanRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let cid = only_chain(&s)?;
    rescan_one(State(s), Path(cid), body).await
}
