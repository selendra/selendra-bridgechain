//! Minimal keeper / executor (Phase 5).
//!
//! One claim loop *per configured target chain*: read the signature store, and
//! for every record destined for that chain that has >= threshold signatures and
//! isn't yet executed, build and submit `claim()` (signatures sorted by signer
//! ascending, as the Gate requires). Configuring several `[[targets]]` lets a
//! single keeper deliver A->B and A->C transfers from the same source.
//!
//! It also relays the two-phase refund, which runs on both sides:
//!   * on a `[[targets]]` chain, a **cancel** quorum burns a stranded transfer
//!     (`cancel()`), taking precedence over claiming it;
//!   * on a `[[sources]]` chain, a **refund** quorum returns the locked funds
//!     (`refund()`).
//!
//! The keeper decides nothing here — it only relays quorums the validators
//! formed after checking both chains themselves. It holds no authority the
//! signatures don't already carry.

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
use bridge_core::store::{SignerSig, SubmissionRecord};
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

    let signer = cfg.keeper.load("keeper").context("loading keeper signer")?;
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

    // And one refund loop per SOURCE chain. Refunds pay out where the funds were
    // locked, so they belong to the source side, not the claim targets.
    for src in cfg.sources {
        let signer = signer.clone();
        let store = source.clone();
        tasks.spawn(async move { run_source_refunds(src, signer, store).await });
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

    // SimpleNonceManager fetches the pending nonce from the chain for every tx,
    // instead of the default CachedNonceManager which keeps a local counter.
    //
    // The cached manager is unsafe here: a tx whose gas estimation reverts (e.g.
    // a claim for an asset the destination hasn't registered — exactly the
    // stranded transfers this keeper is meant to cancel) still advances the
    // cached nonce before failing to send. After a run of such failures the
    // cache sits far ahead of the chain, so the NEXT real tx (a cancel, say)
    // broadcasts with a gap and hangs pending forever. The keeper submits txs
    // one at a time and awaits each receipt, so a fresh per-tx fetch is correct.
    let provider = ProviderBuilder::new()
        .wallet(wallet)
        .with_simple_nonce_management()
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
            // Cancels are handled BEFORE the transfer-threshold and allowlist
            // gates, and deliberately so. Both of those exist to protect payouts,
            // and a cancel is the opposite of a payout — it releases nothing and
            // only burns the transfer so the source can repay the sender.
            //
            // Checking them first would strand precisely the transfers that need
            // refunding most: a transfer the allowlist rejects never collects
            // transfer signatures at all, so it would fail the threshold check,
            // never reach this branch, and its funds would stay locked forever.
            if (rec.cancel_signatures.len() as u64) >= threshold {
                match try_cancel(&gate, &rec).await {
                    Ok(Some(tx)) => {
                        if let Err(e) = source.mark_cancelled(&rec.submission_id, &tx).await {
                            warn!(
                                chain_id = target.chain_id,
                                submission_id = %rec.submission_id,
                                error = %e,
                                "cancelled on-chain but failed to record status"
                            );
                        }
                    }
                    Ok(None) => {} // already executed (claimed or cancelled)
                    Err(e) => warn!(
                        chain_id = target.chain_id,
                        submission_id = %rec.submission_id,
                        error = %e,
                        "cancel failed"
                    ),
                }
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

/// Refund loop for a single SOURCE chain: submit `refund()` for transfers whose
/// destination has already been burned and which have a refund quorum.
///
/// The keeper does not decide anything here — it only relays quorums the
/// validators formed. Both the "was it really burned?" and "was it really sent
/// from this gate?" questions are answered by the validators' on-chain checks
/// and by the Gate's own `sentBy` guard respectively.
async fn run_source_refunds(
    src: config::SourceChain,
    signer: PrivateKeySigner,
    store: Arc<Source>,
) -> anyhow::Result<()> {
    let wallet = EthereumWallet::from(signer.clone());
    let gate_addr: Address = src.gate.parse().context("bad gate address")?;
    let retry = Duration::from_millis(src.poll_interval_ms.max(1000));

    // Fresh per-tx nonce fetch — see the note in run_target on why the default
    // cached manager corrupts the sequence after a reverting send.
    let provider = ProviderBuilder::new()
        .wallet(wallet)
        .with_simple_nonce_management()
        .connect_http(src.rpc.parse()?);

    loop {
        match provider.get_chain_id().await {
            Ok(id) if id == src.chain_id => break,
            Ok(id) => anyhow::bail!("RPC chainId {id} != configured {} for {}", src.chain_id, src.rpc),
            Err(e) => {
                warn!(chain_id = src.chain_id, error = %e, "get_chain_id failed; retrying");
                tokio::time::sleep(retry).await;
            }
        }
    }

    let gate = Gate::new(gate_addr, &provider);
    let threshold: u64 = loop {
        match gate.threshold().call().await {
            Ok(t) => break t.try_into().unwrap_or(u64::MAX),
            Err(e) => {
                warn!(chain_id = src.chain_id, error = %e, "read threshold failed; retrying");
                tokio::time::sleep(retry).await;
            }
        }
    };

    info!(
        keeper = %signer.address(),
        gate = %gate_addr,
        chain_id = src.chain_id,
        threshold,
        "source refund loop started"
    );

    loop {
        let records = store.load_all().await.unwrap_or_default();
        for rec in records {
            if rec.chain_id_from != src.chain_id {
                continue;
            }
            if (rec.refund_signatures.len() as u64) < threshold {
                continue;
            }
            match try_refund(&gate, &rec).await {
                Ok(Some(tx)) => {
                    if let Err(e) = store.mark_refunded(&rec.submission_id, &tx).await {
                        warn!(
                            chain_id = src.chain_id,
                            submission_id = %rec.submission_id,
                            error = %e,
                            "refunded on-chain but failed to record status"
                        );
                    }
                }
                Ok(None) => {} // already refunded, or never sent from this gate
                Err(e) => warn!(
                    chain_id = src.chain_id,
                    submission_id = %rec.submission_id,
                    error = %e,
                    "refund failed"
                ),
            }
        }
        tokio::time::sleep(Duration::from_millis(src.poll_interval_ms)).await;
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

    // Skip a claim that would certainly revert with UnknownAsset: if the
    // destination gate has no local token registered for this debridgeId, there
    // is nothing to release. This is exactly the stranded-transfer case the
    // refund path handles — without this guard the keeper would re-attempt the
    // claim every tick, hammering the RPC and flooding the logs, until the
    // transfer is cancelled. If the asset is registered later, the claim resumes.
    if gate.tokenOf(debridge_id).call().await? == Address::ZERO {
        return Ok(None);
    }

    let amount = U256::from_str(&rec.amount).context("bad amount")?;
    let receiver = bytes_of(&rec.receiver)?;
    let auto_params = bytes_of(&rec.auto_params)?;
    let native_sender = bytes_of(&rec.native_sender)?;

    // signatures MUST be ordered by signer address, strictly ascending
    let signatures = sorted_signatures(&rec.signatures)?;

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

/// Submit `cancel()` on the destination. `None` if it is already executed
/// (claimed or cancelled) — either way there is nothing to burn.
async fn try_cancel<P: Provider>(
    gate: &Gate::GateInstance<P>,
    rec: &SubmissionRecord,
) -> anyhow::Result<Option<String>> {
    let submission_id = B256::from_str(&rec.submission_id).context("bad submission_id")?;
    if gate.executed(submission_id).call().await? {
        return Ok(None);
    }

    let debridge_id = B256::from_str(&rec.debridge_id).context("bad debridge_id")?;
    let amount = U256::from_str(&rec.amount).context("bad amount")?;

    info!(
        submission_id = %rec.submission_id,
        sigs = rec.cancel_signatures.len(),
        "submitting cancel() — burning the transfer on the destination"
    );

    let pending = gate
        .cancel(
            debridge_id,
            amount,
            U256::from(rec.chain_id_from),
            U256::from(rec.nonce),
            bytes_of(&rec.receiver)?,
            bytes_of(&rec.auto_params)?,
            bytes_of(&rec.native_sender)?,
            sorted_signatures(&rec.cancel_signatures)?,
        )
        .send()
        .await
        .context("send cancel")?;

    let receipt = pending.get_receipt().await.context("await receipt")?;
    info!(submission_id = %rec.submission_id, tx = %receipt.transaction_hash, "CANCELLED");
    Ok(Some(format!("{:#x}", receipt.transaction_hash)))
}

/// Submit `refund()` on the source. `None` if already refunded, if this gate
/// never emitted the id, or if we don't know which token was locked.
async fn try_refund<P: Provider>(
    gate: &Gate::GateInstance<P>,
    rec: &SubmissionRecord,
) -> anyhow::Result<Option<String>> {
    let submission_id = B256::from_str(&rec.submission_id).context("bad submission_id")?;

    if gate.refunded(submission_id).call().await? {
        return Ok(None);
    }
    // `sentBy` is the gate's own record that it locked these funds; zero means
    // there is nothing to return (and `refund()` would revert with NotSent).
    if gate.sentBy(submission_id).call().await? == Address::ZERO {
        return Ok(None);
    }

    // The locked ERC-20 is not derivable from debridgeId (a one-way hash), so it
    // is carried on the record — and re-checked on-chain, which is why supplying
    // it from the store is safe: a wrong token reverts rather than paying out.
    if rec.token.is_empty() {
        warn!(
            submission_id = %rec.submission_id,
            "refund quorum reached but the locked token is unknown for this record \
             (pre-refund-path row); re-index its Sent event to populate it"
        );
        return Ok(None);
    }
    let token = Address::from_str(&rec.token).context("bad token")?;
    let debridge_id = B256::from_str(&rec.debridge_id).context("bad debridge_id")?;
    let amount = U256::from_str(&rec.amount).context("bad amount")?;

    info!(
        submission_id = %rec.submission_id,
        sigs = rec.refund_signatures.len(),
        "submitting refund() — returning locked funds on the source"
    );

    let pending = gate
        .refund(
            token,
            debridge_id,
            amount,
            U256::from(rec.chain_id_to),
            U256::from(rec.nonce),
            bytes_of(&rec.receiver)?,
            bytes_of(&rec.auto_params)?,
            bytes_of(&rec.native_sender)?,
            sorted_signatures(&rec.refund_signatures)?,
        )
        .send()
        .await
        .context("send refund")?;

    let receipt = pending.get_receipt().await.context("await receipt")?;
    info!(submission_id = %rec.submission_id, tx = %receipt.transaction_hash, "REFUNDED");
    Ok(Some(format!("{:#x}", receipt.transaction_hash)))
}

/// Signatures ordered by recovered signer ascending, as every Gate entry point
/// requires (the ordering is what dedupes signers on-chain).
fn sorted_signatures(sigs: &[SignerSig]) -> anyhow::Result<Vec<Bytes>> {
    let mut sigs = sigs.to_vec();
    sigs.sort_by(|a, b| {
        let aa = Address::from_str(&a.signer).unwrap_or(Address::ZERO);
        let bb = Address::from_str(&b.signer).unwrap_or(Address::ZERO);
        aa.cmp(&bb)
    });
    sigs.iter().map(|s| bytes_of(&s.signature)).collect()
}

fn bytes_of(hex_str: &str) -> anyhow::Result<Bytes> {
    let s = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    if s.is_empty() {
        return Ok(Bytes::new());
    }
    Ok(Bytes::from(hex::decode(s)?))
}
