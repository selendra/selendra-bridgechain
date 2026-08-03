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
//!     per-target nonces, pause flag. Initialized only by the program's upgrade
//!     authority (the deployer), so governance can't be front-run at deploy time.
//!   * **Asset PDA** (`["asset", debridgeId]`) — the program-owned registry that
//!     binds a `debridgeId` to the SPL `mint` + `vault` allowed to back it. A
//!     `send`/`claim` may only touch the vault registered here for the SIGNED
//!     asset (finding C1).
//!   * **Vault** — an SPL token account owned by ONE global vault-authority PDA
//!     (`["vault_authority"]`), holding the bridge's liquidity. Which vault backs
//!     a given asset is pinned by the Asset PDA above, not by the authority seed.
//!   * **Executed PDA** (`["executed", submissionId]`) — created on claim; its
//!     existence is the replay guard (a second claim fails to init it).

use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    bpf_loader_upgradeable::{self, UpgradeableLoaderState},
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
    program_pack::Pack,
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
    /// C1: bind a `debridge_id` to the SPL `mint` + `vault` that may back it.
    /// Owner-gated. New variant appended last so existing discriminants (0..=4)
    /// stay byte-compatible with `bridge_solana::instruction::GateInstruction`.
    RegisterAsset { debridge_id: [u8; 32] },
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
    verify_config_account(config_ai.key, config_ai.owner, program_id)?;
    Config::deserialize(&mut &config_ai.data.borrow()[..])
        .map_err(|_| ProgramError::InvalidAccountData)
}

/// Pure C1 config-account gate (host-testable): the account MUST be the canonical
/// `["config"]` PDA AND program-owned. A forged config the caller created and
/// owns fails one of these, so it can never drive signature verification.
fn verify_config_account(
    config_key: &Pubkey,
    config_owner: &Pubkey,
    program_id: &Pubkey,
) -> Result<(), ProgramError> {
    let (expected, _bump) = Pubkey::find_program_address(&[b"config"], program_id);
    if config_key != &expected {
        msg!("config account is not the canonical [\"config\"] PDA");
        return Err(ProgramError::InvalidSeeds);
    }
    if config_owner != program_id {
        msg!("config account is not program-owned");
        return Err(ProgramError::IllegalOwner);
    }
    Ok(())
}

/// C1 asset registry: which SPL `mint` + `vault` may back a given `debridge_id`.
/// Stored in the program-owned PDA `["asset", debridge_id]`, so a claim/send can
/// only touch the vault governance explicitly bound to the signed asset — not an
/// arbitrary token account the caller supplies.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default)]
pub struct AssetConfig {
    pub debridge_id: [u8; 32],
    pub mint: Pubkey,
    pub vault: Pubkey,
}

/// Load the canonical asset binding for `debridge_id`, refusing any account that
/// is not the program-owned `["asset", debridge_id]` PDA. Mirrors [`load_config`].
fn load_asset(
    program_id: &Pubkey,
    debridge_id: &[u8; 32],
    asset_ai: &AccountInfo,
) -> Result<AssetConfig, ProgramError> {
    verify_asset_account(asset_ai.key, asset_ai.owner, debridge_id, program_id)?;
    let asset = AssetConfig::deserialize(&mut &asset_ai.data.borrow()[..])
        .map_err(|_| ProgramError::InvalidAccountData)?;
    if &asset.debridge_id != debridge_id {
        // A bound-but-mismatched record can only mean seed confusion; reject.
        return Err(ProgramError::InvalidSeeds);
    }
    Ok(asset)
}

/// Pure C1 asset-PDA gate (host-testable): the account MUST be the canonical
/// `["asset", debridge_id]` PDA AND program-owned.
fn verify_asset_account(
    asset_key: &Pubkey,
    asset_owner: &Pubkey,
    debridge_id: &[u8; 32],
    program_id: &Pubkey,
) -> Result<(), ProgramError> {
    let (expected, _bump) = Pubkey::find_program_address(&[b"asset", debridge_id], program_id);
    if asset_key != &expected {
        msg!("asset account is not the canonical [\"asset\", debridge_id] PDA");
        return Err(ProgramError::InvalidSeeds);
    }
    if asset_owner != program_id {
        msg!("asset account is not program-owned");
        return Err(ProgramError::IllegalOwner);
    }
    Ok(())
}

/// Pure C1 asset-binding gate (host-testable): the vault a send/claim touches and
/// the counterpart token account (user on send, receiver on claim) must both be
/// the registered vault / hold the registered mint, under the real SPL token
/// program. A forged-config drain that points at a different asset's vault fails
/// here.
fn verify_asset_binding(
    asset: &AssetConfig,
    token_program_key: &Pubkey,
    vault_key: &Pubkey,
    vault_mint: &Pubkey,
    counterpart_mint: &Pubkey,
) -> Result<(), ProgramError> {
    if token_program_key != &spl_token::id() {
        return Err(ProgramError::IncorrectProgramId);
    }
    if vault_key != &asset.vault {
        return Err(ProgramError::InvalidAccountData);
    }
    if vault_mint != &asset.mint || counterpart_mint != &asset.mint {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(())
}

/// Pure C1 init-authority gate (host-testable): only the program's upgrade
/// authority (the deployer) may initialize the config; an immutable program
/// (`None`) or any other signer is refused.
fn verify_upgrade_authority(
    upgrade_authority: Option<Pubkey>,
    who: &Pubkey,
) -> Result<(), ProgramError> {
    match upgrade_authority {
        Some(auth) if &auth == who => Ok(()),
        Some(_) => Err(ProgramError::MissingRequiredSignature),
        None => Err(ProgramError::InvalidArgument),
    }
}

/// Read the SPL mint + owner of a token account, asserting it is a real SPL
/// token account owned by the given `token_program`. Used to verify the vault
/// and receiver token accounts against the registry (C1: "verify token program,
/// vault, vault authority, receiver mint, and all writable owners").
fn spl_mint_and_owner(
    token_ai: &AccountInfo,
    token_program: &Pubkey,
) -> Result<(Pubkey, Pubkey), ProgramError> {
    if token_ai.owner != token_program {
        msg!("token account is not owned by the SPL token program");
        return Err(ProgramError::IllegalOwner);
    }
    let acct = spl_token::state::Account::unpack(&token_ai.data.borrow())
        .map_err(|_| ProgramError::InvalidAccountData)?;
    Ok((acct.mint, acct.owner))
}

/// Assert `who` is the program's on-chain upgrade authority. Reads the
/// BPF-loader-upgradeable `ProgramData` account (`Program.programdata_address`),
/// verifying both accounts are the canonical ones for `program_id`. Used to gate
/// `init` to the deployer (C1 atomic/authorized initialization).
fn require_upgrade_authority(
    program_id: &Pubkey,
    program_ai: &AccountInfo,
    program_data_ai: &AccountInfo,
    who: &Pubkey,
) -> Result<(), ProgramError> {
    if program_ai.key != program_id {
        msg!("init: program account is not this program");
        return Err(ProgramError::IncorrectProgramId);
    }
    if program_ai.owner != &bpf_loader_upgradeable::id() {
        msg!("init: program is not owned by the upgradeable loader");
        return Err(ProgramError::IncorrectProgramId);
    }
    // The Program account points at its ProgramData account; verify the link.
    let (expected_pd, _) =
        Pubkey::find_program_address(&[program_id.as_ref()], &bpf_loader_upgradeable::id());
    if program_data_ai.key != &expected_pd {
        msg!("init: wrong ProgramData account for this program");
        return Err(ProgramError::InvalidArgument);
    }
    match bincode_deserialize_loader_state(&program_data_ai.data.borrow())? {
        UpgradeableLoaderState::ProgramData { upgrade_authority_address, .. } => {
            verify_upgrade_authority(upgrade_authority_address, who)
        }
        _ => Err(ProgramError::InvalidAccountData),
    }
}

/// Deserialize an `UpgradeableLoaderState` from a ProgramData account (the BPF
/// loader serializes it with bincode).
fn bincode_deserialize_loader_state(
    data: &[u8],
) -> Result<UpgradeableLoaderState, ProgramError> {
    bincode::deserialize::<UpgradeableLoaderState>(data)
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
        GateInstruction::RegisterAsset { debridge_id } => {
            process_register_asset(program_id, accounts, debridge_id)
        }
    }
}

/// Accounts: [config_pda(w), payer(s,w), system_program, program, program_data]
///
/// C1 (atomic/authorized init): the config PDA is unique, so whoever creates it
/// first becomes `owner` forever. Previously that was ANY caller — a front-runner
/// could seize governance the instant the program was deployed. We now require
/// the initializer to be the program's on-chain **upgrade authority** (the
/// deployer), read from the BPF-loader `ProgramData` account, so only the
/// intended deployer can initialize the gate.
fn process_init(program_id: &Pubkey, accounts: &[AccountInfo], args: InitArgs) -> ProgramResult {
    let it = &mut accounts.iter();
    let config_ai = next_account_info(it)?;
    let payer = next_account_info(it)?;
    let system_program = next_account_info(it)?;
    let program_ai = next_account_info(it)?;
    let program_data_ai = next_account_info(it)?;

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
    if !payer.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    // The initializer must be the program's upgrade authority (the deployer).
    require_upgrade_authority(program_id, program_ai, program_data_ai, payer.key)?;

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

/// Accounts: [config(w), asset, payer(s), user_token(w), vault(w), spl_token_program]
fn process_send(program_id: &Pubkey, accounts: &[AccountInfo], args: SendArgs) -> ProgramResult {
    if args.amount == 0 {
        return Err(GateError::ZeroAmount.into());
    }
    if args.receiver.len() != 20 && args.receiver.len() != 32 {
        return Err(GateError::BadReceiver.into());
    }
    let it = &mut accounts.iter();
    let config_ai = next_account_info(it)?;
    let asset_ai = next_account_info(it)?;
    let payer = next_account_info(it)?;
    let user_token = next_account_info(it)?;
    let vault = next_account_info(it)?;
    let token_program = next_account_info(it)?;

    // C1: bind the chain id + nonce sequence to the canonical config, so a forged
    // config can't rewrite them and desync the off-chain nonce/submissionId.
    let mut cfg = load_config(program_id, config_ai)?;

    // C1: bind the locked asset to the canonical registry for this debridge_id.
    // Without this a caller could lock a worthless token against a debridge_id
    // that pays out a valuable one on the destination. The token program must be
    // the real SPL token program, and the vault must be exactly the registered
    // one holding the registered mint.
    let asset = load_asset(program_id, &args.debridge_id, asset_ai)?;
    let (vault_mint, _vault_owner) = spl_mint_and_owner(vault, token_program.key)?;
    let (user_mint, _user_owner) = spl_mint_and_owner(user_token, token_program.key)?;
    verify_asset_binding(&asset, token_program.key, vault.key, &vault_mint, &user_mint)?;

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

    // Emit the Sent event as structured program data for the validator's source,
    // carrying the registered mint as the locked asset identity (H5).
    emit_sent(&id, &args, cfg.chain_id, nonce, &native_sender, &asset.mint.to_bytes());

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
    let asset_ai = next_account_info(it)?;
    let executed_ai = next_account_info(it)?;
    let payer = next_account_info(it)?;
    let vault = next_account_info(it)?;
    let receiver_token = next_account_info(it)?;
    let vault_authority = next_account_info(it)?;
    let token_program = next_account_info(it)?;
    let system_program = next_account_info(it)?;

    // C1: only the canonical program-owned config may drive signature checks.
    let cfg = load_config(program_id, config_ai)?;

    // C1: the vault a claim releases from must be the one the program registered
    // for the SIGNED debridge_id — not an arbitrary vault under the global
    // vault-authority PDA. Also pin the SPL token program and prove the receiver
    // and vault hold the registered mint, so a forged/mismatched-config claim
    // can't drain a different asset's vault to the signed receiver.
    let asset = load_asset(program_id, &args.debridge_id, asset_ai)?;

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

    // The vault and the receiver's token account must both hold the registered
    // mint under the real SPL token program, and the vault must be the registered
    // one (the receiver account's owner is free — the signed receiver key IS the
    // account, verified above).
    let (vault_mint, _vault_owner) = spl_mint_and_owner(vault, token_program.key)?;
    let (recv_mint, _recv_owner) = spl_mint_and_owner(receiver_token, token_program.key)?;
    verify_asset_binding(&asset, token_program.key, vault.key, &vault_mint, &recv_mint)?;

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

/// Accounts: [config, owner(s,w), asset_pda(w), mint, vault, spl_token_program,
///            system_program]
///
/// C1: owner-gated binding of a `debridge_id` to the SPL `mint` + `vault` that
/// may back it. Creating the `["asset", debridge_id]` PDA is what later lets
/// `send`/`claim` refuse any vault/mint that isn't the one governance signed off
/// on. The vault must hold `mint` and be owned by the canonical vault-authority
/// PDA, so a claim's `invoke_signed` can actually move its funds.
fn process_register_asset(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    debridge_id: [u8; 32],
) -> ProgramResult {
    let it = &mut accounts.iter();
    let config_ai = next_account_info(it)?;
    let owner = next_account_info(it)?;
    let asset_ai = next_account_info(it)?;
    let mint = next_account_info(it)?;
    let vault = next_account_info(it)?;
    let token_program = next_account_info(it)?;
    let system_program = next_account_info(it)?;

    let cfg = load_config(program_id, config_ai)?;
    if owner.key != &cfg.owner || !owner.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if token_program.key != &spl_token::id() {
        return Err(ProgramError::IncorrectProgramId);
    }
    // The mint must be an SPL mint owned by the token program.
    if mint.owner != token_program.key {
        msg!("register: mint is not owned by the SPL token program");
        return Err(ProgramError::IllegalOwner);
    }
    spl_token::state::Mint::unpack(&mint.data.borrow())
        .map_err(|_| ProgramError::InvalidAccountData)?;

    // The vault must hold this mint and be controlled by the canonical
    // vault-authority PDA (only then can `claim`'s invoke_signed release it).
    let (vault_mint, vault_owner) = spl_mint_and_owner(vault, token_program.key)?;
    if &vault_mint != mint.key {
        msg!("register: vault mint != mint");
        return Err(ProgramError::InvalidAccountData);
    }
    let (auth, _auth_bump) = Pubkey::find_program_address(&[b"vault_authority"], program_id);
    if vault_owner != auth {
        msg!("register: vault is not owned by the canonical vault_authority PDA");
        return Err(ProgramError::InvalidAccountData);
    }

    let (expected_asset, bump) =
        Pubkey::find_program_address(&[b"asset", &debridge_id], program_id);
    if asset_ai.key != &expected_asset {
        return Err(ProgramError::InvalidSeeds);
    }

    let record = AssetConfig { debridge_id, mint: *mint.key, vault: *vault.key };
    let space: usize = 1 + 32 + 32 + 32; // borsh: debridge_id + mint + vault (+slack)
    if asset_ai.data_is_empty() {
        let rent = Rent::get()?.minimum_balance(space);
        invoke_signed(
            &system_instruction::create_account(
                owner.key,
                asset_ai.key,
                rent,
                space as u64,
                program_id,
            ),
            &[owner.clone(), asset_ai.clone(), system_program.clone()],
            &[&[b"asset", &debridge_id, &[bump]]],
        )?;
    } else if asset_ai.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    record.serialize(&mut &mut asset_ai.data.borrow_mut()[..])?;
    msg!("asset registered for debridge_id");
    Ok(())
}

/// The single, versioned `Sent` event (finding H5). This MUST stay byte-for-byte
/// identical to `bridge_solana::relayer::SentEvent` (Borsh field order + types);
/// the host crate round-trips this exact `sol_log_data` framing in its tests.
///
/// The old code emitted only `id || debridge_id` with no version and in a framing
/// the relayer didn't decode — so Solana→EVM scanning could never work and a
/// validator couldn't reconstruct the submissionId. This carries every
/// hash-bound field plus the locked `mint` (asset identity).
#[derive(BorshSerialize)]
struct SentEvent {
    version: u8,
    submission_id: [u8; 32],
    debridge_id: [u8; 32],
    mint: [u8; 32],
    amount: u64,
    chain_id_from: u64,
    chain_id_to: u64,
    nonce: u64,
    receiver: Vec<u8>,
    native_sender: Vec<u8>,
    auto: Option<AutoParamsWire>,
}

const SENT_EVENT_TAG: &[u8] = b"BRIDGE_SENT";
const SENT_EVENT_VERSION: u8 = 1;

/// Emit the `Sent` event via `sol_log_data` (base64 program data in the tx logs)
/// so the validator's Solana source can decode it with
/// `bridge_solana::relayer::parse_sent_log_line`.
fn emit_sent(
    id: &[u8; 32],
    args: &SendArgs,
    chain_id_from: u64,
    nonce: u64,
    native_sender: &[u8],
    mint: &[u8; 32],
) {
    let event = SentEvent {
        version: SENT_EVENT_VERSION,
        submission_id: *id,
        debridge_id: args.debridge_id,
        mint: *mint,
        amount: args.amount,
        chain_id_from,
        chain_id_to: args.chain_id_to,
        nonce,
        receiver: args.receiver.clone(),
        native_sender: native_sender.to_vec(),
        auto: args.auto.clone(),
    };
    let bytes = borsh::to_vec(&event).expect("borsh serialize SentEvent");
    solana_program::log::sol_log_data(&[SENT_EVENT_TAG, &bytes]);
}

fn bs58_id(id: &[u8; 32]) -> String {
    // small helper; solana-program pulls in bs58 transitively
    solana_program::pubkey::Pubkey::new_from_array(*id).to_string()
}

// ---------------------------------------------------------------------------
// C1 authorization tests (host-runnable — no SBF VM needed).
//
// These exercise the *pure* authorization predicates the on-chain handlers route
// through, reproducing the C1 attack scenarios (config takeover, forged-config
// vault drain, unauthorized init) and proving each is refused. A full
// `solana-program-test` account-level harness additionally requires the SBF
// toolchain (cargo-build-sbf), which isn't present in this environment; the
// predicate-level coverage here is the runnable evidence.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod c1_tests {
    use super::*;

    fn pid() -> Pubkey {
        Pubkey::new_unique()
    }

    // A config account is trusted ONLY when it is the canonical ["config"] PDA
    // and program-owned. Reproduces the takeover: an attacker's own config
    // account (their key, their ownership) is refused.
    #[test]
    fn forged_config_account_is_rejected() {
        let program_id = pid();
        let (canonical, _) = Pubkey::find_program_address(&[b"config"], &program_id);

        // Genuine config: canonical PDA, program-owned -> accepted.
        assert!(verify_config_account(&canonical, &program_id, &program_id).is_ok());

        // Attack 1: attacker-created account (not the PDA), owned by the program.
        let attacker_key = Pubkey::new_unique();
        assert_eq!(
            verify_config_account(&attacker_key, &program_id, &program_id),
            Err(ProgramError::InvalidSeeds)
        );

        // Attack 2: the canonical *address* but not program-owned (an account the
        // attacker owns) — the forged-config drain vector.
        let attacker_owner = Pubkey::new_unique();
        assert_eq!(
            verify_config_account(&canonical, &attacker_owner, &program_id),
            Err(ProgramError::IllegalOwner)
        );
    }

    // The asset registry PDA is trusted only when canonical for its debridge_id
    // and program-owned.
    #[test]
    fn forged_asset_account_is_rejected() {
        let program_id = pid();
        let debridge_id = [0x11u8; 32];
        let (canonical, _) =
            Pubkey::find_program_address(&[b"asset", &debridge_id], &program_id);

        assert!(verify_asset_account(&canonical, &program_id, &debridge_id, &program_id).is_ok());

        // Wrong debridge_id -> different PDA -> rejected.
        let other = [0x22u8; 32];
        let (canonical_other, _) =
            Pubkey::find_program_address(&[b"asset", &other], &program_id);
        assert_eq!(
            verify_asset_account(&canonical_other, &program_id, &debridge_id, &program_id),
            Err(ProgramError::InvalidSeeds)
        );

        // Right PDA but attacker-owned -> rejected.
        let attacker = Pubkey::new_unique();
        assert_eq!(
            verify_asset_account(&canonical, &attacker, &debridge_id, &program_id),
            Err(ProgramError::IllegalOwner)
        );
    }

    // The forged-config vault drain: even with a threshold-signed submission, a
    // claim can only release from the vault registered for the SIGNED asset, of
    // the registered mint, under the real SPL token program.
    #[test]
    fn asset_binding_blocks_wrong_vault_or_mint() {
        let mint = Pubkey::new_unique();
        let vault = Pubkey::new_unique();
        let asset = AssetConfig { debridge_id: [1; 32], mint, vault };
        let spl = spl_token::id();

        // Honest release: registered vault, registered mint on both sides.
        assert!(verify_asset_binding(&asset, &spl, &vault, &mint, &mint).is_ok());

        // Attack: a different vault (another asset's) under the global authority.
        let other_vault = Pubkey::new_unique();
        assert_eq!(
            verify_asset_binding(&asset, &spl, &other_vault, &mint, &mint),
            Err(ProgramError::InvalidAccountData)
        );

        // Attack: vault holds a different mint than registered.
        let other_mint = Pubkey::new_unique();
        assert_eq!(
            verify_asset_binding(&asset, &spl, &vault, &other_mint, &mint),
            Err(ProgramError::InvalidAccountData)
        );

        // Attack: receiver token account is a different mint.
        assert_eq!(
            verify_asset_binding(&asset, &spl, &vault, &mint, &other_mint),
            Err(ProgramError::InvalidAccountData)
        );

        // Attack: a fake token program.
        let fake_prog = Pubkey::new_unique();
        assert_eq!(
            verify_asset_binding(&asset, &fake_prog, &vault, &mint, &mint),
            Err(ProgramError::IncorrectProgramId)
        );
    }

    // Atomic/authorized init: only the program's upgrade authority (deployer) may
    // initialize; a front-runner or an immutable program is refused.
    #[test]
    fn init_requires_upgrade_authority() {
        let deployer = Pubkey::new_unique();
        assert!(verify_upgrade_authority(Some(deployer), &deployer).is_ok());

        // A front-runner who is not the upgrade authority.
        let attacker = Pubkey::new_unique();
        assert_eq!(
            verify_upgrade_authority(Some(deployer), &attacker),
            Err(ProgramError::MissingRequiredSignature)
        );

        // Immutable program (authority renounced) — can't be safely initialized.
        assert_eq!(
            verify_upgrade_authority(None, &deployer),
            Err(ProgramError::InvalidArgument)
        );
    }
}
