//! Solana bridge gate — the on-chain counterpart of `Gate.sol`.
//!
//! Same protocol, different VM: `send` locks SPL into a vault and emits a `Sent`
//! event; `claim` verifies a threshold of *distinct* validator signatures and
//! releases funds exactly once (replay-safe). The submissionId is the sacred
//! keccak hash (computed here with the `keccak` syscall) and the signatures are
//! the same EIP-191 secp256k1 validator signatures the EVM gate accepts (verified
//! here with the `secp256k1_recover` syscall). One validator set signs for both
//! VMs — see the host crate `bridge-solana`, whose tests prove this logic
//! byte-for-byte against Gate.sol and bridge-core.
//!
//! Build: `cargo build-sbf --manifest-path crates/solana-gate/Cargo.toml`.
//!
//! Account model:
//!   * **Config PDA** (`["config"]`) — owner, validator set, threshold, chain id,
//!     per-target nonces, pause flag.
//!   * **Vault** — an SPL token account owned by a vault-authority PDA
//!     (`["vault", mint]`), holding the bridge's liquidity for a mint.
//!   * **Executed PDA** (`["executed", submissionId]`) — created on claim; its
//!     existence is the replay guard (a second claim fails to init it).

use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint,
    entrypoint::ProgramResult,
    keccak,
    program::invoke_signed,
    program_error::ProgramError,
    pubkey::Pubkey,
    secp256k1_recover::secp256k1_recover,
    sysvar::Sysvar,
    msg,
    rent::Rent,
    system_instruction,
    program::invoke,
};

/// deBridge's chain id for Solana mainnet (also the hash-fixture value).
pub const SOLANA_CHAIN_ID: u64 = 7565164;
const SUBMISSION_PREFIX: u64 = 1;

// ---------------------------------------------------------------------------
// Instructions (Borsh) — mirrors bridge_solana::instruction::GateInstruction.
// ---------------------------------------------------------------------------

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct AutoParamsWire {
    pub execution_fee: u128,
    pub flags: u64,
    pub fallback_address: Vec<u8>,
    pub data: Vec<u8>,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct InitArgs {
    pub validators: Vec<[u8; 20]>,
    pub threshold: u32,
    pub chain_id: u64,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct SendArgs {
    pub debridge_id: [u8; 32],
    pub amount: u64,
    pub chain_id_to: u64,
    pub receiver: Vec<u8>,
    pub auto: Option<AutoParamsWire>,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
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

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub enum GateInstruction {
    Init(InitArgs),
    Send(SendArgs),
    Claim(ClaimArgs),
    SetValidator { validator: [u8; 20], active: bool },
    SetThreshold { threshold: u32 },
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default)]
pub struct Config {
    pub owner: Pubkey,
    pub validators: Vec<[u8; 20]>,
    pub threshold: u32,
    pub chain_id: u64,
    pub paused: bool,
    /// (chainIdTo, nextNonce)
    pub nonce_to: Vec<(u64, u64)>,
}

impl Config {
    fn is_validator(&self, a: &[u8; 20]) -> bool {
        self.validators.iter().any(|v| v == a)
    }
    fn nonce(&self, chain_id_to: u64) -> u64 {
        self.nonce_to.iter().find(|(c, _)| *c == chain_id_to).map(|(_, n)| *n).unwrap_or(0)
    }
    fn bump_nonce(&mut self, chain_id_to: u64) {
        match self.nonce_to.iter_mut().find(|(c, _)| *c == chain_id_to) {
            Some(e) => e.1 += 1,
            None => self.nonce_to.push((chain_id_to, 1)),
        }
    }
}

/// Load the program's canonical `Config`, refusing any account that is not the
/// program-owned `["config"]` PDA.
///
/// SECURITY (finding C1): every instruction that trusts the config — claim
/// (validator set + threshold for signature verification), send (chain id +
/// nonces), and the owner-gated setters — MUST route through here. Without the
/// PDA + program-owner check, a caller could pass a config account they created
/// and own, containing their own validator set and threshold, and satisfy
/// `verify_threshold` with self-signed signatures — draining any vault the
/// program's authority controls. The `["config"]` seed makes the account unique
/// and unforgeable, and the owner check ensures only this program ever wrote it.
fn load_config(program_id: &Pubkey, config_ai: &AccountInfo) -> Result<Config, ProgramError> {
    let (expected, _bump) = Pubkey::find_program_address(&[b"config"], program_id);
    if config_ai.key != &expected {
        msg!("config account is not the canonical [\"config\"] PDA");
        return Err(ProgramError::InvalidSeeds);
    }
    if config_ai.owner != program_id {
        msg!("config account is not program-owned");
        return Err(ProgramError::IllegalOwner);
    }
    Config::deserialize(&mut &config_ai.data.borrow()[..])
        .map_err(|_| ProgramError::InvalidAccountData)
}

#[derive(thiserror::Error, Debug, Copy, Clone)]
pub enum GateError {
    #[error("amount must be non-zero")]
    ZeroAmount,
    #[error("receiver width must be 20 or 32")]
    BadReceiver,
    #[error("already executed (replay)")]
    AlreadyExecuted,
    #[error("signatures unordered or duplicated")]
    InvalidSignerOrder,
    #[error("not enough validator signatures")]
    NotEnoughSignatures,
    #[error("bad signature encoding")]
    BadSignature,
}

impl From<GateError> for ProgramError {
    fn from(e: GateError) -> Self {
        ProgramError::Custom(e as u32 + 1)
    }
}

// ---------------------------------------------------------------------------
// Hashing (keccak syscall) — byte-identical to BridgeHash.sol / bridge-core.
// ---------------------------------------------------------------------------

fn be32(v: u64) -> [u8; 32] {
    let mut o = [0u8; 32];
    o[24..].copy_from_slice(&v.to_be_bytes());
    o
}

fn amount_word(v: u64) -> [u8; 32] {
    let mut o = [0u8; 32];
    o[24..].copy_from_slice(&v.to_be_bytes());
    o
}

#[allow(clippy::too_many_arguments)]
fn submission_id(
    debridge_id: &[u8; 32],
    amount: u64,
    chain_id_from: u64,
    chain_id_to: u64,
    nonce: u64,
    receiver: &[u8],
    auto: Option<&AutoParamsWire>,
    native_sender: &[u8],
) -> [u8; 32] {
    let prefix = be32(SUBMISSION_PREFIX);
    let cf = be32(chain_id_from);
    let ct = be32(chain_id_to);
    let amt = amount_word(amount);
    let nz = be32(nonce);
    // packedSubmission = prefix|debridgeId|chainIdFrom|chainIdTo|amount|receiver|nonce
    let base: &[&[u8]] = &[&prefix, debridge_id, &cf, &ct, &amt, receiver, &nz];
    match auto {
        None => keccak::hashv(base).to_bytes(),
        Some(a) => {
            let mut fee = [0u8; 32];
            fee[16..].copy_from_slice(&a.execution_fee.to_be_bytes());
            let flags = be32(a.flags);
            let fb = keccak::hashv(&[&a.fallback_address]).to_bytes();
            let data = keccak::hashv(&[&a.data]).to_bytes();
            let ns = keccak::hashv(&[native_sender]).to_bytes();
            // keccak(packedSubmission || fee || flags || keccak(fallback) || keccak(data) || keccak(nativeSender))
            keccak::hashv(&[
                &prefix, debridge_id, &cf, &ct, &amt, receiver, &nz, &fee, &flags, &fb, &data, &ns,
            ])
            .to_bytes()
        }
    }
}

// ---------------------------------------------------------------------------
// Signature verification (secp256k1_recover syscall) — mirrors _verifySignatures.
// ---------------------------------------------------------------------------

fn eth_signed_digest(id: &[u8; 32]) -> [u8; 32] {
    keccak::hashv(&[b"\x19Ethereum Signed Message:\n32", id]).to_bytes()
}

fn recover_evm_address(digest: &[u8; 32], sig65: &[u8]) -> Result<[u8; 20], GateError> {
    if sig65.len() != 65 {
        return Err(GateError::BadSignature);
    }
    let v = sig65[64];
    if v != 27 && v != 28 {
        return Err(GateError::BadSignature);
    }
    let pubkey = secp256k1_recover(digest, v - 27, &sig65[..64]).map_err(|_| GateError::BadSignature)?;
    // address = keccak(uncompressed_pubkey_xy)[12..]
    let h = keccak::hashv(&[&pubkey.0]).to_bytes();
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&h[12..]);
    Ok(addr)
}

fn verify_threshold(cfg: &Config, id: &[u8; 32], signatures: &[Vec<u8>]) -> Result<(), GateError> {
    let digest = eth_signed_digest(id);
    let mut last = [0u8; 20];
    let mut have_last = false;
    let mut count: u32 = 0;
    for sig in signatures {
        let signer = recover_evm_address(&digest, sig)?;
        if have_last && signer <= last {
            return Err(GateError::InvalidSignerOrder);
        }
        if cfg.is_validator(&signer) {
            count += 1;
        }
        last = signer;
        have_last = true;
    }
    if count < cfg.threshold {
        return Err(GateError::NotEnoughSignatures);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

entrypoint!(process_instruction);

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let ix = GateInstruction::try_from_slice(data).map_err(|_| ProgramError::InvalidInstructionData)?;
    match ix {
        GateInstruction::Init(args) => process_init(program_id, accounts, args),
        GateInstruction::Send(args) => process_send(program_id, accounts, args),
        GateInstruction::Claim(args) => process_claim(program_id, accounts, args),
        GateInstruction::SetValidator { validator, active } => {
            process_set_validator(program_id, accounts, validator, active)
        }
        GateInstruction::SetThreshold { threshold } => {
            process_set_threshold(program_id, accounts, threshold)
        }
    }
}

/// Accounts: [config_pda(w), payer(s,w), system_program]
fn process_init(program_id: &Pubkey, accounts: &[AccountInfo], args: InitArgs) -> ProgramResult {
    let it = &mut accounts.iter();
    let config_ai = next_account_info(it)?;
    let payer = next_account_info(it)?;
    let system_program = next_account_info(it)?;

    let (expected, bump) = Pubkey::find_program_address(&[b"config"], program_id);
    if config_ai.key != &expected {
        return Err(ProgramError::InvalidSeeds);
    }
    if !config_ai.data_is_empty() {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    if args.threshold == 0 || args.threshold > args.validators.len() as u32 {
        return Err(ProgramError::InvalidArgument);
    }

    // Room for the validator set + a handful of per-chain nonces to grow into.
    let space: usize = 512;
    let rent = Rent::get()?.minimum_balance(space);
    invoke_signed(
        &system_instruction::create_account(payer.key, config_ai.key, rent, space as u64, program_id),
        &[payer.clone(), config_ai.clone(), system_program.clone()],
        &[&[b"config", &[bump]]],
    )?;

    let cfg = Config {
        owner: *payer.key,
        validators: args.validators,
        threshold: args.threshold,
        chain_id: args.chain_id,
        paused: false,
        nonce_to: Vec::new(),
    };
    cfg.serialize(&mut &mut config_ai.data.borrow_mut()[..])?;
    msg!("gate initialized: {} validators, threshold {}", cfg.validators.len(), cfg.threshold);
    Ok(())
}

/// Accounts: [config(w), payer(s), user_token(w), vault(w), spl_token_program]
fn process_send(program_id: &Pubkey, accounts: &[AccountInfo], args: SendArgs) -> ProgramResult {
    if args.amount == 0 {
        return Err(GateError::ZeroAmount.into());
    }
    if args.receiver.len() != 20 && args.receiver.len() != 32 {
        return Err(GateError::BadReceiver.into());
    }
    let it = &mut accounts.iter();
    let config_ai = next_account_info(it)?;
    let payer = next_account_info(it)?;
    let user_token = next_account_info(it)?;
    let vault = next_account_info(it)?;
    let token_program = next_account_info(it)?;

    // C1: bind the chain id + nonce sequence to the canonical config, so a forged
    // config can't rewrite them and desync the off-chain nonce/submissionId.
    let mut cfg = load_config(program_id, config_ai)?;
    let nonce = cfg.nonce(args.chain_id_to);

    let native_sender = payer.key.to_bytes();
    let id = submission_id(
        &args.debridge_id,
        args.amount,
        cfg.chain_id,
        args.chain_id_to,
        nonce,
        &args.receiver,
        args.auto.as_ref(),
        &native_sender,
    );

    // effects before interaction
    cfg.bump_nonce(args.chain_id_to);
    cfg.serialize(&mut &mut config_ai.data.borrow_mut()[..])?;

    // Emit the Sent event as structured program data for the validator's source.
    emit_sent(&id, &args, &cfg.chain_id, nonce, &native_sender);

    // Lock: user -> vault (SPL CPI).
    let transfer = spl_token::instruction::transfer(
        token_program.key,
        user_token.key,
        vault.key,
        payer.key,
        &[],
        args.amount,
    )?;
    invoke(&transfer, &[user_token.clone(), vault.clone(), payer.clone(), token_program.clone()])?;
    Ok(())
}

/// Accounts: [config, executed_pda(w), payer(s,w), vault(w), receiver_token(w),
///            vault_authority, spl_token_program, system_program]
fn process_claim(program_id: &Pubkey, accounts: &[AccountInfo], args: ClaimArgs) -> ProgramResult {
    let it = &mut accounts.iter();
    let config_ai = next_account_info(it)?;
    let executed_ai = next_account_info(it)?;
    let payer = next_account_info(it)?;
    let vault = next_account_info(it)?;
    let receiver_token = next_account_info(it)?;
    let vault_authority = next_account_info(it)?;
    let token_program = next_account_info(it)?;
    let system_program = next_account_info(it)?;

    // C1: only the canonical program-owned config may drive signature checks.
    let cfg = load_config(program_id, config_ai)?;

    // Bind the release destination to the signed `receiver`. `claim` is
    // deliberately permissionless (any keeper may submit a threshold-signed
    // submission), so the token account funds are released to must be exactly
    // the one the validators signed for -- not whatever `receiver_token` the
    // caller happens to supply. Without this check anyone who observes a
    // threshold-signed submission (e.g. via the public sig-store) could claim
    // it themselves and redirect the payout to their own account.
    let receiver: [u8; 32] =
        args.receiver.as_slice().try_into().map_err(|_| GateError::BadReceiver)?;
    if receiver_token.key != &Pubkey::new_from_array(receiver) {
        return Err(GateError::BadReceiver.into());
    }

    let id = submission_id(
        &args.debridge_id,
        args.amount,
        args.chain_id_from,
        cfg.chain_id,
        args.nonce,
        &args.receiver,
        args.auto.as_ref(),
        &args.native_sender,
    );

    // Replay guard: the executed PDA must not exist yet; creating it marks done.
    let (expected_executed, bump) =
        Pubkey::find_program_address(&[b"executed", &id], program_id);
    if executed_ai.key != &expected_executed {
        return Err(ProgramError::InvalidSeeds);
    }
    if executed_ai.lamports() > 0 || !executed_ai.data_is_empty() {
        return Err(GateError::AlreadyExecuted.into());
    }

    verify_threshold(&cfg, &id, &args.signatures)?;

    // Create the executed marker (effects before interaction).
    let rent = Rent::get()?.minimum_balance(0);
    invoke_signed(
        &system_instruction::create_account(payer.key, executed_ai.key, rent, 0, program_id),
        &[payer.clone(), executed_ai.clone(), system_program.clone()],
        &[&[b"executed", &id, &[bump]]],
    )?;

    // Release: vault -> receiver, signed by the vault-authority PDA. Assert the
    // supplied vault_authority IS the canonical PDA (defense in depth: the SPL
    // transfer would already fail if the vault's authority didn't match, but an
    // explicit check fails fast with a clear error and documents the invariant).
    let (auth, auth_bump) = Pubkey::find_program_address(&[b"vault_authority"], program_id);
    if vault_authority.key != &auth {
        return Err(ProgramError::InvalidSeeds);
    }
    let transfer = spl_token::instruction::transfer(
        token_program.key,
        vault.key,
        receiver_token.key,
        vault_authority.key,
        &[],
        args.amount,
    )?;
    invoke_signed(
        &transfer,
        &[vault.clone(), receiver_token.clone(), vault_authority.clone(), token_program.clone()],
        &[&[b"vault_authority", &[auth_bump]]],
    )?;

    msg!("CLAIMED {}", bs58_id(&id));
    Ok(())
}

fn process_set_validator(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    validator: [u8; 20],
    active: bool,
) -> ProgramResult {
    let it = &mut accounts.iter();
    let config_ai = next_account_info(it)?;
    let owner = next_account_info(it)?;
    // C1: mutate only the canonical program-owned config, and only for its owner.
    let mut cfg = load_config(program_id, config_ai)?;
    if owner.key != &cfg.owner || !owner.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let present = cfg.is_validator(&validator);
    if active && !present {
        cfg.validators.push(validator);
    } else if !active && present {
        cfg.validators.retain(|v| v != &validator);
        if (cfg.validators.len() as u32) < cfg.threshold {
            return Err(ProgramError::InvalidArgument);
        }
    }
    cfg.serialize(&mut &mut config_ai.data.borrow_mut()[..])?;
    Ok(())
}

fn process_set_threshold(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    threshold: u32,
) -> ProgramResult {
    let it = &mut accounts.iter();
    let config_ai = next_account_info(it)?;
    let owner = next_account_info(it)?;
    // C1: mutate only the canonical program-owned config, and only for its owner.
    let mut cfg = load_config(program_id, config_ai)?;
    if owner.key != &cfg.owner || !owner.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if threshold == 0 || threshold > cfg.validators.len() as u32 {
        return Err(ProgramError::InvalidArgument);
    }
    cfg.threshold = threshold;
    cfg.serialize(&mut &mut config_ai.data.borrow_mut()[..])?;
    Ok(())
}

/// Emit the Sent event as structured program data (base64 in the tx logs) so the
/// validator's Solana source can decode it. The tag distinguishes it from other
/// program logs, matching `bridge_solana::relayer::SENT_LOG_TAG`.
fn emit_sent(id: &[u8; 32], args: &SendArgs, chain_id_from: &u64, nonce: u64, native_sender: &[u8]) {
    let mut buf = Vec::new();
    // A compact, borsh-friendly event the off-chain source decodes.
    let _ = (id, chain_id_from, nonce, native_sender);
    buf.extend_from_slice(id);
    buf.extend_from_slice(&args.debridge_id);
    solana_program::log::sol_log_data(&[b"BRIDGE_SENT", &buf]);
}

fn bs58_id(id: &[u8; 32]) -> String {
    // small helper; solana-program pulls in bs58 transitively
    solana_program::pubkey::Pubkey::new_from_array(*id).to_string()
}
