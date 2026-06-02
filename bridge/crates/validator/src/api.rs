//! Operator HTTP API (mirrors this repo's `AppController`).
//!
//!   GET  /status              -> scanner cursor, pause state, nonce map
//!   POST /pause               -> pause scanning
//!   POST /resume              -> clear pause
//!   POST /rescan {from_block} -> reset cursor and re-scan from a block
//!
//! State is shared with the scan loop via `Arc<Mutex<Runtime>>`.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::info;

use crate::state::Runtime;

#[derive(Clone)]
pub struct ApiState {
    pub runtime: Arc<Mutex<Runtime>>,
    pub validator: String,
    pub chain_id: u64,
}

#[derive(Serialize)]
struct StatusResponse {
    validator: String,
    chain_id: u64,
    paused: bool,
    pause_reason: Option<String>,
    last_block: u64,
    next_block: u64,
    nonces: std::collections::BTreeMap<u64, u64>,
}

#[derive(Deserialize)]
struct RescanRequest {
    from_block: u64,
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/pause", post(pause))
        .route("/resume", post(resume))
        .route("/rescan", post(rescan))
        .with_state(state)
}

/// Spawn the operator API on `bind`. Returns once the listener is bound.
pub async fn serve(bind: &str, state: ApiState) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    info!(%bind, "operator API listening");
    axum::serve(listener, router(state)).await?;
    Ok(())
}

async fn status(State(s): State<ApiState>) -> Json<StatusResponse> {
    let rt = s.runtime.lock().await;
    Json(StatusResponse {
        validator: s.validator.clone(),
        chain_id: s.chain_id,
        paused: rt.paused,
        pause_reason: rt.pause_reason.as_ref().map(|r| r.as_str()),
        last_block: rt.persist.last_block,
        next_block: rt.next_block(),
        nonces: rt.persist.nonces.clone(),
    })
}

async fn pause(State(s): State<ApiState>) -> Json<Value> {
    let mut rt = s.runtime.lock().await;
    rt.pause(crate::state::PauseReason::Operator);
    info!("scanner paused via operator API");
    Json(json!({ "ok": true, "paused": true }))
}

async fn resume(State(s): State<ApiState>) -> Json<Value> {
    let mut rt = s.runtime.lock().await;
    rt.resume();
    let _ = rt.save();
    info!("scanner resumed via operator API");
    Json(json!({ "ok": true, "paused": false }))
}

async fn rescan(State(s): State<ApiState>, Json(req): Json<RescanRequest>) -> Json<Value> {
    let mut rt = s.runtime.lock().await;
    rt.rescan_from(req.from_block);
    let _ = rt.save();
    info!(from_block = req.from_block, "rescan requested via operator API");
    Json(json!({ "ok": true, "rescan_from": req.from_block, "next_block": rt.next_block() }))
}
