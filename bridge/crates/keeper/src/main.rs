//! Minimal keeper / executor (Phase 5).
//!
//! Loop: read the signature store, and for every record destined for our target
//! chain that has >= threshold signatures and isn't yet executed, build and
//! submit `claim()` (signatures sorted by signer ascending, as the Gate requires).

mod config;
mod source;

use std::str::FromStr;
use std::time::Duration;

use alloy::network::EthereumWallet;
use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use anyhow::Context;
use bridge_core::abi::Gate;
use bridge_core::store::SubmissionRecord;
use config::Config;
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
    let wallet = EthereumWallet::from(signer.clone());
    let gate_addr: Address = cfg.target.gate.parse().context("bad gate address")?;
    let source = Source::from_config(&cfg.store)?;

    let provider = ProviderBuilder::new()
        .wallet(wallet)
        .connect_http(cfg.target.rpc.parse()?);

    let rpc_chain_id = provider.get_chain_id().await.context("get_chain_id")?;
    anyhow::ensure!(
        rpc_chain_id == cfg.target.chain_id,
        "RPC chainId {rpc_chain_id} != configured {}",
        cfg.target.chain_id
    );

    let gate = Gate::new(gate_addr, &provider);
    let threshold: u64 = gate.threshold().call().await.context("read threshold")?.try_into().unwrap_or(u64::MAX);

    info!(
        keeper = %signer.address(),
        gate = %gate_addr,
        chain_id = cfg.target.chain_id,
        threshold,
        source = %source.describe(),
        "keeper started"
    );

    loop {
        let records = source.load_all().await.unwrap_or_default();
        for rec in records {
            if rec.chain_id_to != cfg.target.chain_id {
                continue;
            }
            if (rec.signatures.len() as u64) < threshold {
                continue;
            }
            if let Err(e) = try_claim(&gate, &rec).await {
                warn!(submission_id = %rec.submission_id, error = %e, "claim failed");
            }
        }
        tokio::time::sleep(Duration::from_millis(cfg.target.poll_interval_ms)).await;
    }
}

async fn try_claim<P: Provider>(gate: &Gate::GateInstance<P>, rec: &SubmissionRecord) -> anyhow::Result<()> {
    let submission_id = B256::from_str(&rec.submission_id).context("bad submission_id")?;

    if gate.executed(submission_id).call().await? {
        return Ok(()); // already executed (by us or another keeper)
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
    Ok(())
}

fn bytes_of(hex_str: &str) -> anyhow::Result<Bytes> {
    let s = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    if s.is_empty() {
        return Ok(Bytes::new());
    }
    Ok(Bytes::from(hex::decode(s)?))
}
