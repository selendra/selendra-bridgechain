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
use crate::gate::{
    compute_budget_for, decode_gate_config, domain_id, hex32, hex_bytes, recovered_address,
    GateConfig, CANCEL_PREFIX, SPL_TOKEN,
};
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

    /// Same field order as `ClaimArgs` but a different variant: `chain_id_from`
    /// rather than a destination, because a cancel is executed ON the destination
    /// for a transfer that came FROM somewhere else.
    #[derive(BorshSerialize)]
    pub struct CancelArgs {
        pub debridge_id: [u8; 32],
        pub amount: u64,
        pub chain_id_from: u64,
        pub nonce: u64,
        pub receiver: Vec<u8>,
        pub auto: Option<AutoParamsWire>,
        pub native_sender: Vec<u8>,
        pub signatures: Vec<Vec<u8>>,
    }

    /// `GateInstruction::Cancel` — discriminant 10. Pinned by
    /// `cancel_encoding_matches_the_shared_enum`, because a wrong constant here
    /// produces a well-formed transaction that runs the WRONG handler.
    pub fn cancel_instruction_data(args: &CancelArgs) -> Vec<u8> {
        let mut out = vec![10u8];
        args.serialize(&mut out).expect("borsh serialize CancelArgs");
        out
    }
}

/// Signatures ordered by recovered signer, ascending, keeping ONLY registered
/// validators and capping the result at the validator count.
///
/// The filter is the important half. The gate counts only validator signatures
/// toward quorum, so a non-validator signature can never help — but it still
/// costs ~25k CU to recover on-chain, and the array as a whole is now refused
/// outright if it is longer than the validator set. Forwarding one is therefore
/// pure downside: at best wasted compute, at worst a permanently stuck transfer.
///
/// `digest_input` is the PRE-EIP-191 digest of the domain being authorised — the
/// raw submissionId for a claim, `domain_id(CANCEL_PREFIX, id)` for a burn.
/// Passing the wrong one silently drops every signature (none would recover to a
/// validator), which is why every caller names its domain explicitly.
fn ordered_signatures(
    digest_input: &[u8; 32],
    raw: &[Vec<u8>],
    cfg: &GateConfig,
) -> Vec<Vec<u8>> {
    let digest = bridge_solana::verify::eth_signed_digest(digest_input);
    let mut with_addr: Vec<([u8; 20], Vec<u8>)> = raw
        .iter()
        // Re-encode to low-`s` / v ∈ {27,28} first. `secp256k1_recover` refuses a
        // high-`s` signature on-chain while host recovery accepts it, so
        // forwarding one builds an instruction that fails wholesale — and since
        // the off-chain quorum still reads as satisfied, the loop resubmits the
        // same doomed bytes forever. The store canonicalises on the way in now;
        // this heals rows written before that.
        .filter_map(|s| bridge_solana::verify::canonical_signature(s).map(|c| c.to_vec()))
        .filter_map(|s| recovered_address(&digest, &s).map(|a| (a, s)))
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
        // Surface the quorum requirement once, up front. This process cannot know
        // how many peers exist, but it CAN state what the gate demands — which is
        // the fact an operator running a single relayer against a 2-of-N gate is
        // missing.
        let (config, _) = Pubkey::find_program_address(&[b"config"], &self.program_id);
        // Retry: a diagnostic that gives up on the first transient RPC blip is
        // worse than none, because it reports "could not read" as though the gate
        // were misconfigured.
        let mut cfg_read = None;
        for _ in 0..5 {
            if let Some(c) =
                self.rpc.get_account(&config).await.ok().and_then(|a| decode_gate_config(&a.data).ok())
            {
                cfg_read = Some(c);
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        match cfg_read {
            Some(cfg) if cfg.threshold > 1 => info!(
                threshold = cfg.threshold,
                validators = cfg.validators.len(),
                "gate requires {} signatures; THIS process contributes 1 — {} relayers must run, \
                 each with a distinct validator key, or Solana-origin transfers never reach quorum",
                cfg.threshold,
                cfg.threshold
            ),
            Some(cfg) => info!(
                threshold = cfg.threshold,
                validators = cfg.validators.len(),
                "gate threshold is {}; this process alone can reach it",
                cfg.threshold
            ),
            None => warn!("could not read the gate config to report its threshold"),
        }
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
            // Cancel FIRST, deliberately: a burn is the opposite of a payout, so
            // it must not queue behind the checks that protect payouts. If a
            // cancel quorum exists the transfer is being unwound, and claiming it
            // would be the wrong outcome.
            match self.try_cancel_checked(&rec).await {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => warn!(submission_id = %rec.submission_id, error = %e, "cancel failed"),
            }
            if let Err(e) = self.try_claim(&rec).await {
                warn!(submission_id = %rec.submission_id, error = %e, "claim failed");
            }
        }
        Ok(())
    }

    /// Resolve the id + gate config, then attempt a burn. Returns true when one
    /// was submitted (or the transfer is already spent), so the caller skips the
    /// claim path.
    async fn try_cancel_checked(&self, rec: &SubmissionRecord) -> anyhow::Result<bool> {
        if rec.cancel_signatures.is_empty() {
            return Ok(false);
        }
        let id = hex32(&rec.submission_id)?;
        let (executed, _) = Pubkey::find_program_address(&[b"executed", &id], &self.program_id);
        if let Some(acct) =
            self.rpc.get_account_with_commitment(&executed, self.rpc.commitment()).await?.value
        {
            if acct.owner == self.program_id && !acct.data.is_empty() {
                return Ok(true); // already claimed or already burned
            }
        }
        let (config, _) = Pubkey::find_program_address(&[b"config"], &self.program_id);
        let cfg = decode_gate_config(&self.rpc.get_account(&config).await?.data)?;
        self.try_cancel(rec, &id, &cfg).await
    }

    /// Burn a stuck transfer on this (destination) gate once validators have
    /// attested it. Releases nothing — it only makes the transfer permanently
    /// unclaimable, which is the precondition for the SOURCE chain to repay.
    ///
    /// Checked BEFORE `try_claim`, mirroring the EVM keeper: a cancel is the
    /// opposite of a payout, so it must not sit behind the gates that exist to
    /// protect payouts.
    async fn try_cancel(
        &self,
        rec: &SubmissionRecord,
        id: &[u8; 32],
        gate_cfg: &GateConfig,
    ) -> anyhow::Result<bool> {
        let raw: Vec<Vec<u8>> =
            rec.cancel_signatures.iter().filter_map(|s| hex_bytes(&s.signature).ok()).collect();
        if raw.is_empty() {
            return Ok(false);
        }
        let digest = domain_id(CANCEL_PREFIX, id);
        let signatures = ordered_signatures(&digest, &raw, gate_cfg);
        if (signatures.len() as u32) < gate_cfg.threshold {
            return Ok(false);
        }

        let (config, _) = Pubkey::find_program_address(&[b"config"], &self.program_id);
        let (executed, _) = Pubkey::find_program_address(&[b"executed", id], &self.program_id);
        let args = wire::CancelArgs {
            debridge_id: hex32(&rec.debridge_id)?,
            amount: rec.amount.parse().map_err(|_| anyhow::anyhow!("amount exceeds u64"))?,
            chain_id_from: rec.chain_id_from,
            nonce: rec.nonce,
            receiver: hex_bytes(&rec.receiver)?,
            auto: None,
            native_sender: hex_bytes(&rec.native_sender)?,
            signatures,
        };
        let instruction = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new_readonly(config, false),
                AccountMeta::new(executed, false),
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
            ],
            data: wire::cancel_instruction_data(&args),
        };
        let units = compute_budget_for(gate_cfg.validators.len());
        let blockhash = self.rpc.get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &[ComputeBudgetInstruction::set_compute_unit_limit(units), instruction],
            Some(&self.payer.pubkey()),
            &[&self.payer],
            blockhash,
        );
        let sig = self.rpc.send_and_confirm_transaction(&tx).await?;
        info!(submission_id = %rec.submission_id, tx = %sig, "CANCELLED on Solana (burned)");
        Ok(true)
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
        //
        // Say so, though. Solana `Sent` events are signed ONLY by relayers — the
        // EVM validators never scan Solana — so a deployment running fewer
        // relayers than the threshold can never reach quorum, and the failure is
        // otherwise completely silent: the transfer just sits there. Naming the
        // shortfall is the difference between a five-minute fix and an afternoon.
        if (signatures.len() as u32) < gate_cfg.threshold {
            warn!(
                submission_id = %rec.submission_id,
                have = signatures.len(),
                need = gate_cfg.threshold,
                validators = gate_cfg.validators.len(),
                "below quorum — are enough relayers running, each with a DISTINCT validator key?"
            );
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
                AccountMeta::new_readonly(Pubkey::from_str(SPL_TOKEN).expect("valid spl-token id"), false),
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

    /// Build a `Config` account body the way the PROGRAM serializes it. Every
    /// field is written, in order, including the ones the decoder ignores —
    /// a fixture that omits a field cannot catch a decoder that omits the same
    /// field, which is exactly how the `bridge_domain` shift went unnoticed.
    fn program_config_bytes(validators: &[[u8; 20]], threshold: u32) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&[7u8; 32]); // owner
        d.extend_from_slice(&[9u8; 32]); // bridge_domain
        d.extend_from_slice(&[0u8; 32]); // guardian — Pubkey::default(), the common case
        d.extend_from_slice(&(validators.len() as u32).to_le_bytes());
        for v in validators {
            d.extend_from_slice(v);
        }
        d.extend_from_slice(&threshold.to_le_bytes());
        d.extend_from_slice(&7_565_164u64.to_le_bytes()); // chain_id
        d.push(0); // paused
        d.extend_from_slice(&8u32.to_le_bytes()); // max_validators
        d.extend_from_slice(&8u32.to_le_bytes()); // max_corridors
        d.extend_from_slice(&0u32.to_le_bytes()); // nonce_to.len()
        d
    }

    #[test]
    fn config_layout_matches_the_program() {
        let data = program_config_bytes(&[[0xAAu8; 20], [0xBBu8; 20]], 3);
        let cfg = decode_gate_config(&data).expect("decodes");
        assert_eq!(cfg.validators, vec![[0xAAu8; 20], [0xBBu8; 20]]);
        assert_eq!(cfg.threshold, 3);
    }

    /// The account is SIZED for max_validators/max_corridors, so a real one is
    /// followed by unused padding. Decoding must read a prefix, not demand an
    /// exact fit.
    #[test]
    fn trailing_capacity_padding_is_tolerated() {
        let mut data = program_config_bytes(&[[0xCCu8; 20]], 1);
        data.extend_from_slice(&[0u8; 200]);
        let cfg = decode_gate_config(&data).expect("decodes despite padding");
        assert_eq!(cfg.threshold, 1);
        assert_eq!(cfg.validators.len(), 1);
    }

    /// THE REGRESSION. `bridge_domain` was inserted between `owner` and
    /// `guardian`; a decoder still using the old offsets reads the validator
    /// count out of `guardian` — all zeros on a real gate — and yields "no
    /// validators, threshold 0" with no error at all. That silently filtered
    /// away every signature the claim submitter had.
    #[test]
    fn a_config_without_bridge_domain_is_rejected_not_silently_zeroed() {
        // The pre-domain layout: owner ‖ guardian ‖ validators ‖ threshold ‖ …
        let mut old = Vec::new();
        old.extend_from_slice(&[7u8; 32]); // owner
        old.extend_from_slice(&[0u8; 32]); // guardian
        old.extend_from_slice(&2u32.to_le_bytes());
        old.extend_from_slice(&[0xAAu8; 20]);
        old.extend_from_slice(&[0xBBu8; 20]);
        old.extend_from_slice(&2u32.to_le_bytes()); // threshold
        old.extend_from_slice(&7_565_164u64.to_le_bytes());
        old.push(0);

        match decode_gate_config(&old) {
            Err(_) => {}
            Ok(cfg) => panic!(
                "a stale-layout config decoded instead of erroring: {} validators, threshold {}",
                cfg.validators.len(),
                cfg.threshold
            ),
        }
    }

    /// A truncated or lying account must fail loudly rather than silently decode
    /// as an EMPTY validator set — that would filter every signature away and look
    /// exactly like "nobody has signed yet", hiding the fault indefinitely.
    #[test]
    fn a_truncated_config_is_an_error_not_an_empty_validator_set() {
        assert!(decode_gate_config(&[0u8; 40]).is_err(), "too short for the header");

        // Header claims 5 validators; the body holds none.
        let mut lying = vec![0u8; 96];
        lying.extend_from_slice(&5u32.to_le_bytes());
        assert!(decode_gate_config(&lying).is_err(), "declared length must be honoured");

        // An honestly-empty validator set is also refused: `init` cannot produce
        // one, so it can only mean layout drift.
        assert!(
            decode_gate_config(&program_config_bytes(&[], 0)).is_err(),
            "an empty validator set must be an error, not a filter that drops everything"
        );
    }

    /// The remediation plan listed the compute cost as "unmeasured, flagged for
    /// checking, not a finding". It has now been MEASURED against the deployed
    /// program on devnet:
    ///
    ///   2-of-2 claim -> 91,303 and 91,316 CU (two independent transactions)
    ///
    /// `secp256k1_recover` is ~25k CU each, so that decomposes as ~42k fixed
    /// (config + asset load, SPL account checks, keccak, marker creation, the
    /// token CPI) plus ~25k per signature:
    ///
    ///   total(n) ~= 42_000 + 25_000 * n
    ///
    /// (41k was the first guess and this test rejected it — 41 + 2x25 = 91,000,
    /// which is below the 91,316 actually charged. Pinning the measurement is
    /// what caught it.)
    ///
    /// which makes the plan's worry real rather than hypothetical: a 7-validator
    /// gate needs ~216k and the DEFAULT budget is 200k. Without an explicit
    /// request, a realistic validator set silently exceeds it — and every claim,
    /// cancel and refund for that submission fails until someone works out why.
    ///
    /// This test pins `compute_budget_for` against that model so the constants
    /// cannot drift below what the chain actually charges.
    #[test]
    fn the_compute_budget_covers_measured_cost() {
        /// Measured on devnet, program 7doepJ3tM2tU7vBEj17UKV77uC3P4RJ89ewNyuk7cLtv.
        const MEASURED_2_OF_2: u32 = 91_316;
        /// Derived from the measurement above; secp256k1_recover is ~25k.
        const FIXED: u32 = 42_000;
        const PER_SIG: u32 = 25_000;
        let model = |n: u32| FIXED + PER_SIG * n;

        // The model must not undershoot what we actually observed.
        assert!(
            model(2) >= MEASURED_2_OF_2,
            "model {} is below the measured {MEASURED_2_OF_2}",
            model(2)
        );

        // The request must cover the model at every realistic set size, including
        // the config's ~22-validator ceiling (finding L-3).
        for n in [1u32, 2, 3, 5, 7, 12, 22] {
            assert!(
                compute_budget_for(n as usize) >= model(n),
                "n={n}: requesting {} but the claim needs ~{}",
                compute_budget_for(n as usize),
                model(n)
            );
        }

        // The specific case the plan called out: 7 validators exceeds the 200k
        // default, so an explicit request is mandatory, not an optimisation.
        assert!(model(7) > 200_000, "the default budget should be insufficient at n=7");
        assert!(compute_budget_for(7) > 200_000);

        // And never ask for more than Solana will grant.
        assert!(compute_budget_for(1000) <= 1_400_000, "must stay under the per-tx maximum");
    }

    #[test]
    fn spl_token_id_is_the_canonical_one() {
        assert_eq!(
            Pubkey::from_str(SPL_TOKEN).unwrap().to_string(),
            "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
        );
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
    /// The hand-rolled `cancel` encoder prepends discriminant 10. If anyone
    /// reorders `GateInstruction`, that constant silently starts targeting a
    /// DIFFERENT instruction — the transaction stays well-formed and the program
    /// runs the wrong handler. For a burn that is the difference between
    /// unwinding a transfer and, say, changing the guardian.
    #[test]
    fn cancel_encoding_matches_the_shared_enum() {
        use bridge_solana::instruction::CancelArgs as SharedCancelArgs;
        let debridge_id = [9u8; 32];
        let receiver = vec![0xABu8; 32];
        let native_sender = vec![0x11u8; 20];
        let signatures = vec![vec![7u8; 65]];

        let ours = wire::cancel_instruction_data(&wire::CancelArgs {
            debridge_id,
            amount: 1234,
            chain_id_from: 1337,
            nonce: 7,
            receiver: receiver.clone(),
            auto: None,
            native_sender: native_sender.clone(),
            signatures: signatures.clone(),
        });
        let theirs = GateInstruction::Cancel(SharedCancelArgs {
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
        assert_eq!(ours, theirs, "cancel instruction encoding diverged from the shared enum");
    }

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
