//! Minimal keeper / executor (Phase 5).
//!
//! One claim loop *per configured target chain*: read the signature store, and
//! for every record destined for that chain that has >= threshold signatures and
//! isn't yet executed, build and submit `claim()` (signatures sorted by signer
//! ascending, as the Gate requires). Configuring several `[[targets]]` lets a
//! single keeper deliver A->B and A->C transfers from the same source.

mod config;
mod source;

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use alloy::network::EthereumWallet;
use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use anyhow::Context;
use bridge_core::abi::Gate;
use bridge_core::store::SubmissionRecord;
use config::{Config, TargetChain};
use source::Source;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "keeper=info".into()),
        )
        .init();

    let cfg_path = std::env::args().nth(1).unwrap_or_else(|| "keeper.toml".into());
    let cfg = Config::load(&cfg_path)?;

    let signer: PrivateKeySigner = cfg.keeper.private_key.parse().context("bad private_key")?;
    // Shared across every per-target loop (one HTTP client / one dir handle).
    let source = Arc::new(Source::from_config(&cfg.store)?);

    info!(
        keeper = %signer.address(),
        targets = cfg.targets.len(),
        source = %source.describe(),
        "keeper started"
    );

    // Spawn one independent claim loop per destination chain. A loop only returns
    // on a permanent misconfig (e.g. wrong chainId); transient RPC failures are
    // retried inside it. We isolate a dead loop so one bad chain can't take down
    // delivery to the others — only when EVERY loop has exited do we error out.
    let mut tasks = tokio::task::JoinSet::new();
    for target in cfg.targets {
        let signer = signer.clone();
        let source = source.clone();
        tasks.spawn(async move { run_target(target, signer, source).await });
    }

    let total = tasks.len();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(Ok(())) => warn!("a target loop exited on its own (other chains keep running)"),
            Ok(Err(e)) => warn!(error = %e, "a target loop failed (other chains keep running)"),
            Err(e) => warn!(error = %e, "a target task panicked (other chains keep running)"),
        }
    }
    anyhow::bail!("all {total} target loops have exited");
}

/// Claim loop for a single destination chain.
async fn run_target(
    target: TargetChain,
    signer: PrivateKeySigner,
    source: Arc<Source>,
) -> anyhow::Result<()> {
    let wallet = EthereumWallet::from(signer.clone());
    let gate_addr: Address = target.gate.parse().context("bad gate address")?;
    let retry = Duration::from_millis(target.poll_interval_ms.max(1000));

    let provider = ProviderBuilder::new()
        .wallet(wallet)
        .connect_http(target.rpc.parse()?);

    // Verify the RPC is on the expected chain. Unreachable => transient, retry.
    // Wrong chainId => permanent misconfig, return Err (isolated; siblings live).
    loop {
        match provider.get_chain_id().await {
            Ok(id) if id == target.chain_id => break,
            Ok(id) => anyhow::bail!(
                "RPC chainId {id} != configured {} for {}",
                target.chain_id,
                target.rpc
            ),
            Err(e) => {
                warn!(chain_id = target.chain_id, error = %e, "get_chain_id failed; retrying");
                tokio::time::sleep(retry).await;
            }
        }
    }

    let gate = Gate::new(gate_addr, &provider);
    let threshold: u64 = loop {
        match gate.threshold().call().await {
            Ok(t) => break t.try_into().unwrap_or(u64::MAX),
            Err(e) => {
                warn!(chain_id = target.chain_id, error = %e, "read threshold failed; retrying");
                tokio::time::sleep(retry).await;
            }
        }
    };

    info!(
        keeper = %signer.address(),
        gate = %gate_addr,
        chain_id = target.chain_id,
        threshold,
        "target loop started"
    );

    loop {
        // Allowlist for this tick. Fail-closed: if the sig-store is unreachable,
        // skip the tick rather than claim on a stale view. None => file mode
        // (no central allowlist, enforcement disabled).
        let allowlist = match source.fetch_allowlist().await {
            Ok(a) => a,
            Err(e) => {
                warn!(chain_id = target.chain_id, error = %e, "allowlist fetch failed; skipping tick");
                tokio::time::sleep(retry).await;
                continue;
            }
        };

        let records = source.load_all().await.unwrap_or_default();
        for rec in records {
            if rec.chain_id_to != target.chain_id {
                continue;
            }
            if (rec.signatures.len() as u64) < threshold {
                continue;
            }
            // Second enforcement gate (validators are the first): never submit a
            // claim for a non-whitelisted token or chain pair.
            if let Some(allow) = &allowlist {
                if !allow.token_allowed(&rec.debridge_id)
                    || !allow.chain_allowed(rec.chain_id_from, rec.chain_id_to)
                {
                    warn!(
                        chain_id = target.chain_id,
                        submission_id = %rec.submission_id,
                        "BLOCKED by allowlist — refusing to claim"
                    );
                    continue;
                }
            }
            match try_claim(&gate, &rec).await {
                Ok(Some(tx)) => {
                    if let Err(e) = source.mark_claimed(&rec.submission_id, &tx).await {
                        warn!(
                            chain_id = target.chain_id,
                            submission_id = %rec.submission_id,
                            error = %e,
                            "claimed on-chain but failed to record status"
                        );
                    }
                }
                Ok(None) => {} // already executed
                Err(e) => warn!(
                    chain_id = target.chain_id,
                    submission_id = %rec.submission_id,
                    error = %e,
                    "claim failed"
                ),
            }
        }
        tokio::time::sleep(Duration::from_millis(target.poll_interval_ms)).await;
    }
}

/// Submit `claim()` for one record. Returns `Some(tx_hash)` on a fresh claim,
/// `None` if it was already executed (by us or another keeper).
async fn try_claim<P: Provider>(
    gate: &Gate::GateInstance<P>,
    rec: &SubmissionRecord,
) -> anyhow::Result<Option<String>> {
    let submission_id = B256::from_str(&rec.submission_id).context("bad submission_id")?;

    if gate.executed(submission_id).call().await? {
        return Ok(None); // already executed (by us or another keeper)
    }

    let debridge_id = B256::from_str(&rec.debridge_id).context("bad debridge_id")?;
    let amount = U256::from_str(&rec.amount).context("bad amount")?;
    let receiver = bytes_of(&rec.receiver)?;
    let auto_params = bytes_of(&rec.auto_params)?;
    let native_sender = bytes_of(&rec.native_sender)?;

    // signatures MUST be ordered by signer address, strictly ascending
    let mut sigs = rec.signatures.clone();
    sigs.sort_by(|a, b| {
        let aa = Address::from_str(&a.signer).unwrap_or(Address::ZERO);
        let bb = Address::from_str(&b.signer).unwrap_or(Address::ZERO);
        aa.cmp(&bb)
    });
    let signatures: Vec<Bytes> = sigs
        .iter()
        .map(|s| bytes_of(&s.signature))
        .collect::<anyhow::Result<_>>()?;

    info!(submission_id = %rec.submission_id, sigs = signatures.len(), "submitting claim()");

    let pending = gate
        .claim(
            debridge_id,
            amount,
            U256::from(rec.chain_id_from),
            U256::from(rec.nonce),
            receiver,
            auto_params,
            native_sender,
            signatures,
        )
        .send()
        .await
        .context("send claim")?;

    let receipt = pending.get_receipt().await.context("await receipt")?;
    info!(
        submission_id = %rec.submission_id,
        tx = %receipt.transaction_hash,
        status = receipt.status(),
        "CLAIMED"
    );
    Ok(Some(format!("{:#x}", receipt.transaction_hash)))
}

fn bytes_of(hex_str: &str) -> anyhow::Result<Bytes> {
    let s = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    if s.is_empty() {
        return Ok(Bytes::new());
    }
    Ok(Bytes::from(hex::decode(s)?))
}
