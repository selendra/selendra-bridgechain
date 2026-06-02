//! External-validator node.
//!
//! Phase 4 gave us the core loop: scan the source chain for `Sent`, recompute
//! `submissionId`, sign it (EIP-191 `eth_sign`) only if it matches, store it.
//!
//! Phase 6 hardens it into the real node:
//!   * multi-RPC failover with a chainId guard ([`provider::Failover`]),
//!   * a finality buffer (`block_confirmation`),
//!   * a resumable cursor persisted to disk ([`state::Runtime`]),
//!   * sequential-nonce enforcement — a missed or duplicated nonce *pauses* the
//!     scanner instead of silently signing,
//!   * an operator HTTP API (pause / resume / rescan / status).

mod api;
mod config;
mod provider;
mod state;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::{Address, B256, U256};
use alloy::rpc::types::Filter;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use alloy_sol_types::{SolEvent, SolValue};
use anyhow::Context;
use bridge_core::abi::{AutoParamsTo, Gate};
use bridge_core::store::{self, SignerSig, SubmissionRecord};
use bridge_core::{AutoParams, Submission};
use config::Config;
use state::{NonceDecision, PauseReason, Runtime};
use tokio::sync::Mutex;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "validator=info,bridge_core=info".into()),
        )
        .init();

    let cfg_path = std::env::args().nth(1).unwrap_or_else(|| "validator.toml".into());
    let cfg = Config::load(&cfg_path)?;

    let signer: PrivateKeySigner = cfg.signer.private_key.parse().context("bad private_key")?;
    let signer_addr = signer.address();
    let gate: Address = cfg.source.gate.parse().context("bad gate address")?;
    let store_dir = PathBuf::from(&cfg.store.dir);
    store::ensure_dir(&store_dir)?;

    // Multi-RPC failover, with a chainId guard per endpoint.
    let endpoints = cfg.source.endpoints()?;
    let mut failover = provider::Failover::connect(&endpoints, cfg.source.chain_id)
        .await
        .context("connecting RPC endpoints")?;

    // Resumable cursor + nonce state, shared with the operator API.
    let state_path = PathBuf::from(&cfg.source.state_file);
    let runtime = Arc::new(Mutex::new(Runtime::load_or_init(&state_path, cfg.source.start_block)?));

    if let Some(api) = &cfg.api {
        let api_state = api::ApiState {
            runtime: runtime.clone(),
            validator: format!("{signer_addr:#x}"),
            chain_id: cfg.source.chain_id,
        };
        let bind = api.bind.clone();
        tokio::spawn(async move {
            if let Err(e) = api::serve(&bind, api_state).await {
                warn!(error = %e, "operator API exited");
            }
        });
    }

    info!(
        validator = %signer_addr,
        gate = %gate,
        chain_id = cfg.source.chain_id,
        rpc = %failover.active_url(),
        endpoints = endpoints.len(),
        resume_from = { runtime.lock().await.next_block() },
        "validator started"
    );

    let sent_sig = Gate::Sent::SIGNATURE_HASH;

    loop {
        // Respect the pause flag (operator-set, or tripped by a nonce anomaly).
        {
            let rt = runtime.lock().await;
            if rt.paused {
                let reason = rt.pause_reason.as_ref().map(|r| r.as_str()).unwrap_or_default();
                drop(rt);
                warn!(%reason, "scanner PAUSED — not processing (resume via operator API)");
                tokio::time::sleep(Duration::from_millis(cfg.source.poll_interval_ms.max(1000))).await;
                continue;
            }
        }

        let from_block = runtime.lock().await.next_block();
        let latest = failover.get_block_number().await.context("get_block_number")?;
        let confirmed = latest.saturating_sub(cfg.source.block_confirmation);

        if confirmed >= from_block {
            let to_block = confirmed.min(from_block + cfg.source.max_block_range - 1);

            let filter = Filter::new()
                .address(gate)
                .event_signature(sent_sig)
                .from_block(from_block)
                .to_block(to_block);

            let mut logs = failover.get_logs(&filter).await.context("get_logs")?;
            // Process in chain order so nonce sequencing is meaningful.
            logs.sort_by_key(|l| (l.block_number.unwrap_or(0), l.log_index.unwrap_or(0)));

            let mut paused = false;
            for log in &logs {
                match handle_log(&signer, signer_addr, &store_dir, &runtime, log).await {
                    Ok(true) => {} // processed
                    Ok(false) => {
                        // a nonce anomaly paused the scanner; stop this batch
                        paused = true;
                        break;
                    }
                    Err(e) => warn!(error = %e, "failed handling log"),
                }
            }

            if !paused {
                // Advance and persist the cursor only after the whole batch is done.
                let mut rt = runtime.lock().await;
                rt.persist.last_block = to_block;
                if let Err(e) = rt.save() {
                    warn!(error = %e, "failed to persist cursor");
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(cfg.source.poll_interval_ms)).await;
    }
}

/// Returns `Ok(true)` if the event was processed (or harmlessly skipped),
/// `Ok(false)` if a nonce anomaly paused the scanner (caller should stop).
async fn handle_log(
    signer: &PrivateKeySigner,
    signer_addr: Address,
    store_dir: &std::path::Path,
    runtime: &Arc<Mutex<Runtime>>,
    log: &alloy::rpc::types::Log,
) -> anyhow::Result<bool> {
    let decoded = Gate::Sent::decode_log(&log.inner).context("decode Sent")?;
    let ev = &decoded.data;

    let emitted_id: B256 = ev.submissionId;
    let chain_to = u256_to_u64(ev.chainIdTo);
    let nonce = u256_to_u64(ev.nonce);

    // Sequential-nonce enforcement (mirrors NonceControllingService).
    {
        let mut rt = runtime.lock().await;
        match rt.check_nonce(chain_to, nonce) {
            NonceDecision::Accept => {}
            NonceDecision::Missed => {
                let expected = rt.persist.nonces.get(&chain_to).map(|n| n + 1).unwrap_or(0);
                warn!(chain_to, expected, got = nonce, "MISSED_NONCE — pausing scanner");
                rt.pause(PauseReason::MissedNonce { chain_to, expected, got: nonce });
                let _ = rt.save();
                return Ok(false);
            }
            NonceDecision::Duplicated => {
                let last = rt.persist.nonces.get(&chain_to).copied().unwrap_or(0);
                warn!(chain_to, last, got = nonce, "DUPLICATED_NONCE — pausing scanner");
                rt.pause(PauseReason::DuplicatedNonce { chain_to, last, got: nonce });
                let _ = rt.save();
                return Ok(false);
            }
        }
    }

    // Independently recompute the submissionId; never sign one we can't reproduce.
    let submission = submission_from_event(ev);
    let computed_id = submission.compute_id();
    if computed_id != emitted_id {
        warn!(
            emitted = %emitted_id,
            computed = %computed_id,
            "submissionId MISMATCH — refusing to sign and pausing (bad/lying RPC?)"
        );
        let mut rt = runtime.lock().await;
        rt.pause(PauseReason::IdMismatch { submission_id: format!("{emitted_id:#x}") });
        let _ = rt.save();
        return Ok(false);
    }

    // EIP-191 eth_sign over the raw 32-byte submissionId.
    let sig = signer.sign_message(emitted_id.as_slice()).await?;
    let sig_hex = encode_signature(&sig);

    let record = SubmissionRecord {
        submission_id: format!("{emitted_id:#x}"),
        debridge_id: format!("{:#x}", ev.debridgeId),
        amount: ev.amount.to_string(),
        chain_id_from: u256_to_u64(ev.chainIdFrom),
        chain_id_to: chain_to,
        nonce,
        receiver: format!("0x{}", hex::encode(&ev.receiver)),
        auto_params: format!("0x{}", hex::encode(&ev.autoParams)),
        native_sender: format!("0x{}", hex::encode(&ev.nativeSender)),
        signatures: vec![],
    };

    store::upsert_signature(
        store_dir,
        record,
        SignerSig { signer: format!("{signer_addr:#x}"), signature: sig_hex },
    )?;

    // Record the accepted nonce only after a successful sign+store.
    runtime.lock().await.accept_nonce(chain_to, nonce);

    info!(
        submission_id = %emitted_id,
        nonce,
        chain_to,
        "SIGNED and stored"
    );
    Ok(true)
}

/// Build our independent `Submission` from a decoded `Sent` event.
fn submission_from_event(ev: &Gate::Sent) -> Submission {
    let auto = if ev.autoParams.is_empty() {
        None
    } else {
        match AutoParamsTo::abi_decode(&ev.autoParams) {
            Ok(ap) => Some(AutoParams {
                execution_fee: ap.executionFee,
                flags: ap.flags,
                fallback_address: ap.fallbackAddress.to_vec(),
                data: ap.data.to_vec(),
                native_sender: ev.nativeSender.to_vec(),
            }),
            Err(_) => None,
        }
    };

    Submission {
        debridge_id: ev.debridgeId,
        amount: ev.amount,
        chain_id_from: ev.chainIdFrom,
        chain_id_to: ev.chainIdTo,
        nonce: ev.nonce,
        receiver: ev.receiver.to_vec(),
        auto,
    }
}

/// Encode an alloy signature as 65 bytes r||s||v with v in {27,28} (OZ ECDSA form).
fn encode_signature(sig: &alloy::primitives::Signature) -> String {
    let mut out = Vec::with_capacity(65);
    out.extend_from_slice(&sig.r().to_be_bytes::<32>());
    out.extend_from_slice(&sig.s().to_be_bytes::<32>());
    out.push(27 + sig.v() as u8);
    format!("0x{}", hex::encode(out))
}

fn u256_to_u64(v: U256) -> u64 {
    v.try_into().unwrap_or(u64::MAX)
}
