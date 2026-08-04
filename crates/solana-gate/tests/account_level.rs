//! Account-level tests — the handlers actually EXECUTED, not just their pure
//! predicates asserted.
//!
//! `solana-program-test` runs `process_instruction` natively inside a real test
//! bank (no SBF toolchain needed), so rent, lamports, account ownership, PDA
//! derivation and Borsh (de)serialization behave as they do on-chain. The
//! `c1_tests` module in `lib.rs` covers the pure authorization *rules*; this file
//! covers the handlers that apply them.
//!
//! ## Scope, and two honest exclusions
//!
//! **`init` is not covered here.** `process_init` reads the BPF-loader `Program`
//! and `ProgramData` accounts to identify the upgrade authority. Installing a
//! `bpf_loader_upgradeable`-owned account at the program's own address defeats
//! `ProgramTest`'s builtin dispatch — the runtime then tries to load a real ELF
//! from the (fake) ProgramData and the instruction never reaches our code. Testing
//! it needs a genuine `cargo build-sbf` artifact. The authority rule itself is
//! covered by `c1_tests::init_requires_upgrade_authority`. Tests below therefore
//! seed the config account directly, exactly as a successful `init` would leave it.
//!
//! **Nothing past an SPL CPI is covered.** `register_asset`, `send`'s lock and
//! `claim`'s release all `invoke` into the SPL token program, which is not in this
//! bank. So H-2's `create_marker` — the transfer/allocate/assign on a
//! griefer-funded PDA — is still unexecuted; it sits behind `claim`'s asset checks.
//! Closing that needs `spl_token.so` added to the bank.
//!
//! What IS executed below: `process_register_corridor`, `process_set_paused`,
//! `process_set_guardian`, `process_set_validator`, and `send`'s pause + corridor
//! guards — i.e. the live paths for findings H-3, L-3 and M-1.

use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::instruction::{AccountMeta, Instruction};
use solana_program::pubkey::Pubkey;
use solana_program_test::{processor, ProgramTest, ProgramTestContext};
use solana_sdk::account::Account;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::transaction::Transaction;

use solana_gate::{process_instruction, Config, GateInstruction, SendArgs};

const PROGRAM_ID: Pubkey = Pubkey::new_from_array([7u8; 32]);
const CHAIN_ID: u64 = 7565164; // Solana
const DEST_CHAIN: u64 = 1337;

fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"config"], &PROGRAM_ID).0
}

/// Mirrors `config_space` in the program: the account is sized for the DECLARED
/// capacities, which is the H-3 fix.
fn config_space(validators: u32, corridors: u32) -> usize {
    32 + 32 + (4 + 20 * validators as usize) + 4 + 8 + 1 + 4 + 4 + (4 + 16 * corridors as usize)
}

/// A bank with the gate registered and its config PDA already initialized —
/// owner-gated instructions are then driven by `owner`, which is funded.
async fn setup(max_validators: u32, max_corridors: u32, guardian: Pubkey) -> (ProgramTestContext, Keypair) {
    let owner = Keypair::new();
    let mut pt = ProgramTest::new("solana_gate", PROGRAM_ID, processor!(process_instruction));

    pt.add_account(
        owner.pubkey(),
        Account {
            lamports: 10_000_000_000,
            data: vec![],
            owner: solana_sdk::system_program::id(),
            executable: false,
            rent_epoch: 0,
        },
    );

    // Exactly what a successful `init` leaves behind.
    let cfg = Config {
        owner: owner.pubkey(),
        guardian,
        validators: vec![[1u8; 20], [2u8; 20], [3u8; 20]],
        threshold: 2,
        chain_id: CHAIN_ID,
        paused: false,
        max_validators,
        max_corridors,
        nonce_to: Vec::new(),
    };
    let space = config_space(max_validators, max_corridors);
    let mut data = vec![0u8; space];
    cfg.serialize(&mut &mut data[..]).expect("config must fit the space it declares");

    pt.add_account(
        config_pda(),
        Account {
            lamports: 10_000_000_000,
            data,
            owner: PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        },
    );

    (pt.start_with_context().await, owner)
}

fn ix(data: GateInstruction, accounts: Vec<AccountMeta>) -> Instruction {
    Instruction { program_id: PROGRAM_ID, accounts, data: borsh::to_vec(&data).unwrap() }
}

async fn exec(
    ctx: &mut ProgramTestContext,
    instruction: Instruction,
    extra: &[&Keypair],
) -> Result<(), solana_sdk::transaction::TransactionError> {
    let mut signers: Vec<&Keypair> = vec![&ctx.payer];
    signers.extend_from_slice(extra);
    let tx = Transaction::new_signed_with_payer(
        &[instruction],
        Some(&ctx.payer.pubkey()),
        &signers,
        ctx.last_blockhash,
    );
    ctx.banks_client.process_transaction(tx).await.map_err(|e| match e {
        solana_program_test::BanksClientError::TransactionError(te) => te,
        other => panic!("unexpected banks error: {other:?}"),
    })
}

async fn read_config(ctx: &mut ProgramTestContext) -> Config {
    let acct = ctx.banks_client.get_account(config_pda()).await.unwrap().expect("config exists");
    assert_eq!(acct.owner, PROGRAM_ID, "config must stay program-owned");
    Config::deserialize(&mut &acct.data[..]).expect("config must deserialize")
}

fn register_corridor(who: Pubkey, chain_id_to: u64) -> Instruction {
    ix(
        GateInstruction::RegisterCorridor { chain_id_to },
        vec![AccountMeta::new(config_pda(), false), AccountMeta::new(who, true)],
    )
}

/// Six accounts so `send` clears `next_account_info`. The pause and corridor
/// guards both fire before any of them is dereferenced, which is what lets these
/// tests reach the guards without the SPL token program.
fn send_instruction(signer: Pubkey, chain_id_to: u64) -> Instruction {
    ix(
        GateInstruction::Send(SendArgs {
            debridge_id: [9u8; 32],
            amount: 1,
            chain_id_to,
            receiver: vec![0xAB; 20],
            auto: None,
        }),
        vec![
            AccountMeta::new(config_pda(), false),
            AccountMeta::new_readonly(Pubkey::new_unique(), false), // asset
            AccountMeta::new(signer, true),
            AccountMeta::new(Pubkey::new_unique(), false), // user_token
            AccountMeta::new(Pubkey::new_unique(), false), // vault
            AccountMeta::new_readonly(spl_token::id(), false),
        ],
    )
}

// ---------------------------------------------------------------------------
// H-3 — `send` cannot create a corridor; corridors are owner-gated and bounded
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_to_an_unregistered_corridor_is_refused_and_creates_nothing() {
    let (mut ctx, owner) = setup(8, 4, Pubkey::default()).await;

    // THE H-3 attack: `send` used to append a `(chain_id, nonce)` entry for any
    // destination a caller invented, growing the config until it no longer fit its
    // account — permanently bricking send AND governance, with no realloc path.
    let err = exec(&mut ctx, send_instruction(owner.pubkey(), 424242), &[&owner])
        .await
        .expect_err("an unregistered destination must be refused");
    assert!(format!("{err:?}").contains("Custom"), "got {err:?}");

    // The load-bearing assertion: no entry was created as a side effect.
    let cfg = read_config(&mut ctx).await;
    assert!(cfg.nonce_to.is_empty(), "send must never create a corridor, got {:?}", cfg.nonce_to);
}

#[tokio::test]
async fn corridor_registration_is_owner_gated() {
    let (mut ctx, _owner) = setup(8, 4, Pubkey::default()).await;

    let stranger = Keypair::new();
    let err = exec(&mut ctx, register_corridor(stranger.pubkey(), DEST_CHAIN), &[&stranger])
        .await
        .expect_err("only the owner may register a corridor");
    assert!(format!("{err:?}").contains("MissingRequiredSignature"), "got {err:?}");
    assert!(read_config(&mut ctx).await.nonce_to.is_empty());
}

#[tokio::test]
async fn corridor_registration_is_idempotent_and_never_resets_a_nonce() {
    let (mut ctx, owner) = setup(8, 4, Pubkey::default()).await;

    exec(&mut ctx, register_corridor(owner.pubkey(), DEST_CHAIN), &[&owner]).await.expect("first");
    exec(&mut ctx, register_corridor(owner.pubkey(), DEST_CHAIN), &[&owner]).await.expect("second");

    let cfg = read_config(&mut ctx).await;
    assert_eq!(cfg.nonce_to, vec![(DEST_CHAIN, 0)], "no duplicate entry, nonce untouched");
}

#[tokio::test]
async fn corridors_are_capacity_bounded() {
    // Room for exactly 2 corridors.
    let (mut ctx, owner) = setup(8, 2, Pubkey::default()).await;

    exec(&mut ctx, register_corridor(owner.pubkey(), 1), &[&owner]).await.expect("first fits");
    exec(&mut ctx, register_corridor(owner.pubkey(), 2), &[&owner]).await.expect("second fits");

    // The third must fail cleanly rather than overflow the account — this bound is
    // what makes the H-3 vector impossible even for the owner.
    let err = exec(&mut ctx, register_corridor(owner.pubkey(), 3), &[&owner])
        .await
        .expect_err("registering past max_corridors must fail");
    assert!(format!("{err:?}").contains("Custom"), "got {err:?}");

    let cfg = read_config(&mut ctx).await;
    assert_eq!(cfg.nonce_to.len(), 2, "state unchanged after the rejected registration");
}

// ---------------------------------------------------------------------------
// M-1 — the circuit breaker, executed rather than asserted
// ---------------------------------------------------------------------------

fn pause(who: Pubkey) -> Instruction {
    ix(
        GateInstruction::Pause,
        vec![AccountMeta::new(config_pda(), false), AccountMeta::new_readonly(who, true)],
    )
}

fn unpause(who: Pubkey) -> Instruction {
    ix(
        GateInstruction::Unpause,
        vec![AccountMeta::new(config_pda(), false), AccountMeta::new_readonly(who, true)],
    )
}

#[tokio::test]
async fn a_paused_gate_refuses_send() {
    let (mut ctx, owner) = setup(8, 4, Pubkey::default()).await;
    // Register the corridor first, so a later failure can only be the pause.
    exec(&mut ctx, register_corridor(owner.pubkey(), DEST_CHAIN), &[&owner]).await.unwrap();

    // Un-paused: the corridor guard passes and we get as far as the SPL asset
    // checks, which fail for a different reason — proving the send guard is not
    // what stopped us.
    let before = exec(&mut ctx, send_instruction(owner.pubkey(), DEST_CHAIN), &[&owner]).await;
    let before = format!("{:?}", before.expect_err("dummy SPL accounts must fail"));
    assert!(!before.contains("Custom(7)"), "should not be the Paused error yet: {before}");

    exec(&mut ctx, pause(owner.pubkey()), &[&owner]).await.expect("owner pause");
    assert!(read_config(&mut ctx).await.paused, "the flag must actually be persisted");

    // Paused: now it stops before any of that. `Config.paused` used to be dead
    // code — written false at init, never read, with no instruction to set it.
    let err = exec(&mut ctx, send_instruction(owner.pubkey(), DEST_CHAIN), &[&owner])
        .await
        .expect_err("a paused gate must refuse send");
    assert!(format!("{err:?}").contains("Custom"), "got {err:?}");
}

#[tokio::test]
async fn a_guardian_may_stop_the_gate_but_never_restart_it() {
    let guardian = Keypair::new();
    let (mut ctx, owner) = setup(8, 4, guardian.pubkey()).await;

    // Fund the guardian so signing is possible.
    let fund = solana_sdk::system_instruction::transfer(
        &ctx.payer.pubkey(),
        &guardian.pubkey(),
        1_000_000_000,
    );
    let tx = Transaction::new_signed_with_payer(
        &[fund],
        Some(&ctx.payer.pubkey()),
        &[&ctx.payer],
        ctx.last_blockhash,
    );
    ctx.banks_client.process_transaction(tx).await.unwrap();

    // The guardian is a low-trust STOP button: it may halt...
    exec(&mut ctx, pause(guardian.pubkey()), &[&guardian]).await.expect("guardian may pause");
    assert!(read_config(&mut ctx).await.paused);

    // ...but never resume, so a compromised guardian causes only a recoverable
    // liveness halt, never a restart of a gate the owner deliberately stopped.
    let err = exec(&mut ctx, unpause(guardian.pubkey()), &[&guardian])
        .await
        .expect_err("a guardian must not resume the gate");
    assert!(format!("{err:?}").contains("MissingRequiredSignature"), "got {err:?}");
    assert!(read_config(&mut ctx).await.paused, "still paused after the refused unpause");

    // The owner can.
    exec(&mut ctx, unpause(owner.pubkey()), &[&owner]).await.expect("owner may resume");
    assert!(!read_config(&mut ctx).await.paused);
}

#[tokio::test]
async fn a_stranger_can_neither_pause_nor_unpause() {
    let (mut ctx, _owner) = setup(8, 4, Pubkey::default()).await;
    let stranger = Keypair::new();
    let fund = solana_sdk::system_instruction::transfer(
        &ctx.payer.pubkey(),
        &stranger.pubkey(),
        1_000_000_000,
    );
    let tx = Transaction::new_signed_with_payer(
        &[fund],
        Some(&ctx.payer.pubkey()),
        &[&ctx.payer],
        ctx.last_blockhash,
    );
    ctx.banks_client.process_transaction(tx).await.unwrap();

    for instruction in [pause(stranger.pubkey()), unpause(stranger.pubkey())] {
        let err = exec(&mut ctx, instruction, &[&stranger]).await.expect_err("stranger refused");
        assert!(format!("{err:?}").contains("MissingRequiredSignature"), "got {err:?}");
    }
    assert!(!read_config(&mut ctx).await.paused);
}

// ---------------------------------------------------------------------------
// L-3 — the validator set cannot outgrow its account
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validator_set_is_capped_at_the_declared_capacity() {
    // Room for 4; the seeded config holds 3.
    let (mut ctx, owner) = setup(4, 4, Pubkey::default()).await;

    let add = |v: u8| {
        ix(
            GateInstruction::SetValidator { validator: [v; 20], active: true },
            vec![
                AccountMeta::new(config_pda(), false),
                AccountMeta::new_readonly(owner.pubkey(), true),
            ],
        )
    };

    exec(&mut ctx, add(4), &[&owner]).await.expect("the 4th fits");
    assert_eq!(read_config(&mut ctx).await.validators.len(), 4);

    // The 5th must be refused explicitly rather than blowing up inside Borsh once
    // the buffer overflows — that opaque failure was finding L-3.
    let err = exec(&mut ctx, add(5), &[&owner]).await.expect_err("past capacity must fail");
    assert!(format!("{err:?}").contains("Custom"), "got {err:?}");
    assert_eq!(read_config(&mut ctx).await.validators.len(), 4, "state unchanged");
}
