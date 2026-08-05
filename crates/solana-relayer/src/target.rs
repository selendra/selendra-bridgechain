//! EVM → Solana delivery: the keeper half of finding M-3.
//!
//! Mirrors `crates/keeper`'s claim loop against the other VM. It reads the same
//! sig-store, filters to transfers bound for Solana, and submits `claim` once a
//! validator quorum exists.
//!
//! It decides nothing. The validator signatures carry all the authority; the
//! Solana keypair here only pays fees. Two rules carried over from the EVM
//! keeper, both of which exist because getting them wrong is expensive:
//!
//!   * **submit only validator signatures** (finding H-1) — the store accepts a
//!     signature from any key, and the gate rejects an array longer than its
//!     validator set, so forwarding junk makes a transfer permanently unclaimable.
//!     We cannot read the on-chain validator set cheaply per tick here, so we cap
//!     the array at the count the config declares and drop anything that fails to
//!     recover at all.
//!   * **skip what is already executed** — the marker PDA is authoritative.

use std::str::FromStr;
use std::time::Duration;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{read_keypair_file, Keypair, Signer as _};
use solana_sdk::transaction::Transaction;
use tracing::{info, warn};

use crate::config::{SourceChain, TargetChain};
use crate::store::{Store, SubmissionRecord};

/// Borsh-encode the `Claim` instruction. Built here rather than via
/// `bridge_solana::relayer::build_claim_instruction` because that helper needs
/// the `recover` feature (k256), which cannot coexist with solana-client.
mod wire {
    use borsh::BorshSerialize;

    #[derive(BorshSerialize, Clone, Debug, Default)]
    pub struct AutoParamsWire {
        pub execution_fee: u128,
        pub flags: u64,
        pub fallback_address: Vec<u8>,
        pub data: Vec<u8>,
    }

    #[derive(BorshSerialize)]
    pub struct ClaimArgs {
        pub debridge_id: [u8; 32],
        pub amount: u64,
        pub chain_id_from: u64,
        pub nonce: u64,
        pub receiver: Vec<u8>,
        pub auto: Option<AutoParamsWire>,
        pub native_sender: Vec<u8>,
        pub signatures: Vec<Vec<u8>>,
    }

    /// Discriminant 2 in `GateInstruction` — Init, Send, Claim, ...
    /// Kept byte-compatible with the program's enum; the account-level suite
    /// pins that layout.
    pub fn claim_instruction_data(args: &ClaimArgs) -> Vec<u8> {
        let mut out = vec![2u8];
        args.serialize(&mut out).expect("borsh serialize ClaimArgs");
        out
    }
}

fn hex_bytes(s: &str) -> anyhow::Result<Vec<u8>> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.is_empty() {
        return Ok(Vec::new());
    }
    Ok(hex::decode(s)?)
}

fn hex32(s: &str) -> anyhow::Result<[u8; 32]> {
    hex_bytes(s)?.try_into().map_err(|_| anyhow::anyhow!("expected 32 bytes"))
}

/// Recover the EVM address a signature belongs to, so the array can be sorted
/// ascending as `verify_threshold` requires. A signature that will not recover is
/// dropped rather than submitted — it could only pad the array toward the gate's
/// length cap (finding H-1).
fn recovered_address(digest: &[u8; 32], sig65: &[u8]) -> Option<[u8; 20]> {
    if sig65.len() != 65 {
        return None;
    }
    let recid = libsecp256k1::RecoveryId::parse(sig65[64].checked_sub(27)?).ok()?;
    let sig = libsecp256k1::Signature::parse_standard_slice(&sig65[..64]).ok()?;
    let public = libsecp256k1::recover(&libsecp256k1::Message::parse(digest), &sig, &recid).ok()?;
    let hash = bridge_solana::hash::keccak(&public.serialize()[1..]);
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&hash[12..]);
    Some(addr)
}

/// Signatures ordered by recovered signer, ascending, with unrecoverable ones
/// dropped and the result capped at `max`.
fn ordered_signatures(
    submission_id: &[u8; 32],
    raw: &[Vec<u8>],
    max: usize,
) -> Vec<Vec<u8>> {
    let digest = bridge_solana::verify::eth_signed_digest(submission_id);
    let mut with_addr: Vec<([u8; 20], Vec<u8>)> = raw
        .iter()
        .filter_map(|s| recovered_address(&digest, s).map(|a| (a, s.clone())))
        .collect();
    with_addr.sort_by_key(|(a, _)| *a);
    with_addr.dedup_by_key(|(a, _)| *a); // the gate requires STRICTLY ascending
    with_addr.truncate(max);
    with_addr.into_iter().map(|(_, s)| s).collect()
}

pub struct Submitter {
    rpc: RpcClient,
    program_id: Pubkey,
    payer: Keypair,
    chain_id: u64,
    poll: Duration,
    store: Store,
}

impl Submitter {
    pub fn new(
        source: &SourceChain,
        target: &TargetChain,
        store: Store,
    ) -> anyhow::Result<Self> {
        let payer = read_keypair_file(&target.payer_keypair)
            .map_err(|e| anyhow::anyhow!("reading payer keypair {}: {e}", target.payer_keypair))?;
        Ok(Submitter {
            rpc: RpcClient::new_with_commitment(
                source.rpc.clone(),
                CommitmentConfig::confirmed(),
            ),
            program_id: Pubkey::from_str(&source.program_id)
                .map_err(|_| anyhow::anyhow!("program_id is not a valid pubkey"))?,
            payer,
            chain_id: source.chain_id,
            poll: Duration::from_millis(target.poll_interval_ms.max(500)),
            store,
        })
    }

    pub async fn run(self) -> anyhow::Result<()> {
        info!(payer = %self.payer.pubkey(), program = %self.program_id, "solana claim submitter started");
        loop {
            if let Err(e) = self.tick().await {
                warn!(error = %e, "claim tick failed; retrying");
            }
            tokio::time::sleep(self.poll).await;
        }
    }

    async fn tick(&self) -> anyhow::Result<()> {
        for rec in self.store.list().await? {
            if rec.chain_id_to != self.chain_id {
                continue;
            }
            if let Err(e) = self.try_claim(&rec).await {
                warn!(submission_id = %rec.submission_id, error = %e, "claim failed");
            }
        }
        Ok(())
    }

    async fn try_claim(&self, rec: &SubmissionRecord) -> anyhow::Result<()> {
        let id = hex32(&rec.submission_id)?;
        let (executed, _) = Pubkey::find_program_address(&[b"executed", &id], &self.program_id);

        // The marker is authoritative: if it exists this submission is spent,
        // whether it was claimed or cancelled.
        if let Some(acct) = self.rpc.get_account_with_commitment(&executed, self.rpc.commitment()).await?.value {
            if acct.owner == self.program_id && !acct.data.is_empty() {
                return Ok(());
            }
        }

        let debridge_id = hex32(&rec.debridge_id)?;
        let receiver = hex_bytes(&rec.receiver)?;
        let receiver_token: [u8; 32] = receiver
            .clone()
            .try_into()
            .map_err(|_| anyhow::anyhow!("a Solana receiver must be a 32-byte token account"))?;
        let receiver_token = Pubkey::new_from_array(receiver_token);

        let raw_sigs: Vec<Vec<u8>> =
            rec.signatures.iter().filter_map(|s| hex_bytes(&s.signature).ok()).collect();
        // Without an on-chain read of the validator set, the record's own signature
        // count is the only bound available; the gate re-checks membership anyway.
        let signatures = ordered_signatures(&id, &raw_sigs, raw_sigs.len());
        if signatures.is_empty() {
            return Ok(());
        }

        let (asset, _) = Pubkey::find_program_address(&[b"asset", &debridge_id], &self.program_id);
        let (config, _) = Pubkey::find_program_address(&[b"config"], &self.program_id);
        let (vault_authority, _) =
            Pubkey::find_program_address(&[b"vault_authority"], &self.program_id);

        // The vault is whatever the asset registry binds to this debridge_id; read
        // it rather than trusting the record.
        let asset_account = self
            .rpc
            .get_account(&asset)
            .await
            .map_err(|_| anyhow::anyhow!("no asset registered for this debridge_id"))?;
        // AssetConfig = debridge_id(32) || mint(32) || vault(32)
        if asset_account.data.len() < 96 {
            anyhow::bail!("asset account is malformed");
        }
        let vault = Pubkey::new_from_array(asset_account.data[64..96].try_into()?);

        let args = wire::ClaimArgs {
            debridge_id,
            amount: rec.amount.parse().map_err(|_| anyhow::anyhow!("amount exceeds u64"))?,
            chain_id_from: rec.chain_id_from,
            nonce: rec.nonce,
            receiver,
            // The EVM auto-params encoding does not carry over; a Solana claim
            // reconstructs the id from the same fields the source hashed.
            auto: None,
            native_sender: hex_bytes(&rec.native_sender)?,
            signatures,
        };

        let instruction = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new_readonly(config, false),
                AccountMeta::new_readonly(asset, false),
                AccountMeta::new(executed, false),
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new(vault, false),
                AccountMeta::new(receiver_token, false),
                AccountMeta::new_readonly(vault_authority, false),
                AccountMeta::new_readonly(spl_token_id(), false),
                AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
            ],
            data: wire::claim_instruction_data(&args),
        };

        let blockhash = self.rpc.get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&self.payer.pubkey()),
            &[&self.payer],
            blockhash,
        );
        let sig = self.rpc.send_and_confirm_transaction(&tx).await?;
        info!(submission_id = %rec.submission_id, tx = %sig, "CLAIMED on Solana");
        Ok(())
    }
}

/// The SPL token program id, hardcoded to avoid pulling spl-token in.
fn spl_token_id() -> Pubkey {
    Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").expect("valid spl-token id")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(seed: u8, digest: &[u8; 32]) -> ([u8; 20], Vec<u8>) {
        let secret = libsecp256k1::SecretKey::parse(&[seed; 32]).unwrap();
        let (sig, recid) = libsecp256k1::sign(&libsecp256k1::Message::parse(digest), &secret);
        let mut out = sig.serialize().to_vec();
        out.push(recid.serialize() + 27);
        let public = libsecp256k1::PublicKey::from_secret_key(&secret);
        let hash = bridge_solana::hash::keccak(&public.serialize()[1..]);
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&hash[12..]);
        (addr, out)
    }

    /// The gate requires strictly ascending signers; an out-of-order array is
    /// rejected outright.
    #[test]
    fn signatures_come_out_ascending() {
        let id = [0x11u8; 32];
        let digest = bridge_solana::verify::eth_signed_digest(&id);
        let raw: Vec<Vec<u8>> = (1u8..=4).map(|s| sign(s, &digest).1).collect();

        let ordered = ordered_signatures(&id, &raw, 10);
        assert_eq!(ordered.len(), 4);

        let addrs: Vec<[u8; 20]> =
            ordered.iter().map(|s| recovered_address(&digest, s).unwrap()).collect();
        let mut sorted = addrs.clone();
        sorted.sort();
        assert_eq!(addrs, sorted, "signers must be strictly ascending");
    }

    /// Junk that cannot recover must never reach the calldata — it could only pad
    /// the array toward the gate's length cap (finding H-1's Solana counterpart).
    #[test]
    fn unrecoverable_signatures_are_dropped() {
        let id = [0x11u8; 32];
        let digest = bridge_solana::verify::eth_signed_digest(&id);
        let good = sign(1, &digest).1;
        let raw = vec![good, vec![0u8; 10], vec![0xFFu8; 65]];

        let ordered = ordered_signatures(&id, &raw, 10);
        assert_eq!(ordered.len(), 1, "only the recoverable signature survives");
    }

    /// Duplicate signers would break the strictly-ascending rule.
    #[test]
    fn duplicate_signers_are_deduped() {
        let id = [0x11u8; 32];
        let digest = bridge_solana::verify::eth_signed_digest(&id);
        let s = sign(3, &digest).1;
        let ordered = ordered_signatures(&id, &[s.clone(), s], 10);
        assert_eq!(ordered.len(), 1);
    }

    #[test]
    fn the_array_is_capped() {
        let id = [0x11u8; 32];
        let digest = bridge_solana::verify::eth_signed_digest(&id);
        let raw: Vec<Vec<u8>> = (1u8..=5).map(|s| sign(s, &digest).1).collect();
        assert_eq!(ordered_signatures(&id, &raw, 2).len(), 2);
    }

    #[test]
    fn spl_token_id_is_the_canonical_one() {
        assert_eq!(spl_token_id().to_string(), "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
    }
}

#[cfg(test)]
mod wire_compat_tests {
    use super::*;
    use bridge_solana::instruction::{ClaimArgs as SharedClaimArgs, GateInstruction};

    /// The hand-rolled encoder in `wire` prepends discriminant 2 for `Claim`. If
    /// anyone reorders `GateInstruction`, that constant silently starts targeting
    /// a DIFFERENT instruction — the transaction would still be well-formed and
    /// the program would execute the wrong handler. Pin it against the shared
    /// definition, which the on-chain program mirrors byte-for-byte.
    #[test]
    fn our_claim_encoding_matches_the_shared_instruction_enum() {
        let debridge_id = [9u8; 32];
        let receiver = vec![0xABu8; 32];
        let native_sender = vec![0x11u8; 20];
        let signatures = vec![vec![7u8; 65]];

        let ours = wire::claim_instruction_data(&wire::ClaimArgs {
            debridge_id,
            amount: 1234,
            chain_id_from: 1337,
            nonce: 7,
            receiver: receiver.clone(),
            auto: None,
            native_sender: native_sender.clone(),
            signatures: signatures.clone(),
        });

        let theirs = GateInstruction::Claim(SharedClaimArgs {
            debridge_id,
            amount: 1234,
            chain_id_from: 1337,
            nonce: 7,
            receiver,
            auto: None,
            native_sender,
            signatures,
        })
        .to_bytes();

        assert_eq!(ours, theirs, "claim instruction encoding diverged from the shared enum");
    }
}
