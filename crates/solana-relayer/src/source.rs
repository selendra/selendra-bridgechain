//! The Solana source scanner: the missing runner from finding M-3.
//!
//! Does exactly what the EVM validator's `scan_source` does, against a different
//! VM: poll for the gate's transactions, decode each `Sent` event, INDEPENDENTLY
//! recompute the submissionId, sign it only if it matches, and store the
//! signature. It signs with the same secp256k1 key the validator uses on the EVM
//! side, so one validator set attests for both chains.
//!
//! The two safety rules carried over from the EVM scanner:
//!   * **finality** — read only at `finalized`, so a fork cannot discard a `Sent`
//!     after the destination has paid out (enforced in [`crate::config`]);
//!   * **never sign what you cannot reproduce** — a mismatch between the emitted
//!     and recomputed id means a lying RPC or a divergent program, and is a hard
//!     stop rather than a skip.

use std::str::FromStr;
use std::time::Duration;

use bridge_solana::gate::Sent;
use bridge_solana::hash::{amount_word, submission_id, submission_id_with_auto};
use bridge_solana::relayer::parse_sent_log_line;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_transaction_status::UiTransactionEncoding;
use tracing::{info, warn};

use crate::config::SourceChain;
use crate::state::Cursor;
use crate::store::{SignerSig, Store, SubmissionRecord};

/// Recompute a submissionId from a decoded event, exactly as the program did.
///
/// This is THE check: we sign the id we derived, never the one we were handed.
fn recompute(sent: &Sent) -> [u8; 32] {
    match sent.auto.as_ref() {
        None => submission_id(
            &sent.debridge_id,
            &amount_word(sent.amount as u128),
            sent.chain_id_from,
            sent.chain_id_to,
            sent.nonce,
            &sent.receiver,
        ),
        Some(auto) => submission_id_with_auto(
            &sent.debridge_id,
            &amount_word(sent.amount as u128),
            sent.chain_id_from,
            sent.chain_id_to,
            sent.nonce,
            &sent.receiver,
            auto,
        ),
    }
}

/// 65-byte r||s||v EIP-191 signature over `id`, matching what the EVM validator
/// produces and what `Gate._verifySignatures` accepts.
fn sign(secret: &libsecp256k1::SecretKey, id: &[u8; 32]) -> String {
    let digest = bridge_solana::verify::eth_signed_digest(id);
    let (sig, recid) = libsecp256k1::sign(&libsecp256k1::Message::parse(&digest), secret);
    let mut out = sig.serialize().to_vec();
    out.push(recid.serialize() + 27);
    format!("0x{}", hex::encode(out))
}

/// The signer's EVM address — `keccak(uncompressed_pubkey[1..])[12..]`.
fn evm_address(secret: &libsecp256k1::SecretKey) -> String {
    let public = libsecp256k1::PublicKey::from_secret_key(secret);
    let hash = bridge_solana::hash::keccak(&public.serialize()[1..]);
    format!("0x{}", hex::encode(&hash[12..]))
}

pub struct Scanner {
    rpc: RpcClient,
    program_id: Pubkey,
    cfg: SourceChain,
    secret: libsecp256k1::SecretKey,
    signer_address: String,
    store: Store,
    cursor: Cursor,
}

impl Scanner {
    pub fn new(
        cfg: SourceChain,
        secret_key: [u8; 32],
        store: Store,
    ) -> anyhow::Result<Self> {
        let commitment = match cfg.commitment.as_str() {
            "finalized" => CommitmentConfig::finalized(),
            "confirmed" => CommitmentConfig::confirmed(),
            _ => CommitmentConfig::processed(),
        };
        let secret = libsecp256k1::SecretKey::parse(&secret_key)
            .map_err(|_| anyhow::anyhow!("signer key is not a valid secp256k1 scalar"))?;
        let signer_address = evm_address(&secret);
        let cursor = Cursor::load_or_init(&cfg.state_file)?;
        Ok(Scanner {
            rpc: RpcClient::new_with_commitment(cfg.rpc.clone(), commitment),
            program_id: Pubkey::from_str(&cfg.program_id)
                .map_err(|_| anyhow::anyhow!("program_id is not a valid pubkey"))?,
            cfg,
            secret,
            signer_address,
            store,
            cursor,
        })
    }

    pub fn signer_address(&self) -> &str {
        &self.signer_address
    }

    /// Poll forever. Transient RPC failures back off and retry rather than kill
    /// the loop; a batch that fails to store leaves the cursor put so the same
    /// range is re-scanned (the store's upsert is idempotent).
    pub async fn run(mut self) -> anyhow::Result<()> {
        let retry = Duration::from_millis(self.cfg.poll_interval_ms.max(500));
        info!(
            validator = %self.signer_address,
            program = %self.program_id,
            commitment = %self.cfg.commitment,
            resume_after = ?self.cursor.last_signature,
            "solana source scanner started"
        );

        loop {
            match self.tick().await {
                Ok(n) if n > 0 => info!(processed = n, "handled Sent events"),
                Ok(_) => {}
                Err(e) => warn!(error = %e, "scan tick failed; retrying"),
            }
            tokio::time::sleep(retry).await;
        }
    }

    async fn tick(&mut self) -> anyhow::Result<usize> {
        let until = self
            .cursor
            .last_signature
            .as_deref()
            .and_then(|s| Signature::from_str(s).ok());

        let sigs = self
            .rpc
            .get_signatures_for_address_with_config(
                &self.program_id,
                GetConfirmedSignaturesForAddress2Config {
                    until,
                    limit: Some(self.cfg.max_batch),
                    commitment: Some(self.rpc.commitment()),
                    ..Default::default()
                },
            )
            .await?;

        if sigs.is_empty() {
            return Ok(0);
        }

        // The RPC returns newest-first; process oldest-first so the nonce sequence
        // and the cursor advance monotonically.
        let mut handled = 0usize;
        for entry in sigs.iter().rev() {
            if entry.err.is_some() {
                continue; // a failed tx emitted no committed event
            }
            let signature = Signature::from_str(&entry.signature)?;
            let tx = self
                .rpc
                .get_transaction_with_config(
                    &signature,
                    solana_client::rpc_config::RpcTransactionConfig {
                        encoding: Some(UiTransactionEncoding::Json),
                        commitment: Some(self.rpc.commitment()),
                        max_supported_transaction_version: Some(0),
                    },
                )
                .await?;

            let logs = tx
                .transaction
                .meta
                .as_ref()
                .and_then(|m| Option::<Vec<String>>::from(m.log_messages.clone()))
                .unwrap_or_default();

            for line in &logs {
                match parse_sent_log_line(line) {
                    None => continue, // not our event
                    // A tagged-but-malformed payload is a fault, not noise: surface
                    // it and leave the cursor put rather than silently skipping a
                    // transfer (the H3 posture).
                    Some(Err(e)) => {
                        anyhow::bail!("malformed BRIDGE_SENT in tx {}: {e}", entry.signature)
                    }
                    Some(Ok(sent)) => {
                        self.handle(&sent, &entry.signature).await?;
                        handled += 1;
                    }
                }
            }

            // Advance only after the whole transaction is durably handled.
            self.cursor.last_signature = Some(entry.signature.clone());
            self.cursor.save(&self.cfg.state_file)?;
        }
        Ok(handled)
    }

    async fn handle(&self, sent: &Sent, tx: &str) -> anyhow::Result<()> {
        // Never sign an id we cannot reproduce ourselves.
        let computed = recompute(sent);
        if computed != sent.submission_id {
            anyhow::bail!(
                "submissionId MISMATCH in tx {tx}: emitted {} computed {} — refusing to sign",
                hex::encode(sent.submission_id),
                hex::encode(computed)
            );
        }
        // The gate binds its own chain id into the hash; if it disagrees with our
        // config we are pointed at the wrong program or the wrong cluster.
        if sent.chain_id_from != self.cfg.chain_id {
            anyhow::bail!(
                "event chain_id_from {} != configured {} — refusing to sign",
                sent.chain_id_from,
                self.cfg.chain_id
            );
        }

        let record = SubmissionRecord {
            submission_id: format!("0x{}", hex::encode(sent.submission_id)),
            debridge_id: format!("0x{}", hex::encode(sent.debridge_id)),
            amount: sent.amount.to_string(),
            chain_id_from: sent.chain_id_from,
            chain_id_to: sent.chain_id_to,
            nonce: sent.nonce,
            receiver: format!("0x{}", hex::encode(&sent.receiver)),
            // Solana auto-params ride in the id, not as EVM-encoded bytes; the
            // store treats an empty string as "no payload".
            auto_params: "0x".to_string(),
            native_sender: format!("0x{}", hex::encode(&sent.native_sender)),
            // `token` is the EVM-side ERC-20 for the refund relayer. A Solana
            // transfer's asset is the SPL mint, which does not hash to the same
            // debridgeId formula, so it is deliberately left empty rather than
            // filled with something the store would reject.
            token: String::new(),
            signatures: vec![SignerSig {
                signer: self.signer_address.clone(),
                signature: sign(&self.secret, &sent.submission_id),
            }],
            cancel_signatures: vec![],
            refund_signatures: vec![],
        };

        self.store.upsert(&record).await?;
        info!(
            submission_id = %record.submission_id,
            nonce = sent.nonce,
            chain_to = sent.chain_id_to,
            "SIGNED and stored"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_solana::relayer::{sent_event_to_program_data_line, SentEvent};

    fn sample() -> Sent {
        let mut s = Sent {
            submission_id: [0u8; 32],
            debridge_id: [0x22; 32],
            amount: 42_000,
            chain_id_from: 7565164,
            chain_id_to: 1337,
            receiver: vec![0xEE; 20],
            nonce: 7,
            native_sender: vec![0x33; 32],
            auto: None,
        };
        s.submission_id = recompute(&s);
        s
    }

    /// The scanner must derive the same id the program did — otherwise every
    /// signature it produces is for a submission no gate will ever recognise.
    #[test]
    fn recompute_round_trips_through_the_real_log_framing() {
        let sent = sample();
        let line = sent_event_to_program_data_line(&SentEvent::from_sent(&sent, [0x55; 32]));
        let parsed = parse_sent_log_line(&line).expect("our line").expect("decodes");
        assert_eq!(recompute(&parsed), sent.submission_id, "id must survive the round trip");
    }

    /// A tampered event must not reproduce its claimed id — this is the check that
    /// stops a lying RPC getting a signature over params nobody sent.
    #[test]
    fn a_tampered_event_fails_recomputation() {
        let mut sent = sample();
        sent.amount += 1; // the classic: inflate the payout
        assert_ne!(recompute(&sent), sent.submission_id, "tampering must be detectable");
    }

    /// The signature must recover to the address we claim, or the store rejects it.
    #[test]
    fn signature_recovers_to_the_claimed_signer() {
        let secret = libsecp256k1::SecretKey::parse(&[7u8; 32]).unwrap();
        let id = [0x11u8; 32];
        let sig_hex = sign(&secret, &id);
        let raw = hex::decode(sig_hex.trim_start_matches("0x")).unwrap();

        let digest = bridge_solana::verify::eth_signed_digest(&id);
        let recovered = libsecp256k1::recover(
            &libsecp256k1::Message::parse(&digest),
            &libsecp256k1::Signature::parse_standard_slice(&raw[..64]).unwrap(),
            &libsecp256k1::RecoveryId::parse(raw[64] - 27).unwrap(),
        )
        .unwrap();
        let hash = bridge_solana::hash::keccak(&recovered.serialize()[1..]);
        assert_eq!(format!("0x{}", hex::encode(&hash[12..])), evm_address(&secret));
    }
}
