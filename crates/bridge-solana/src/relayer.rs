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

use base64::Engine as _;
use borsh::{BorshDeserialize, BorshSerialize};

use crate::gate::Sent;
use crate::hash::AutoParams;
use crate::instruction::AutoParamsWire;
#[cfg(feature = "recover")]
use crate::instruction::{ClaimArgs, GateInstruction};
#[cfg(feature = "recover")]
use crate::verify::{eth_signed_digest, recover_evm_address};

/// First field of the Solana gate's `Sent` program-data event — lets the
/// validator's Solana source pick our event out of a transaction's program data.
/// Kept as the historical `BRIDGE_SENT` bytes so existing tooling recognises it.
pub const SENT_EVENT_TAG: &[u8] = b"BRIDGE_SENT";

/// Wire-format version of [`SentEvent`]. Bump on any layout change so a decoder
/// can reject an event it wasn't built to read instead of silently misreading it.
pub const SENT_EVENT_VERSION: u8 = 1;

/// The line prefix Solana's runtime prints for `sol_log_data(...)` — the program
/// emits its `Sent` event through that syscall, so this is where we find it.
pub const PROGRAM_DATA_PREFIX: &str = "Program data:";

/// Back-compat alias for the human-readable tag (the string form of
/// [`SENT_EVENT_TAG`]). Retained so older references keep resolving.
pub const SENT_LOG_TAG: &str = "BRIDGE_SENT";

/// The single, versioned on-chain `Sent` event — finding H5.
///
/// PROBLEM (H5): the deployed program emitted `sol_log_data([b"BRIDGE_SENT", id
/// || debridge_id])` (two hash-bound fields, binary), while the off-chain relayer
/// looked for a *text* line `BRIDGE_SENT {json}` carrying the *whole* transfer.
/// Neither the framing nor the field set matched, so Solana→EVM scanning could
/// never work and a validator could not reconstruct the submissionId.
///
/// FIX: one Borsh struct, versioned, carrying every field the submissionId hashes
/// over **plus the locked asset identity** (`mint`), emitted via `sol_log_data`
/// and decoded from the real `Program data:` base64 line. The program
/// (`crates/solana-gate`) mirrors this exact layout byte-for-byte; the round-trip
/// through the genuine log framing is tested here (see `tests`/`e2e.rs`).
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct SentEvent {
    /// [`SENT_EVENT_VERSION`]; a decoder rejects anything it doesn't understand.
    pub version: u8,
    pub submission_id: [u8; 32],
    pub debridge_id: [u8; 32],
    /// The locked SPL mint on the Solana side — the asset identity bound to the
    /// transfer (a validator/relayer can check it against the debridgeId).
    pub mint: [u8; 32],
    pub amount: u64,
    pub chain_id_from: u64,
    pub chain_id_to: u64,
    pub nonce: u64,
    pub receiver: Vec<u8>,
    pub native_sender: Vec<u8>,
    pub auto: Option<AutoParamsWire>,
}

impl SentEvent {
    /// Build the event a source-side `send` emits. `mint` is the locked SPL mint.
    pub fn from_sent(s: &Sent, mint: [u8; 32]) -> Self {
        SentEvent {
            version: SENT_EVENT_VERSION,
            submission_id: s.submission_id,
            debridge_id: s.debridge_id,
            mint,
            amount: s.amount,
            chain_id_from: s.chain_id_from,
            chain_id_to: s.chain_id_to,
            nonce: s.nonce,
            receiver: s.receiver.clone(),
            native_sender: s.native_sender.clone(),
            auto: s.auto.as_ref().map(auto_to_wire),
        }
    }

    /// Reconstruct the `Sent` the validator/keeper machinery consumes. The
    /// auto-params' `native_sender` is the event's single `native_sender` — the
    /// same value the submissionId hashes over — so a recompute matches.
    pub fn to_sent(&self) -> anyhow::Result<Sent> {
        if self.version != SENT_EVENT_VERSION {
            anyhow::bail!(
                "unsupported SentEvent version {} (expected {})",
                self.version,
                SENT_EVENT_VERSION
            );
        }
        let auto = self.auto.as_ref().map(|w| wire_to_auto(w, &self.native_sender));
        Ok(Sent {
            submission_id: self.submission_id,
            debridge_id: self.debridge_id,
            amount: self.amount,
            chain_id_from: self.chain_id_from,
            chain_id_to: self.chain_id_to,
            receiver: self.receiver.clone(),
            nonce: self.nonce,
            native_sender: self.native_sender.clone(),
            auto,
        })
    }

    /// Borsh bytes of this event — the second field the program passes to
    /// `sol_log_data`. The program serializes the identical layout.
    pub fn to_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("borsh serialize SentEvent")
    }

    pub fn try_from_bytes(b: &[u8]) -> anyhow::Result<Self> {
        Ok(borsh::from_slice(b)?)
    }
}

/// Encode a `SentEvent` exactly as it appears in a Solana transaction's logs:
/// `Program data: <base64(tag)> <base64(borsh(event))>`. This is the string the
/// runtime prints for `sol_log_data(&[SENT_EVENT_TAG, &event.to_bytes()])`, so a
/// test can feed it straight back through [`parse_sent_event_line`].
pub fn sent_event_to_program_data_line(event: &SentEvent) -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    format!(
        "{PROGRAM_DATA_PREFIX} {} {}",
        b64.encode(SENT_EVENT_TAG),
        b64.encode(event.to_bytes())
    )
}

/// Parse a Solana `Program data:` log line into a [`SentEvent`]. Returns `None`
/// for any line that isn't a tagged bridge event (the validator skips those);
/// `Some(Err(..))` if it's ours but malformed (a fault the caller must surface,
/// per finding H3 — never a silent skip).
pub fn parse_sent_event_line(line: &str) -> Option<anyhow::Result<SentEvent>> {
    let rest = line.trim().strip_prefix(PROGRAM_DATA_PREFIX)?.trim();
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut fields = rest.split_whitespace();
    let tag_b64 = fields.next()?;
    // Decode the first field; only claim this line if it's our tag.
    let tag = b64.decode(tag_b64).ok()?;
    if tag != SENT_EVENT_TAG {
        return None;
    }
    Some((|| {
        let payload_b64 = fields
            .next()
            .ok_or_else(|| anyhow::anyhow!("BRIDGE_SENT program data missing its payload field"))?;
        let bytes = b64.decode(payload_b64)?;
        SentEvent::try_from_bytes(&bytes)
    })())
}

/// Parse a Solana program-log line back into a `Sent`. Returns `None` for any
/// line that isn't our tagged event (the validator skips those); `Some(Err(..))`
/// if it is ours but won't decode.
pub fn parse_sent_log_line(line: &str) -> Option<anyhow::Result<Sent>> {
    match parse_sent_event_line(line)? {
        Ok(ev) => Some(ev.to_sent()),
        Err(e) => Some(Err(e)),
    }
}

/// Convert the hash-form auto-params into the Borsh instruction/event form.
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

/// Inverse of [`auto_to_wire`]: widen the Borsh scalars back to the 32-byte hash
/// words and attach `native_sender` (the field the submissionId hashes over).
fn wire_to_auto(w: &AutoParamsWire, native_sender: &[u8]) -> AutoParams {
    let mut execution_fee = [0u8; 32];
    execution_fee[16..].copy_from_slice(&w.execution_fee.to_be_bytes());
    let mut flags = [0u8; 32];
    flags[24..].copy_from_slice(&w.flags.to_be_bytes());
    AutoParams {
        execution_fee,
        flags,
        fallback_address: w.fallback_address.clone(),
        data: w.data.clone(),
        native_sender: native_sender.to_vec(),
    }
}

/// Build the Borsh `Claim` instruction the keeper submits to the Solana gate.
///
/// Signatures are sorted by recovered signer address strictly ascending — exactly
/// the order the gate's `verify` step requires.
#[cfg(feature = "recover")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::AutoParams;

    fn sample_sent(auto: Option<AutoParams>) -> Sent {
        Sent {
            submission_id: [0x11; 32],
            debridge_id: [0x22; 32],
            amount: 42_000,
            chain_id_from: crate::SOLANA_CHAIN_ID,
            chain_id_to: 1337,
            receiver: vec![0xEE; 20],
            nonce: 7,
            native_sender: vec![0x33; 32],
            auto,
        }
    }

    // H5: the exact `Program data:` line the runtime prints for
    // `sol_log_data([tag, borsh])` must round-trip back to the same Sent, both
    // with and without auto-params, and must carry the locked mint.
    #[test]
    fn program_data_line_round_trips() {
        for auto in [
            None,
            Some(AutoParams {
                execution_fee: {
                    let mut f = [0u8; 32];
                    f[16..].copy_from_slice(&1_234u128.to_be_bytes());
                    f
                },
                flags: {
                    let mut f = [0u8; 32];
                    f[24..].copy_from_slice(&5u64.to_be_bytes());
                    f
                },
                fallback_address: vec![0xAB; 20],
                data: vec![1, 2, 3, 4],
                native_sender: vec![0x33; 32],
            }),
        ] {
            let sent = sample_sent(auto);
            let mint = [0x55u8; 32];
            let line = sent_event_to_program_data_line(&SentEvent::from_sent(&sent, mint));
            assert!(line.starts_with("Program data: "), "must use sol_log_data framing");

            let event = parse_sent_event_line(&line).expect("our line").expect("decodes");
            assert_eq!(event.version, SENT_EVENT_VERSION);
            assert_eq!(event.mint, mint, "locked asset identity must survive");

            let back = parse_sent_log_line(&line).expect("our line").expect("decodes");
            assert_eq!(back, sent, "round-trip must be lossless");
        }
    }

    // Lines that aren't our tagged program data are skipped (None), so the
    // validator's scan ignores unrelated program output.
    #[test]
    fn non_bridge_lines_are_skipped() {
        for line in [
            "Program log: instruction: Send",
            "Program consumed 4242 compute units",
            "Program data: aGVsbG8=", // base64("hello"), not our tag
            "Program invoke [1]",
            "",
        ] {
            assert!(parse_sent_log_line(line).is_none(), "should skip: {line:?}");
            assert!(parse_sent_event_line(line).is_none(), "should skip: {line:?}");
        }
    }

    // A tagged-but-malformed payload is a hard error (Some(Err)), never a silent
    // skip — a corrupt event must surface, not vanish (finding H3 posture).
    #[test]
    fn tagged_but_malformed_payload_errors() {
        let b64 = base64::engine::general_purpose::STANDARD;
        // Correct tag, but the payload isn't valid Borsh for SentEvent.
        let line = format!(
            "Program data: {} {}",
            b64.encode(SENT_EVENT_TAG),
            b64.encode([0xFFu8; 3])
        );
        let r = parse_sent_event_line(&line).expect("recognised as ours");
        assert!(r.is_err(), "malformed payload must error, not skip");
    }

    // A future version the decoder wasn't built for is rejected rather than
    // silently misread.
    #[test]
    fn unknown_version_is_rejected() {
        let mut ev = SentEvent::from_sent(&sample_sent(None), [0x55; 32]);
        ev.version = SENT_EVENT_VERSION + 1;
        let line = sent_event_to_program_data_line(&ev);
        // The bytes decode into a SentEvent, but converting to a Sent rejects it.
        let ev2 = parse_sent_event_line(&line).unwrap().unwrap();
        assert!(ev2.to_sent().is_err(), "unknown version must be rejected");
    }
}
