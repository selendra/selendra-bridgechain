//! Off-chain relayer adapters for the Solana leg.
//!
//! Two directions, two adapters:
//!   * **Solana → EVM (source):** a Solana gate program emits its `Sent` event as
//!     a tagged program log; the validator's Solana source scans transaction logs
//!     for [`SENT_LOG_TAG`], parses it back into a [`Sent`], recomputes the id and
//!     signs — the same path the EVM source already runs.
//!   * **EVM → Solana (sink):** the keeper builds a Borsh `Claim` instruction
//!     ([`build_claim_instruction`]) — signatures sorted ascending by signer, as
//!     the gate requires — and submits it to the Solana program.
//!
//! The wire fields here are hex/decimal strings, matching the off-chain
//! `SubmissionRecord` shape, so a Solana-origin transfer drops straight into the
//! existing sig-store / keeper machinery.

use serde::{Deserialize, Serialize};

use crate::gate::Sent;
use crate::hash::AutoParams;
use crate::instruction::{AutoParamsWire, ClaimArgs, GateInstruction};
use crate::verify::{eth_signed_digest, recover_evm_address};

/// Prefix a Solana gate program puts on its `Sent` event log line, so the
/// validator can find it among a transaction's program logs.
pub const SENT_LOG_TAG: &str = "BRIDGE_SENT";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AutoWire {
    pub execution_fee: String,
    pub flags: String,
    pub fallback_address: String,
    pub data: String,
    pub native_sender: String,
}

/// Serde mirror of a `Sent` event (hex / decimal strings).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SentWire {
    pub submission_id: String,
    pub debridge_id: String,
    pub amount: String,
    pub chain_id_from: u64,
    pub chain_id_to: u64,
    pub receiver: String,
    pub nonce: u64,
    pub native_sender: String,
    pub auto: Option<AutoWire>,
}

fn hex0x(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}

fn unhex(s: &str) -> anyhow::Result<Vec<u8>> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.is_empty() {
        return Ok(Vec::new());
    }
    Ok(hex::decode(s)?)
}

fn to_arr32(v: &[u8]) -> anyhow::Result<[u8; 32]> {
    if v.len() != 32 {
        anyhow::bail!("expected 32 bytes, got {}", v.len());
    }
    let mut o = [0u8; 32];
    o.copy_from_slice(v);
    Ok(o)
}

impl SentWire {
    pub fn from_sent(s: &Sent) -> Self {
        SentWire {
            submission_id: hex0x(&s.submission_id),
            debridge_id: hex0x(&s.debridge_id),
            amount: s.amount.to_string(),
            chain_id_from: s.chain_id_from,
            chain_id_to: s.chain_id_to,
            receiver: hex0x(&s.receiver),
            nonce: s.nonce,
            native_sender: hex0x(&s.native_sender),
            auto: s.auto.as_ref().map(|a| AutoWire {
                execution_fee: hex0x(&a.execution_fee),
                flags: hex0x(&a.flags),
                fallback_address: hex0x(&a.fallback_address),
                data: hex0x(&a.data),
                native_sender: hex0x(&a.native_sender),
            }),
        }
    }

    pub fn to_sent(&self) -> anyhow::Result<Sent> {
        let auto = match &self.auto {
            None => None,
            Some(a) => Some(AutoParams {
                execution_fee: to_arr32(&unhex(&a.execution_fee)?)?,
                flags: to_arr32(&unhex(&a.flags)?)?,
                fallback_address: unhex(&a.fallback_address)?,
                data: unhex(&a.data)?,
                native_sender: unhex(&a.native_sender)?,
            }),
        };
        Ok(Sent {
            submission_id: to_arr32(&unhex(&self.submission_id)?)?,
            debridge_id: to_arr32(&unhex(&self.debridge_id)?)?,
            amount: self.amount.parse()?,
            chain_id_from: self.chain_id_from,
            chain_id_to: self.chain_id_to,
            receiver: unhex(&self.receiver)?,
            nonce: self.nonce,
            native_sender: unhex(&self.native_sender)?,
            auto,
        })
    }
}

/// Encode a `Sent` as the program-log line a Solana gate emits.
pub fn sent_to_log_line(sent: &Sent) -> String {
    let json = serde_json::to_string(&SentWire::from_sent(sent)).expect("serialize SentWire");
    format!("{SENT_LOG_TAG} {json}")
}

/// Parse a Solana program-log line back into a `Sent`. Returns `None` for any
/// line that isn't our tagged event (the validator skips those).
pub fn parse_sent_log_line(line: &str) -> Option<anyhow::Result<Sent>> {
    let rest = line.trim().strip_prefix(SENT_LOG_TAG)?.trim_start();
    Some((|| {
        let wire: SentWire = serde_json::from_str(rest)?;
        wire.to_sent()
    })())
}

/// Convert the hash-form auto-params into the Borsh instruction form.
fn auto_to_wire(a: &AutoParams) -> AutoParamsWire {
    let mut fee = [0u8; 16];
    fee.copy_from_slice(&a.execution_fee[16..]);
    let mut flags = [0u8; 8];
    flags.copy_from_slice(&a.flags[24..]);
    AutoParamsWire {
        execution_fee: u128::from_be_bytes(fee),
        flags: u64::from_be_bytes(flags),
        fallback_address: a.fallback_address.clone(),
        data: a.data.clone(),
    }
}

/// Build the Borsh `Claim` instruction the keeper submits to the Solana gate.
///
/// Signatures are sorted by recovered signer address strictly ascending — exactly
/// the order the gate's `verify` step requires.
pub fn build_claim_instruction(
    sent: &Sent,
    mut signatures: Vec<Vec<u8>>,
) -> anyhow::Result<Vec<u8>> {
    let digest = eth_signed_digest(&sent.submission_id);
    // Sort by recovered signer; a malformed signature sorts last deterministically.
    signatures.sort_by_key(|s| recover_evm_address(&digest, s).unwrap_or([0xff; 20]));

    let ix = GateInstruction::Claim(ClaimArgs {
        debridge_id: sent.debridge_id,
        amount: sent.amount,
        chain_id_from: sent.chain_id_from,
        nonce: sent.nonce,
        receiver: sent.receiver.clone(),
        auto: sent.auto.as_ref().map(auto_to_wire),
        native_sender: sent.native_sender.clone(),
        signatures,
    });
    Ok(ix.to_bytes())
}
