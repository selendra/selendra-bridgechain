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
//!     signature from any key that recovers to its claimed signer; it does not
//!     know the validator set. So anyone holding a `Sign`-scoped token can append
//!     junk-but-recoverable signatures to a pending submission. Forwarding them is
//!     not merely wasteful: each one costs the gate ~25k CU in `secp256k1_recover`,
//!     so a handful exhausts the compute budget and `claim` — plus `cancel` and
//!     `refund`, which share the same verifier — fail forever. We therefore read
//!     the on-chain config, keep only signatures that recover to a REGISTERED
//!     validator, and cap the array at the validator count.
//!   * **skip what is already executed** — the marker PDA is authoritative.

use std::str::FromStr;
use std::time::Duration;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::compute_budget::ComputeBudgetInstruction;
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

/// The subset of the on-chain `Config` this process needs: who may sign, and how
/// many signatures constitute a quorum.
///
/// Hand-decoded from the account's Borsh bytes rather than by depending on
/// `solana-gate` — that crate is outside this workspace and pins its own
/// (deliberately v3) lockfile. The layout is pinned by
/// `config_layout_matches_the_program` below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateConfig {
    pub validators: Vec<[u8; 20]>,
    pub threshold: u32,
}

/// Decode `Config { owner, guardian, validators, threshold, .. }` — we stop
/// reading after `threshold`, so trailing fields (chain_id, paused, capacities,
/// nonce_to) are ignored and may change without breaking this.
fn decode_gate_config(data: &[u8]) -> anyhow::Result<GateConfig> {
    // owner(32) + guardian(32) + vec-len(4)
    if data.len() < 68 {
        anyhow::bail!("config account is too short to hold a validator set");
    }
    let n = u32::from_le_bytes(data[64..68].try_into()?) as usize;
    let end = 68 + n * 20;
    if data.len() < end + 4 {
        anyhow::bail!("config account declares {n} validators but is too short to hold them");
    }
    let validators =
        (0..n).map(|i| {
            let off = 68 + i * 20;
            let mut a = [0u8; 20];
            a.copy_from_slice(&data[off..off + 20]);
            a
        })
        .collect();
    let threshold = u32::from_le_bytes(data[end..end + 4].try_into()?);
    Ok(GateConfig { validators, threshold })
}

/// Signatures ordered by recovered signer, ascending, keeping ONLY registered
/// validators and capping the result at the validator count.
///
/// The filter is the important half. The gate counts only validator signatures
/// toward quorum, so a non-validator signature can never help — but it still
/// costs ~25k CU to recover on-chain, and the array as a whole is now refused
/// outright if it is longer than the validator set. Forwarding one is therefore
/// pure downside: at best wasted compute, at worst a permanently stuck transfer.
fn ordered_signatures(
    submission_id: &[u8; 32],
    raw: &[Vec<u8>],
    cfg: &GateConfig,
) -> Vec<Vec<u8>> {
    let digest = bridge_solana::verify::eth_signed_digest(submission_id);
    let mut with_addr: Vec<([u8; 20], Vec<u8>)> = raw
        .iter()
        .filter_map(|s| recovered_address(&digest, s).map(|a| (a, s.clone())))
        // Junk from a throwaway key recovers fine — membership is the real filter.
        .filter(|(a, _)| cfg.validators.contains(a))
        .collect();
    with_addr.sort_by_key(|(a, _)| *a);
    with_addr.dedup_by_key(|(a, _)| *a); // the gate requires STRICTLY ascending
    with_addr.truncate(cfg.validators.len());
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

        let (asset, _) = Pubkey::find_program_address(&[b"asset", &debridge_id], &self.program_id);
        let (config, _) = Pubkey::find_program_address(&[b"config"], &self.program_id);
        let (vault_authority, _) =
            Pubkey::find_program_address(&[b"vault_authority"], &self.program_id);

        // Who may sign, straight from the canonical config PDA — never from the
        // store, which is exactly the thing an attacker can write to.
        let config_account = self
            .rpc
            .get_account(&config)
            .await
            .map_err(|_| anyhow::anyhow!("gate config PDA is not initialized"))?;
        let gate_cfg = decode_gate_config(&config_account.data)?;

        let raw_sigs: Vec<Vec<u8>> =
            rec.signatures.iter().filter_map(|s| hex_bytes(&s.signature).ok()).collect();
        let signatures = ordered_signatures(&id, &raw_sigs, &gate_cfg);
        // Below quorum there is nothing to submit: the gate would reject it, and
        // sending anyway burns fees on a guaranteed revert every poll.
        if (signatures.len() as u32) < gate_cfg.threshold {
            return Ok(());
        }
        if raw_sigs.len() > signatures.len() {
            warn!(
                submission_id = %rec.submission_id,
                dropped = raw_sigs.len() - signatures.len(),
                kept = signatures.len(),
                "dropped stored signatures that are not from registered validators"
            );
        }

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

        // The default budget is 200k CU per instruction. A single
        // `secp256k1_recover` is ~25k, so a legitimate 5-of-7 gate spends ~175k on
        // recovery alone — before keccak, Borsh and the SPL transfer. Ask for a
        // budget sized to the actual validator set instead of discovering the
        // ceiling as an unexplained failure on a larger gate.
        let units = compute_budget_for(gate_cfg.validators.len());

        let blockhash = self.rpc.get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &[ComputeBudgetInstruction::set_compute_unit_limit(units), instruction],
            Some(&self.payer.pubkey()),
            &[&self.payer],
            blockhash,
        );
        let sig = self.rpc.send_and_confirm_transaction(&tx).await?;
        info!(submission_id = %rec.submission_id, tx = %sig, "CLAIMED on Solana");
        Ok(())
    }
}

/// Compute units to request for a claim against a gate with `validators`
/// registered signers.
///
/// ~25k CU per `secp256k1_recover`, plus headroom for keccak, Borsh
/// deserialization, marker creation and the SPL transfer. Clamped to Solana's
/// 1.4M per-transaction maximum; requesting more is rejected outright.
fn compute_budget_for(validators: usize) -> u32 {
    const PER_SIGNATURE: u32 = 30_000;
    const OVERHEAD: u32 = 120_000;
    const MAX: u32 = 1_400_000;
    OVERHEAD.saturating_add(PER_SIGNATURE.saturating_mul(validators as u32)).min(MAX)
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

    /// A gate whose validator set is exactly the signers behind `seeds`.
    fn gate_for(seeds: &[u8], digest: &[u8; 32], threshold: u32) -> GateConfig {
        GateConfig {
            validators: seeds.iter().map(|s| sign(*s, digest).0).collect(),
            threshold,
        }
    }

    /// The gate requires strictly ascending signers; an out-of-order array is
    /// rejected outright.
    #[test]
    fn signatures_come_out_ascending() {
        let id = [0x11u8; 32];
        let digest = bridge_solana::verify::eth_signed_digest(&id);
        let raw: Vec<Vec<u8>> = (1u8..=4).map(|s| sign(s, &digest).1).collect();
        let cfg = gate_for(&[1, 2, 3, 4], &digest, 2);

        let ordered = ordered_signatures(&id, &raw, &cfg);
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
        let cfg = gate_for(&[1], &digest, 1);

        let ordered = ordered_signatures(&id, &raw, &cfg);
        assert_eq!(ordered.len(), 1, "only the recoverable signature survives");
    }

    /// Duplicate signers would break the strictly-ascending rule.
    #[test]
    fn duplicate_signers_are_deduped() {
        let id = [0x11u8; 32];
        let digest = bridge_solana::verify::eth_signed_digest(&id);
        let s = sign(3, &digest).1;
        let cfg = gate_for(&[3], &digest, 1);
        let ordered = ordered_signatures(&id, &[s.clone(), s], &cfg);
        assert_eq!(ordered.len(), 1);
    }

    /// THE griefing scenario, and the reason the filter exists.
    ///
    /// The sig-store authenticates a signature against its CLAIMED signer, not
    /// against the validator set, so anyone with a `Sign`-scoped token can attach
    /// signatures from throwaway keys. Each one costs the gate ~25k CU to recover,
    /// and the gate now refuses any array longer than its validator set outright —
    /// so forwarding them would make the transfer permanently unclaimable, taking
    /// `cancel` and `refund` (same verifier) down with it.
    #[test]
    fn signatures_from_non_validators_never_reach_the_gate() {
        let id = [0x11u8; 32];
        let digest = bridge_solana::verify::eth_signed_digest(&id);

        // A 3-validator gate; seeds 1..=3 are the real validators.
        let cfg = gate_for(&[1, 2, 3], &digest, 2);

        // Two of them have signed — a legitimate quorum. An attacker piles on six
        // perfectly valid signatures from keys nobody registered.
        let mut raw: Vec<Vec<u8>> = vec![sign(1, &digest).1, sign(2, &digest).1];
        raw.extend((50u8..=55).map(|s| sign(s, &digest).1));
        assert_eq!(raw.len(), 8, "8 recoverable signatures were stored");

        let ordered = ordered_signatures(&id, &raw, &cfg);

        assert_eq!(ordered.len(), 2, "only the two registered validators survive");
        assert!(
            ordered.len() <= cfg.validators.len(),
            "the array must never exceed the validator count — the gate refuses it on length"
        );
        for sig in &ordered {
            let signer = recovered_address(&digest, sig).unwrap();
            assert!(cfg.validators.contains(&signer), "a non-validator signature leaked through");
        }
    }

    /// The cap still holds if the config somehow yields duplicate members.
    #[test]
    fn the_array_is_capped_at_the_validator_count() {
        let id = [0x11u8; 32];
        let digest = bridge_solana::verify::eth_signed_digest(&id);
        let raw: Vec<Vec<u8>> = (1u8..=5).map(|s| sign(s, &digest).1).collect();

        let cfg = gate_for(&[1, 2], &digest, 2);
        assert_eq!(ordered_signatures(&id, &raw, &cfg).len(), 2);
    }

    /// Borsh `Config`: owner(32) ‖ guardian(32) ‖ len(4) ‖ validators(20·n) ‖
    /// threshold(4) ‖ … — decoded by hand here, so it is pinned by a test.
    #[test]
    fn config_layout_matches_the_program() {
        let mut data = Vec::new();
        data.extend_from_slice(&[7u8; 32]); // owner
        data.extend_from_slice(&[8u8; 32]); // guardian
        data.extend_from_slice(&2u32.to_le_bytes()); // validators.len()
        data.extend_from_slice(&[0xAAu8; 20]);
        data.extend_from_slice(&[0xBBu8; 20]);
        data.extend_from_slice(&3u32.to_le_bytes()); // threshold
        data.extend_from_slice(&7565164u64.to_le_bytes()); // chain_id — ignored
        data.push(0); // paused — ignored

        let cfg = decode_gate_config(&data).expect("decodes");
        assert_eq!(cfg.validators, vec![[0xAAu8; 20], [0xBBu8; 20]]);
        assert_eq!(cfg.threshold, 3);
    }

    /// A truncated or lying account must fail loudly rather than silently decode
    /// as an EMPTY validator set — that would filter every signature away and look
    /// exactly like "nobody has signed yet", hiding the fault indefinitely.
    #[test]
    fn a_truncated_config_is_an_error_not_an_empty_validator_set() {
        assert!(decode_gate_config(&[0u8; 40]).is_err(), "too short for the header");

        // Header claims 5 validators; the body holds none.
        let mut lying = vec![0u8; 64];
        lying.extend_from_slice(&5u32.to_le_bytes());
        assert!(decode_gate_config(&lying).is_err(), "declared length must be honoured");
    }

    /// A budget that does not scale with the validator set is the failure the
    /// remediation plan flagged as "unmeasured": 5-of-7 spends ~175k CU on
    /// recovery alone, against a 200k default.
    #[test]
    fn the_compute_budget_scales_with_the_validator_set() {
        assert!(
            compute_budget_for(7) > 200_000,
            "a 7-validator gate needs more than the default budget"
        );
        assert!(compute_budget_for(7) > compute_budget_for(3), "must scale with the set");
        assert!(compute_budget_for(1000) <= 1_400_000, "must stay under Solana's per-tx maximum");
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
