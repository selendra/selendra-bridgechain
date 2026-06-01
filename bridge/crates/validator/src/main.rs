//! Minimal external-validator node (Phase 4).
//!
//! Loop: scan the source chain for `Sent` events up to the finality buffer,
//! independently recompute each `submissionId`, and — only if it matches the
//! emitted one — sign it (EIP-191 `eth_sign`) and upsert the signature into the
//! file-backed store for the keeper to collect.

mod config;

use std::path::PathBuf;
use std::time::Duration;

use alloy::primitives::{Address, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::Filter;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use alloy_sol_types::{SolEvent, SolValue};
use anyhow::Context;
use bridge_core::abi::{AutoParamsTo, Gate};
use bridge_core::store::{self, SignerSig, SubmissionRecord};
use bridge_core::{AutoParams, Submission};
use config::Config;
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

    let provider = ProviderBuilder::new().connect_http(cfg.source.rpc.parse()?);

    // chainId guard (mirrors Web3Service.validateChainId)
    let rpc_chain_id = provider.get_chain_id().await.context("get_chain_id")?;
    anyhow::ensure!(
        rpc_chain_id == cfg.source.chain_id,
        "RPC chainId {rpc_chain_id} != configured {}",
        cfg.source.chain_id
    );

    info!(
        validator = %signer_addr,
        gate = %gate,
        chain_id = cfg.source.chain_id,
        "validator started"
    );

    let mut from_block = cfg.source.start_block;
    let sent_sig = Gate::Sent::SIGNATURE_HASH;

    loop {
        let latest = provider.get_block_number().await.context("get_block_number")?;
        let confirmed = latest.saturating_sub(cfg.source.block_confirmation);

        if confirmed >= from_block {
            let to_block = confirmed.min(from_block + cfg.source.max_block_range - 1);

            let filter = Filter::new()
                .address(gate)
                .event_signature(sent_sig)
                .from_block(from_block)
                .to_block(to_block);

            let logs = provider.get_logs(&filter).await.context("get_logs")?;
            for log in logs {
                if let Err(e) = handle_log(&cfg, &signer, signer_addr, &store_dir, &log).await {
                    warn!(error = %e, "failed handling log");
                }
            }
            from_block = to_block + 1;
        }

        tokio::time::sleep(Duration::from_millis(cfg.source.poll_interval_ms)).await;
    }
}

async fn handle_log(
    cfg: &Config,
    signer: &PrivateKeySigner,
    signer_addr: Address,
    store_dir: &std::path::Path,
    log: &alloy::rpc::types::Log,
) -> anyhow::Result<()> {
    let decoded = Gate::Sent::decode_log(&log.inner).context("decode Sent")?;
    let ev = &decoded.data;

    let emitted_id: B256 = ev.submissionId;
    let submission = submission_from_event(ev);
    let computed_id = submission.compute_id();

    if computed_id != emitted_id {
        warn!(
            emitted = %emitted_id,
            computed = %computed_id,
            "submissionId MISMATCH — refusing to sign (bad/lying RPC?)"
        );
        anyhow::bail!("submissionId mismatch");
    }

    // EIP-191 eth_sign over the raw 32-byte submissionId
    let sig = signer.sign_message(emitted_id.as_slice()).await?;
    let sig_hex = encode_signature(&sig);

    let record = SubmissionRecord {
        submission_id: format!("{emitted_id:#x}"),
        debridge_id: format!("{:#x}", ev.debridgeId),
        amount: ev.amount.to_string(),
        chain_id_from: u256_to_u64(ev.chainIdFrom),
        chain_id_to: u256_to_u64(ev.chainIdTo),
        nonce: u256_to_u64(ev.nonce),
        receiver: format!("0x{}", hex::encode(&ev.receiver)),
        auto_params: format!("0x{}", hex::encode(&ev.autoParams)),
        native_sender: format!("0x{}", hex::encode(&ev.nativeSender)),
        signatures: vec![],
    };

    store::upsert_signature(
        store_dir,
        record,
        SignerSig {
            signer: format!("{signer_addr:#x}"),
            signature: sig_hex,
        },
    )?;

    info!(
        submission_id = %emitted_id,
        nonce = u256_to_u64(ev.nonce),
        chain_to = u256_to_u64(ev.chainIdTo),
        "SIGNED and stored"
    );
    let _ = cfg; // (reserved for future per-chain settings)
    Ok(())
}

/// Build our independent `Submission` from a decoded `Sent` event.
fn submission_from_event(ev: &Gate::Sent) -> Submission {
    let auto = if ev.autoParams.is_empty() {
        None
    } else {
        // decode the To-struct and attach the event's nativeSender
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
