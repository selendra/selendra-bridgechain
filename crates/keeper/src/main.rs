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

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy::network::EthereumWallet;
use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use anyhow::Context;
use bridge_core::abi::Gate;
use bridge_core::backend::StoreBackend;
use bridge_core::store::{SignerSig, SubmissionRecord};
use config::{ChainCfg, Config};
use tracing::{debug, info, warn};

/// Upper bound on waiting for a submitted tx's receipt. A tx that never confirms
/// within this window (stuck/underpriced/replaced) makes `get_receipt` return an
/// error instead of blocking the whole per-chain loop forever; the record is
/// retried next tick, and each `try_*` re-checks on-chain state first so a retry
/// after a tx actually landed is a no-op.
const RECEIPT_TIMEOUT: Duration = Duration::from_secs(120);

/// How often [`GateView`] re-reads `threshold`, `validatorCount` and validator
/// membership from the gate.
///
/// These used to be a startup snapshot (`threshold`) or a memo that never expired
/// (membership), so an on-chain validator-set change needed a keeper restart to
/// take effect. That is not merely stale reporting: a cached `true` for a
/// validator that has since been removed lets the built signature array exceed
/// the CURRENT `validatorCount`, which `Gate._verifySignatures` rejects outright.
const GATE_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

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
    // L-5: read + mark-claimed only — it cannot deposit signatures.
    let source = Arc::new(StoreBackend::from_config(&cfg.store, "SIG_STORE_KEEPER_TOKEN")?);

    info!(
        keeper = %signer.address(),
        targets = cfg.targets.len(),
        sources = cfg.sources.len(),
        source = %source.describe(),
        "keeper started"
    );

    // A chain listed as BOTH a claim target and a refund source (a bidirectional
    // corridor) gets two loops submitting from the same account on the same chain
    // concurrently. Each has its own fresh-nonce provider, so under simultaneous
    // load they can fetch the same pending nonce and one tx is rejected
    // (nonce-too-low) — self-healing on the next tick, but worth flagging. For a
    // busy bidirectional keeper, run the target and source roles as separate
    // processes (or separate signer accounts) to avoid the contention.
    for t in &cfg.targets {
        if cfg.sources.iter().any(|s| s.chain_id == t.chain_id) {
            warn!(
                chain_id = t.chain_id,
                "chain is both a claim target and a refund source; the two loops share one \
                 account and may briefly contend on nonces under load (self-healing). Consider \
                 separate keeper processes for the two roles."
            );
        }
    }

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

/// Connect to one chain and read its gate's parameters — the identical prologue
/// every submit loop needs (claims on a target, refunds on a source).
///
/// Returns the signing provider plus the initial [`GateView`]. Transient RPC
/// failures retry here rather than killing the loop; only a wrong `chainId` is
/// fatal, because that is a permanent misconfiguration and submitting to the
/// wrong network is not something to keep retrying.
async fn connect_gate(
    chain: &ChainCfg,
    signer: &PrivateKeySigner,
    role: &'static str,
) -> anyhow::Result<(impl Provider + Clone, Address, GateView)> {
    let wallet = EthereumWallet::from(signer.clone());
    let gate_addr: Address = chain.gate.parse().context("bad gate address")?;
    let retry = Duration::from_millis(chain.poll_interval_ms.max(1000));

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
        .connect_http(chain.rpc.parse()?);

    // Verify the RPC is on the expected chain. Unreachable => transient, retry.
    // Wrong chainId => permanent misconfig, return Err (isolated; siblings live).
    loop {
        match provider.get_chain_id().await {
            Ok(id) if id == chain.chain_id => break,
            Ok(id) => {
                anyhow::bail!("RPC chainId {id} != configured {} for {}", chain.chain_id, chain.rpc)
            }
            Err(e) => {
                warn!(chain_id = chain.chain_id, error = %e, "get_chain_id failed; retrying");
                tokio::time::sleep(retry).await;
            }
        }
    }

    let view = loop {
        match GateView::load(&Gate::new(gate_addr, &provider)).await {
            Ok(v) => break v,
            Err(e) => {
                warn!(chain_id = chain.chain_id, error = %e, "read gate params failed; retrying");
                tokio::time::sleep(retry).await;
            }
        }
    };

    info!(
        keeper = %signer.address(),
        gate = %gate_addr,
        chain_id = chain.chain_id,
        threshold = view.threshold,
        validator_count = view.validator_count,
        "{role} loop started"
    );
    Ok((provider, gate_addr, view))
}

/// Claim loop for a single destination chain.
async fn run_target(
    target: ChainCfg,
    signer: PrivateKeySigner,
    source: Arc<StoreBackend>,
) -> anyhow::Result<()> {
    let retry = Duration::from_millis(target.poll_interval_ms.max(1000));
    let (provider, gate_addr, mut view) = connect_gate(&target, &signer, "target").await?;
    let gate = Gate::new(gate_addr, &provider);

    loop {
        view.refresh_if_stale(&gate).await;
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
            let cancel_sigs = view.member_signatures(&gate, &rec.cancel_signatures).await;
            if cancel_sigs.len() as u64 >= view.threshold {
                match try_cancel(&gate, &rec, &cancel_sigs).await {
                    // The DB `refund_status` is advanced by the indexer when it
                    // observes the resulting `Cancelled` event on-chain, not
                    // reported here — the keeper's word is not authoritative for a
                    // state that gates the refund-candidate list.
                    Ok(Some(_tx)) => {}
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

            let claim_sigs = view.member_signatures(&gate, &rec.signatures).await;
            if (claim_sigs.len() as u64) < view.threshold {
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

            match try_claim(&gate, &rec, &claim_sigs).await {
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
    src: ChainCfg,
    signer: PrivateKeySigner,
    store: Arc<StoreBackend>,
) -> anyhow::Result<()> {
    let (provider, gate_addr, mut view) = connect_gate(&src, &signer, "source refund").await?;
    let gate = Gate::new(gate_addr, &provider);

    loop {
        view.refresh_if_stale(&gate).await;
        let records = store.load_all().await.unwrap_or_default();
        for rec in records {
            if rec.chain_id_from != src.chain_id {
                continue;
            }
            let refund_sigs = view.member_signatures(&gate, &rec.refund_signatures).await;
            if (refund_sigs.len() as u64) < view.threshold {
                continue;
            }
            match try_refund(&gate, &rec, &refund_sigs).await {
                // As with cancel, the indexer records `refund_status = refunded`
                // from the observed on-chain `Refunded` event; the keeper does not
                // report a state that gates the candidate list.
                Ok(Some(_tx)) => {}
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
///
/// `sigs` MUST already be filtered to the gate's on-chain validator set (see
/// [`GateView::member_signatures`]). The record's own `signatures` field is
/// deliberately NOT read here: it may contain signatures from any key at all, and
/// forwarding those is what makes a transfer permanently unclaimable.
async fn try_claim<P: Provider>(
    gate: &Gate::GateInstance<P>,
    rec: &SubmissionRecord,
    sigs: &[SignerSig],
) -> anyhow::Result<Option<String>> {
    let submission_id = B256::from_str(&rec.submission_id).context("bad submission_id")?;

    // An EVM `claim` decodes `receiver` as an address and now requires EXACTLY 20
    // bytes (it used to truncate anything longer, silently paying an unowned
    // address). A 32-byte receiver here means the transfer was addressed for a
    // non-EVM VM but routed to an EVM chain, so the claim is certain to revert
    // `BadReceiver`. Skip it rather than burn a tx and an RPC round trip every
    // tick — this is exactly the stranded-transfer case the two-phase refund
    // recovers, same posture as the `tokenOf == 0` guard below.
    // DEBUG, not WARN: this runs on every sweep for as long as the transfer sits
    // unclaimed, which is at minimum the whole refund timeout. Warning here put
    // ~1.5 lines/second per stranded transfer into the log and buried everything
    // else — the same reasoning the `tokenOf == 0` guard below documents. The
    // operator still sees the transfer's fate, because the cancel and the refund
    // that recover it are both logged at INFO.
    let receiver_len = rec.receiver.strip_prefix("0x").unwrap_or(&rec.receiver).len() / 2;
    if receiver_len != 20 {
        debug!(
            submission_id = %rec.submission_id,
            receiver_len,
            "receiver is not a 20-byte EVM address; this transfer can never be claimed \
             here and must be recovered via cancel/refund"
        );
        return Ok(None);
    }

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
    let signatures = sorted_signatures(sigs)?;

    info!(submission_id = %rec.submission_id, sigs = signatures.len(), "submitting claim()");

    let call = gate.claim(
        debridge_id,
        amount,
        U256::from(rec.chain_id_from),
        U256::from(rec.nonce),
        receiver,
        auto_params,
        native_sender,
        signatures,
    );
    confirm(call, "claim", &rec.submission_id, "CLAIMED").await.map(Some)
}

/// Send a prepared gate call, await its receipt, and refuse to report a
/// mined-but-REVERTED transaction as success.
///
/// That last part is the reason this is one helper rather than three copies. A
/// reverted `claim` reported as success makes the caller run `mark_claimed`,
/// which also clears the `eligible` refund flag — permanently hiding a stranded
/// transfer from recovery. A reverted `cancel` reported as success mis-sequences
/// the two-phase refund, and a reverted `refund` claims funds were returned when
/// they were not. Failing instead leaves the tick to retry, and each `try_*`
/// re-checks on-chain state first, so a retry after a tx really landed is a no-op.
///
/// `verb` names the call in the error; `done` is the log line on success.
async fn confirm<P: Provider>(
    call: alloy::contract::CallBuilder<P, impl alloy::contract::CallDecoder>,
    verb: &str,
    submission_id: &str,
    done: &str,
) -> anyhow::Result<String> {
    let pending = call.send().await.with_context(|| format!("send {verb}"))?;
    let receipt = pending
        .with_timeout(Some(RECEIPT_TIMEOUT))
        .get_receipt()
        .await
        .context("await receipt")?;
    if !receipt.status() {
        anyhow::bail!(
            "{verb} tx {:#x} reverted (status 0) for submission {submission_id}; \
             not recording it as successful",
            receipt.transaction_hash,
        );
    }
    info!(submission_id, tx = %receipt.transaction_hash, "{done}");
    Ok(format!("{:#x}", receipt.transaction_hash))
}

/// Submit `cancel()` on the destination. `None` if it is already executed
/// (claimed or cancelled) — either way there is nothing to burn.
///
/// `sigs` MUST already be validator-filtered — see [`try_claim`].
async fn try_cancel<P: Provider>(
    gate: &Gate::GateInstance<P>,
    rec: &SubmissionRecord,
    sigs: &[SignerSig],
) -> anyhow::Result<Option<String>> {
    let submission_id = B256::from_str(&rec.submission_id).context("bad submission_id")?;
    if gate.executed(submission_id).call().await? {
        return Ok(None);
    }

    let debridge_id = B256::from_str(&rec.debridge_id).context("bad debridge_id")?;
    let amount = U256::from_str(&rec.amount).context("bad amount")?;

    info!(
        submission_id = %rec.submission_id,
        sigs = sigs.len(),
        "submitting cancel() — burning the transfer on the destination"
    );

    let call = gate.cancel(
        debridge_id,
        amount,
        U256::from(rec.chain_id_from),
        U256::from(rec.nonce),
        bytes_of(&rec.receiver)?,
        bytes_of(&rec.auto_params)?,
        bytes_of(&rec.native_sender)?,
        sorted_signatures(sigs)?,
    );
    confirm(call, "cancel", &rec.submission_id, "CANCELLED").await.map(Some)
}

/// Submit `refund()` on the source. `None` if already refunded, if this gate
/// never emitted the id, or if we don't know which token was locked.
///
/// `sigs` MUST already be validator-filtered — see [`try_claim`].
async fn try_refund<P: Provider>(
    gate: &Gate::GateInstance<P>,
    rec: &SubmissionRecord,
    sigs: &[SignerSig],
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
        sigs = sigs.len(),
        "submitting refund() — returning locked funds on the source"
    );

    let call = gate.refund(
        token,
        debridge_id,
        amount,
        U256::from(rec.chain_id_to),
        U256::from(rec.nonce),
        bytes_of(&rec.receiver)?,
        bytes_of(&rec.auto_params)?,
        bytes_of(&rec.native_sender)?,
        sorted_signatures(sigs)?,
    );
    confirm(call, "refund", &rec.submission_id, "REFUNDED").await.map(Some)
}

/// The keeper's live view of one gate: the signature `threshold`, the
/// `validatorCount` that bounds how long a signature array may be, and a
/// per-signer membership memo.
///
/// ## Why the array must be filtered, not merely counted
///
/// The sig-store verifies only that a signature recovers to its claimed signer —
/// NOT that the signer is a validator. Anyone able to write to the store (any
/// validator, the keeper, or any holder of the shared token) can therefore
/// deposit structurally valid signatures from throwaway keys.
///
/// Counting only members was necessary but not sufficient. The keeper used to
/// count members for the quorum decision and then forward the record's ENTIRE
/// signature list as calldata. `Gate._verifySignatures` rejects any array longer
/// than `validatorCount`, so two junk signatures against a 3-validator gate made
/// every submission revert `TooManySignatures` — forever, since the off-chain
/// quorum still read as satisfied and the tick retried. The transfer became
/// permanently unclaimable.
///
/// So membership is now a FILTER, and the filtered list is the only thing that
/// ever reaches calldata. Store signers are deduplicated by `(submission_id,
/// signer)`, and members are a subset of the on-chain set, so the resulting array
/// can never exceed `validatorCount` — `TooManySignatures` is unreachable.
struct GateView {
    threshold: u64,
    /// Bounds the signature array `Gate._verifySignatures` will accept.
    validator_count: usize,
    /// Per-signer membership, refreshed on [`GATE_REFRESH_INTERVAL`].
    members: HashMap<Address, bool>,
    refreshed_at: Instant,
}

impl GateView {
    async fn load<P: Provider>(gate: &Gate::GateInstance<P>) -> anyhow::Result<Self> {
        let threshold = gate.threshold().call().await?;
        let validator_count = gate.validatorCount().call().await?;
        Ok(GateView {
            threshold: threshold.try_into().unwrap_or(u64::MAX),
            validator_count: validator_count.try_into().unwrap_or(usize::MAX),
            members: HashMap::new(),
            refreshed_at: Instant::now(),
        })
    }

    /// Re-read the gate's parameters once the memo is older than
    /// [`GATE_REFRESH_INTERVAL`], dropping the membership cache with them.
    ///
    /// A read failure leaves the previous (still usable) view in place and retries
    /// on the next tick — the alternative, treating an RPC blip as "no validators",
    /// would stall delivery entirely.
    async fn refresh_if_stale<P: Provider>(&mut self, gate: &Gate::GateInstance<P>) {
        if self.refreshed_at.elapsed() < GATE_REFRESH_INTERVAL {
            return;
        }
        match GateView::load(gate).await {
            Ok(fresh) => *self = fresh,
            Err(e) => {
                warn!(error = %e, "refreshing gate params failed; keeping the previous view");
                // Back off a full interval rather than retrying every tick.
                self.refreshed_at = Instant::now();
            }
        }
    }

    /// Those `sigs` signed by a member of the gate's on-chain validator set.
    ///
    /// Fail-closed: an `isValidator` read error drops that signer for this tick
    /// (we wait rather than submit a doomed tx) and is not cached.
    async fn member_signatures<P: Provider>(
        &mut self,
        gate: &Gate::GateInstance<P>,
        sigs: &[SignerSig],
    ) -> Vec<SignerSig> {
        let mut resolved = Vec::with_capacity(sigs.len());
        for s in sigs {
            let Ok(addr) = Address::from_str(&s.signer) else { continue };
            let member = match self.members.get(&addr) {
                Some(&m) => m,
                None => match gate.isValidator(addr).call().await {
                    Ok(m) => {
                        self.members.insert(addr, m);
                        m
                    }
                    Err(e) => {
                        warn!(signer = %s.signer, error = %e, "isValidator read failed; dropping this signer for this tick");
                        continue;
                    }
                },
            };
            resolved.push((addr, member, s));
        }
        filter_members(resolved, self.validator_count)
    }
}

/// Keep only member signatures, then cap the result at `validator_count`.
///
/// Split from the I/O so the rule is unit-testable. The cap is belt-and-braces:
/// distinct members can only exceed `validator_count` when the memo is stale
/// (a validator removed on-chain within the refresh window). Truncating after the
/// caller sorts would break the ascending-order requirement, so we cap here and
/// let [`sorted_signatures`] order what survives — a short array reverts
/// `NotEnoughSignatures`, which is retryable, rather than `TooManySignatures`,
/// which was not.
fn filter_members(
    resolved: Vec<(Address, bool, &SignerSig)>,
    validator_count: usize,
) -> Vec<SignerSig> {
    let mut kept: Vec<(Address, SignerSig)> = resolved
        .into_iter()
        .filter(|(_, member, _)| *member)
        .map(|(addr, _, s)| (addr, s.clone()))
        .collect();
    kept.sort_by(|a, b| a.0.cmp(&b.0));
    kept.truncate(validator_count);
    kept.into_iter().map(|(_, s)| s).collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A signer whose address is `byte` repeated — lets tests control ordering.
    fn sig(byte: u8) -> SignerSig {
        SignerSig {
            signer: format!("{:#x}", Address::repeat_byte(byte)),
            signature: format!("0x{}", hex::encode([byte; 65])),
        }
    }

    /// What `member_signatures` hands to `filter_members` after its RPC reads.
    fn resolved<'a>(pairs: &[(&'a SignerSig, bool)]) -> Vec<(Address, bool, &'a SignerSig)> {
        pairs
            .iter()
            .map(|(s, member)| (Address::from_str(&s.signer).unwrap(), *member, *s))
            .collect()
    }

    /// THE regression. Before the fix the keeper counted only validators toward
    /// quorum but forwarded the record's ENTIRE signature list as calldata. Two
    /// signatures from throwaway keys pushed the array past `validatorCount`, and
    /// `Gate._verifySignatures` reverted `TooManySignatures` on every retry —
    /// permanently unclaimable.
    #[test]
    fn junk_signatures_never_reach_the_calldata() {
        let (v1, v2) = (sig(0x11), sig(0x22));
        let (junk1, junk2) = (sig(0xAA), sig(0xBB));
        // A 3-validator / threshold-2 gate; the store holds 2 real + 2 forged.
        let validator_count = 3;

        // The pre-fix path: forward `rec.signatures` verbatim. Pin the defect so
        // this test fails loudly if anyone reintroduces it.
        let unfiltered = [&v1, &junk1, &v2, &junk2].len();
        assert!(
            unfiltered > validator_count,
            "premise of this regression: the unfiltered array overflows the gate's cap"
        );

        let kept = filter_members(
            resolved(&[(&v1, true), (&junk1, false), (&v2, true), (&junk2, false)]),
            validator_count,
        );

        assert_eq!(kept.len(), 2, "only validator signatures may be submitted");
        assert!(
            kept.iter().all(|s| s.signer != junk1.signer && s.signer != junk2.signer),
            "a non-validator signature reached the calldata"
        );
        // The Gate's own cap must now be unreachable from the keeper.
        assert!(
            kept.len() <= validator_count,
            "array would revert TooManySignatures at the gate"
        );
        // ...and quorum is still met, so the transfer is claimable.
        assert!(kept.len() >= 2, "junk signatures must not deny a real quorum");
    }

    /// The stale-memo guard: if validators are removed on-chain inside the refresh
    /// window, a cached `true` could otherwise build an array longer than the
    /// CURRENT `validatorCount`. Capping turns that into a retryable
    /// `NotEnoughSignatures` instead of a permanent `TooManySignatures`.
    #[test]
    fn array_is_capped_at_the_current_validator_count() {
        let sigs: Vec<SignerSig> = (1u8..=4).map(sig).collect();
        let pairs: Vec<(&SignerSig, bool)> = sigs.iter().map(|s| (s, true)).collect();

        let kept = filter_members(resolved(&pairs), 2);

        assert_eq!(kept.len(), 2, "must not exceed the gate's validatorCount");
    }

    /// The Gate requires strictly ascending signers; capping must not disturb the
    /// ordering `sorted_signatures` then relies on.
    #[test]
    fn survivors_stay_in_ascending_signer_order() {
        let (a, b, c) = (sig(0x33), sig(0x11), sig(0x22));
        let kept = filter_members(resolved(&[(&a, true), (&b, true), (&c, true)]), 3);

        let addrs: Vec<Address> =
            kept.iter().map(|s| Address::from_str(&s.signer).unwrap()).collect();
        let mut sorted = addrs.clone();
        sorted.sort();
        assert_eq!(addrs, sorted, "signers must be strictly ascending");

        // And the encoded array the gate actually receives round-trips.
        let encoded = sorted_signatures(&kept).expect("valid hex signatures");
        assert_eq!(encoded.len(), 3);
    }

    /// A record carrying nothing but forged signatures must not reach quorum.
    #[test]
    fn all_junk_yields_no_quorum() {
        let (j1, j2, j3) = (sig(0xAA), sig(0xBB), sig(0xCC));
        let kept = filter_members(resolved(&[(&j1, false), (&j2, false), (&j3, false)]), 3);
        assert!(kept.is_empty(), "no forged signature may count toward quorum");
    }
}
