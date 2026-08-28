//! The swap handlers actually EXECUTED, not just their pure math asserted.
//!
//! `solana-program-test` runs `process_instruction` natively in a real test bank
//! and bundles the SPL token program, so the CPIs, PDA signing, rent and Borsh
//! round-trips all behave as they do on-chain.
//!
//! `Init` is not covered here, for the same reason the gate's `init` is not: it
//! reads the BPF-loader `Program`/`ProgramData` accounts to identify the upgrade
//! authority, and installing a loader-owned account at the program's own address
//! defeats `ProgramTest`'s builtin dispatch. These tests seed the pool and token
//! records exactly as a successful `Init` leaves them.

use borsh::BorshDeserialize;
use solana_program::instruction::{AccountMeta, Instruction};
use solana_program::program_option::COption;
use solana_program::program_pack::Pack;
use solana_program::pubkey::Pubkey;
use solana_program_test::{processor, ProgramTest, ProgramTestContext};
use solana_sdk::account::Account;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::transaction::Transaction;

use solana_swap::{
    math, process_instruction, Pool, SwapInstruction, TokenRec, POOL_SEED, PRICE_ONE, TOKEN_SEED,
    VAULT_AUTHORITY_SEED,
};

const PROGRAM_ID: Pubkey = Pubkey::new_from_array([11u8; 32]);
const HUB_DEC: u8 = 9;
const ALT_DEC: u8 = 9;
/// 3180 hub units per ALT, PRICE_ONE-scaled — the same peg the EVM pools use.
const ALT_PRICE: u128 = 3180 * PRICE_ONE;

fn pool_pda() -> Pubkey {
    Pubkey::find_program_address(&[POOL_SEED], &PROGRAM_ID).0
}
fn token_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[TOKEN_SEED, mint.as_ref()], &PROGRAM_ID).0
}
fn vault_authority() -> Pubkey {
    Pubkey::find_program_address(&[VAULT_AUTHORITY_SEED], &PROGRAM_ID).0
}

fn mint_account(decimals: u8) -> Account {
    let mut data = vec![0u8; spl_token::state::Mint::LEN];
    spl_token::state::Mint {
        mint_authority: COption::None,
        supply: u64::MAX / 2,
        decimals,
        is_initialized: true,
        freeze_authority: COption::None,
    }
    .pack_into_slice(&mut data);
    Account { lamports: 10_000_000, data, owner: spl_token::id(), executable: false, rent_epoch: 0 }
}

fn token_account(mint: Pubkey, owner: Pubkey, amount: u64) -> Account {
    let mut data = vec![0u8; spl_token::state::Account::LEN];
    spl_token::state::Account {
        mint,
        owner,
        amount,
        delegate: COption::None,
        state: spl_token::state::AccountState::Initialized,
        is_native: COption::None,
        delegated_amount: 0,
        close_authority: COption::None,
    }
    .pack_into_slice(&mut data);
    Account { lamports: 10_000_000, data, owner: spl_token::id(), executable: false, rent_epoch: 0 }
}

fn program_account<T: borsh::BorshSerialize>(v: &T, space: usize) -> Account {
    let mut data = borsh::to_vec(v).unwrap();
    data.resize(space, 0);
    Account { lamports: 10_000_000, data, owner: PROGRAM_ID, executable: false, rent_epoch: 0 }
}

struct Fx {
    ctx: ProgramTestContext,
    owner: Keypair,
    user: Keypair,
    hub_mint: Pubkey,
    alt_mint: Pubkey,
    hub_vault: Pubkey,
    alt_vault: Pubkey,
    user_hub: Pubkey,
    user_alt: Pubkey,
}

/// A pool with both sides listed and seeded, and a user holding hub tokens.
async fn setup(hub_reserve: u64, alt_reserve: u64, user_hub_balance: u64, fee_bps: u16) -> Fx {
    let owner = Keypair::new();
    let user = Keypair::new();
    let hub_mint = Pubkey::new_unique();
    let alt_mint = Pubkey::new_unique();
    let hub_vault = Pubkey::new_unique();
    let alt_vault = Pubkey::new_unique();
    let user_hub = Pubkey::new_unique();
    let user_alt = Pubkey::new_unique();

    let mut pt = ProgramTest::new("solana_swap", PROGRAM_ID, processor!(process_instruction));
    for k in [owner.pubkey(), user.pubkey()] {
        pt.add_account(
            k,
            Account {
                lamports: 10_000_000_000,
                data: vec![],
                owner: solana_sdk::system_program::id(),
                executable: false,
                rent_epoch: 0,
            },
        );
    }
    pt.add_account(hub_mint, mint_account(HUB_DEC));
    pt.add_account(alt_mint, mint_account(ALT_DEC));
    pt.add_account(hub_vault, token_account(hub_mint, vault_authority(), hub_reserve));
    pt.add_account(alt_vault, token_account(alt_mint, vault_authority(), alt_reserve));
    pt.add_account(user_hub, token_account(hub_mint, user.pubkey(), user_hub_balance));
    pt.add_account(user_alt, token_account(alt_mint, user.pubkey(), 0));

    pt.add_account(
        pool_pda(),
        program_account(
            &Pool {
                owner: owner.pubkey().to_bytes(),
                oracle: owner.pubkey().to_bytes(),
                guardian: [0u8; 32],
                hub_mint: hub_mint.to_bytes(),
                fee_bps,
                max_price_deviation_bps: 1000,
                min_price_update_interval: 3600,
                paused: false,
            },
            160,
        ),
    );
    // Reserves match the vault balances, as a real seed would leave them.
    pt.add_account(
        token_pda(&hub_mint),
        program_account(
            &TokenRec {
                mint: hub_mint.to_bytes(),
                vault: hub_vault.to_bytes(),
                decimals: HUB_DEC,
                price: PRICE_ONE,
                reserve: hub_reserve,
                last_price_update: 0,
                listed: true,
            },
            160,
        ),
    );
    pt.add_account(
        token_pda(&alt_mint),
        program_account(
            &TokenRec {
                mint: alt_mint.to_bytes(),
                vault: alt_vault.to_bytes(),
                decimals: ALT_DEC,
                price: ALT_PRICE,
                reserve: alt_reserve,
                last_price_update: 0,
                listed: true,
            },
            160,
        ),
    );

    let ctx = pt.start_with_context().await;
    Fx { ctx, owner, user, hub_mint, alt_mint, hub_vault, alt_vault, user_hub, user_alt }
}

fn swap_ix(fx: &Fx, amount_in: u64, min_out: u64, reverse: bool) -> Instruction {
    let (rec_in, rec_out, user_in, user_out, vault_in, vault_out, mint_in, mint_out) = if reverse {
        (
            token_pda(&fx.alt_mint), token_pda(&fx.hub_mint), fx.user_alt, fx.user_hub,
            fx.alt_vault, fx.hub_vault, fx.alt_mint, fx.hub_mint,
        )
    } else {
        (
            token_pda(&fx.hub_mint), token_pda(&fx.alt_mint), fx.user_hub, fx.user_alt,
            fx.hub_vault, fx.alt_vault, fx.hub_mint, fx.alt_mint,
        )
    };
    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(pool_pda(), false),
            AccountMeta::new_readonly(fx.user.pubkey(), true),
            AccountMeta::new(rec_in, false),
            AccountMeta::new(rec_out, false),
            AccountMeta::new(user_in, false),
            AccountMeta::new(user_out, false),
            AccountMeta::new(vault_in, false),
            AccountMeta::new(vault_out, false),
            AccountMeta::new_readonly(mint_in, false),
            AccountMeta::new_readonly(mint_out, false),
            AccountMeta::new_readonly(vault_authority(), false),
            AccountMeta::new_readonly(spl_token::id(), false),
        ],
        data: SwapInstruction::Swap { amount_in, min_amount_out: min_out }.to_bytes(),
    }
}

async fn send(ctx: &mut ProgramTestContext, ix: Instruction, signer: &Keypair) -> Result<(), String> {
    let bh = ctx.banks_client.get_latest_blockhash().await.unwrap();
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&signer.pubkey()), &[signer], bh);
    ctx.banks_client.process_transaction(tx).await.map_err(|e| e.to_string())
}

async fn spl_amount(ctx: &mut ProgramTestContext, key: Pubkey) -> u64 {
    let acct = ctx.banks_client.get_account(key).await.unwrap().unwrap();
    spl_token::state::Account::unpack(&acct.data).unwrap().amount
}

async fn rec(ctx: &mut ProgramTestContext, mint: Pubkey) -> TokenRec {
    let acct = ctx.banks_client.get_account(token_pda(&mint)).await.unwrap().unwrap();
    TokenRec::deserialize(&mut &acct.data[..]).unwrap()
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn swap_pays_out_the_quoted_amount_and_books_both_reserves() {
    // 1000 hub units in, at 1.0 / 3180 => 0.314465408 ALT.
    let amount_in = 1_000_000_000_000u64;
    let expected = math::amount_out(amount_in, PRICE_ONE, HUB_DEC, ALT_PRICE, ALT_DEC, 0).unwrap();

    let mut fx = setup(0, 1_000_000_000_000, amount_in, 0).await;
    let ix = swap_ix(&fx, amount_in, 0, false);
    let user = fx.user.insecure_clone();
    send(&mut fx.ctx, ix, &user).await.expect("swap should succeed");

    assert_eq!(spl_amount(&mut fx.ctx, fx.user_alt).await, expected, "user received the quote");
    assert_eq!(spl_amount(&mut fx.ctx, fx.user_hub).await, 0, "input was taken");
    // The reserve is INTERNAL accounting and must track the vaults exactly.
    assert_eq!(rec(&mut fx.ctx, fx.hub_mint).await.reserve, amount_in);
    assert_eq!(rec(&mut fx.ctx, fx.alt_mint).await.reserve, 1_000_000_000_000 - expected);
    assert_eq!(spl_amount(&mut fx.ctx, fx.hub_vault).await, amount_in);
}

#[tokio::test]
async fn a_swap_can_never_drain_more_than_the_reserve() {
    // The output side holds one unit less than the swap would pay out.
    let amount_in = 1_000_000_000_000u64;
    let out = math::amount_out(amount_in, PRICE_ONE, HUB_DEC, ALT_PRICE, ALT_DEC, 0).unwrap();
    let mut fx = setup(0, out - 1, amount_in, 0).await;
    let ix = swap_ix(&fx, amount_in, 0, false);
    let user = fx.user.insecure_clone();
    let err = send(&mut fx.ctx, ix, &user).await.expect_err("must refuse to overdraw the lock");
    assert!(err.contains("custom program error"), "got: {err}");
    assert_eq!(spl_amount(&mut fx.ctx, fx.user_alt).await, 0, "nothing paid out");
}

#[tokio::test]
async fn slippage_bound_is_enforced() {
    let amount_in = 1_000_000_000_000u64;
    let out = math::amount_out(amount_in, PRICE_ONE, HUB_DEC, ALT_PRICE, ALT_DEC, 0).unwrap();
    let mut fx = setup(0, 1_000_000_000_000, amount_in, 0).await;
    let ix = swap_ix(&fx, amount_in, out + 1, false); // ask for one more than the quote
    let user = fx.user.insecure_clone();
    send(&mut fx.ctx, ix, &user).await.expect_err("must refuse below the caller's minimum");
    assert_eq!(spl_amount(&mut fx.ctx, fx.user_alt).await, 0);
}

#[tokio::test]
async fn the_fee_is_retained_as_reserve_not_paid_out() {
    let amount_in = 1_000_000_000_000u64;
    let gross = math::amount_out(amount_in, PRICE_ONE, HUB_DEC, ALT_PRICE, ALT_DEC, 0).unwrap();
    let net = math::amount_out(amount_in, PRICE_ONE, HUB_DEC, ALT_PRICE, ALT_DEC, 30).unwrap();
    assert!(net < gross, "a 30bps fee must reduce the output");

    let mut fx = setup(0, 1_000_000_000_000, amount_in, 30).await;
    let ix = swap_ix(&fx, amount_in, 0, false);
    let user = fx.user.insecure_clone();
    send(&mut fx.ctx, ix, &user).await.expect("swap should succeed");

    assert_eq!(spl_amount(&mut fx.ctx, fx.user_alt).await, net);
    // The input side grew by the WHOLE input while the output side shrank by
    // only the net — that difference is the fee, kept in the pool.
    assert_eq!(rec(&mut fx.ctx, fx.hub_mint).await.reserve, amount_in);
    assert_eq!(rec(&mut fx.ctx, fx.alt_mint).await.reserve, 1_000_000_000_000 - net);
}

#[tokio::test]
async fn a_paused_pool_refuses_swaps() {
    let amount_in = 1_000_000_000u64;
    let mut fx = setup(0, 1_000_000_000_000, amount_in, 0).await;

    let pause = Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(pool_pda(), false),
            AccountMeta::new_readonly(fx.owner.pubkey(), true),
        ],
        data: SwapInstruction::Pause.to_bytes(),
    };
    let owner = fx.owner.insecure_clone();
    send(&mut fx.ctx, pause, &owner).await.expect("owner may pause");

    let ix = swap_ix(&fx, amount_in, 0, false);
    let user = fx.user.insecure_clone();
    send(&mut fx.ctx, ix, &user).await.expect_err("a paused pool must refuse");

    let unpause = Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(pool_pda(), false),
            AccountMeta::new_readonly(fx.owner.pubkey(), true),
        ],
        data: SwapInstruction::Unpause.to_bytes(),
    };
    send(&mut fx.ctx, unpause, &owner).await.expect("owner may unpause");
    let ix = swap_ix(&fx, amount_in, 0, false);
    send(&mut fx.ctx, ix, &user).await.expect("swaps resume");
}

#[tokio::test]
async fn only_the_owner_may_unpause() {
    let mut fx = setup(0, 1_000, 1_000, 0).await;
    let stranger = Keypair::new();
    fx.ctx.banks_client.get_account(stranger.pubkey()).await.ok();
    let ix = Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(pool_pda(), false),
            AccountMeta::new_readonly(fx.user.pubkey(), true),
        ],
        data: SwapInstruction::Unpause.to_bytes(),
    };
    let user = fx.user.insecure_clone();
    send(&mut fx.ctx, ix, &user).await.expect_err("a non-owner must not release the breaker");
}

#[tokio::test]
async fn the_hub_price_can_never_be_moved() {
    let mut fx = setup(0, 1_000, 1_000, 0).await;
    let ix = Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(pool_pda(), false),
            AccountMeta::new_readonly(fx.owner.pubkey(), true),
            AccountMeta::new(token_pda(&fx.hub_mint), false),
        ],
        data: SwapInstruction::SetPrice { price: 2 * PRICE_ONE }.to_bytes(),
    };
    let owner = fx.owner.insecure_clone();
    send(&mut fx.ctx, ix, &owner).await.expect_err("the unit of account is pinned at 1.0");
    assert_eq!(rec(&mut fx.ctx, fx.hub_mint).await.price, PRICE_ONE);
}

#[tokio::test]
async fn a_price_move_past_the_deviation_cap_is_refused() {
    let mut fx = setup(0, 1_000, 1_000, 0).await;
    let set = |price: u128| Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(pool_pda(), false),
            AccountMeta::new_readonly(fx.owner.pubkey(), true),
            AccountMeta::new(token_pda(&fx.alt_mint), false),
        ],
        data: SwapInstruction::SetPrice { price }.to_bytes(),
    };
    let owner = fx.owner.insecure_clone();
    // The first move after listing is always allowed.
    send(&mut fx.ctx, set(3000 * PRICE_ONE), &owner).await.expect("first reprice allowed");
    assert_eq!(rec(&mut fx.ctx, fx.alt_mint).await.price, 3000 * PRICE_ONE);
    // The second is capped — 10% here, and this asks for +100%.
    send(&mut fx.ctx, set(6000 * PRICE_ONE), &owner)
        .await
        .expect_err("a 100% step past a 10% cap must be refused");
    assert_eq!(rec(&mut fx.ctx, fx.alt_mint).await.price, 3000 * PRICE_ONE, "price unchanged");
}

#[tokio::test]
async fn swapping_a_token_for_itself_is_refused() {
    let amount_in = 1_000u64;
    let mut fx = setup(1_000_000, 1_000_000, amount_in, 0).await;
    let mut ix = swap_ix(&fx, amount_in, 0, false);
    // Point both sides at the hub.
    ix.accounts[3] = AccountMeta::new(token_pda(&fx.hub_mint), false);
    ix.accounts[5] = AccountMeta::new(fx.user_hub, false);
    ix.accounts[7] = AccountMeta::new(fx.hub_vault, false);
    ix.accounts[9] = AccountMeta::new_readonly(fx.hub_mint, false);
    let user = fx.user.insecure_clone();
    send(&mut fx.ctx, ix, &user).await.expect_err("same-token swap must be refused");
}

#[tokio::test]
async fn a_swap_cannot_be_pointed_at_another_assets_vault() {
    // The vault is pinned by the token record, so passing a different (even
    // well-formed) vault must fail rather than release the wrong liquidity.
    let amount_in = 1_000_000_000u64;
    let mut fx = setup(1_000_000_000_000, 1_000_000_000_000, amount_in, 0).await;
    let mut ix = swap_ix(&fx, amount_in, 0, false);
    ix.accounts[7] = AccountMeta::new(fx.hub_vault, false); // out-vault := hub's
    let user = fx.user.insecure_clone();
    send(&mut fx.ctx, ix, &user).await.expect_err("vault must match the token record");
}
