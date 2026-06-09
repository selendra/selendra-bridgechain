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
mod sink;
mod state;

use std::collections::BTreeMap;
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
use bridge_core::store::{SignerSig, SubmissionRecord};
use bridge_core::{AutoParams, Submission};
use config::{Config, SourceChain};
use sink::Sink;
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
    // One sink, shared across every per-source scan loop.
    let sink = Arc::new(Sink::from_config(&cfg.store)?);

    info!(
        validator = %signer_addr,
        sources = cfg.sources.len(),
        sink = %sink.describe(),
        "validator started"
    );

    // Build a runtime per source up front so the operator API can address each by
    // chain_id, then spawn one scan loop per source sharing those runtimes.
    let mut runtimes: BTreeMap<u64, Arc<Mutex<Runtime>>> = BTreeMap::new();
    for source in &cfg.sources {
        let state_path = PathBuf::from(&source.state_file);
        let runtime = Arc::new(Mutex::new(Runtime::load_or_init(&state_path, source.start_block)?));
        runtimes.insert(source.chain_id, runtime);
    }
    let runtimes = Arc::new(runtimes);

    if let Some(api) = &cfg.api {
        let api_state = api::ApiState {
            sources: runtimes.clone(),
            validator: format!("{signer_addr:#x}"),
        };
        let bind = api.bind.clone();
        tokio::spawn(async move {
            if let Err(e) = api::serve(&bind, api_state).await {
                warn!(error = %e, "operator API exited");
            }
        });
    }

    let mut tasks = tokio::task::JoinSet::new();
    for source in cfg.sources {
        let signer = signer.clone();
        let sink = sink.clone();
        let runtime = runtimes.get(&source.chain_id).unwrap().clone();
        tasks.spawn(async move { scan_source(source, signer, signer_addr, sink, runtime).await });
    }

    // Isolate a dead source loop so one bad chain can't stop the validator from
    // signing transfers on the others. Only error out once every loop has exited.
    let total = tasks.len();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(Ok(())) => warn!("a source scan loop exited on its own (other chains keep running)"),
            Ok(Err(e)) => warn!(error = %e, "a source scan loop failed (other chains keep running)"),
            Err(e) => warn!(error = %e, "a source task panicked (other chains keep running)"),
        }
    }
    anyhow::bail!("all {total} source scan loops have exited");
}

/// Scan one source chain forever: poll for `Sent`, verify, sign, store.
async fn scan_source(
    source: SourceChain,
    signer: PrivateKeySigner,
    signer_addr: Address,
    sink: Arc<Sink>,
    runtime: Arc<Mutex<Runtime>>,
) -> anyhow::Result<()> {
    let gate: Address = source.gate.parse().context("bad gate address")?;
    let retry = Duration::from_millis(source.poll_interval_ms.max(1000));

    // Multi-RPC failover, with a chainId guard per endpoint. Connecting can fail
    // if every endpoint is momentarily down/wrong-chain; retry rather than kill
    // this loop (and, with the isolation in main, never the sibling chains).
    let endpoints = source.endpoints()?;
    let mut failover = loop {
        match provider::Failover::connect(&endpoints, source.chain_id).await {
            Ok(f) => break f,
            Err(e) => {
                warn!(chain_id = source.chain_id, error = %e, "connecting RPC endpoints failed; retrying");
                tokio::time::sleep(retry).await;
            }
        }
    };

    let resume_from = runtime.lock().await.next_block();
    info!(
        validator = %signer_addr,
        gate = %gate,
        chain_id = source.chain_id,
        rpc = %failover.active_url(),
        endpoints = endpoints.len(),
        resume_from,
        "source scan loop started"
    );

    let sent_sig = Gate::Sent::SIGNATURE_HASH;

    loop {
        // Respect the pause flag (operator-set, or tripped by a nonce anomaly).
        {
            let rt = runtime.lock().await;
            if rt.paused {
                let reason = rt.pause_reason.as_ref().map(|r| r.as_str()).unwrap_or_default();
                drop(rt);
                warn!(chain_id = source.chain_id, %reason, "scanner PAUSED — not processing (resume via operator API)");
                tokio::time::sleep(Duration::from_millis(source.poll_interval_ms.max(1000))).await;
                continue;
            }
        }

        let from_block = runtime.lock().await.next_block();
        // Transient RPC failures must not kill the loop (which, pre-fix, also took
        // down every sibling chain). Log, back off, and try again next tick.
        let latest = match failover.get_block_number().await {
            Ok(v) => v,
            Err(e) => {
                warn!(chain_id = source.chain_id, error = %e, "get_block_number failed; retrying");
                tokio::time::sleep(retry).await;
                continue;
            }
        };
        let confirmed = latest.saturating_sub(source.block_confirmation);

        if confirmed >= from_block {
            let to_block = confirmed.min(from_block + source.max_block_range - 1);

            let filter = Filter::new()
                .address(gate)
                .event_signature(sent_sig)
                .from_block(from_block)
                .to_block(to_block);

            let mut logs = match failover.get_logs(&filter).await {
                Ok(v) => v,
                Err(e) => {
                    warn!(chain_id = source.chain_id, error = %e, "get_logs failed; retrying");
                    tokio::time::sleep(retry).await;
                    continue;
                }
            };
            // Process in chain order so nonce sequencing is meaningful.
            logs.sort_by_key(|l| (l.block_number.unwrap_or(0), l.log_index.unwrap_or(0)));

            let mut paused = false;
            for log in &logs {
                match handle_log(&signer, signer_addr, &sink, &runtime, log).await {
                    Ok(true) => {} // processed
                    Ok(false) => {
                        // a nonce anomaly paused the scanner; stop this batch
                        paused = true;
                        break;
                    }
                    Err(e) => warn!(chain_id = source.chain_id, error = %e, "failed handling log"),
                }
            }

            if !paused {
                // Advance and persist the cursor only after the whole batch is done.
                let mut rt = runtime.lock().await;
                rt.persist.last_block = to_block;
                if let Err(e) = rt.save() {
                    warn!(chain_id = source.chain_id, error = %e, "failed to persist cursor");
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(source.poll_interval_ms)).await;
    }
}

/// Returns `Ok(true)` if the event was processed (or harmlessly skipped),
/// `Ok(false)` if a nonce anomaly paused the scanner (caller should stop).
async fn handle_log(
    signer: &PrivateKeySigner,
    signer_addr: Address,
    sink: &Sink,
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

    sink.upsert(record, SignerSig { signer: format!("{signer_addr:#x}"), signature: sig_hex })
        .await?;

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
