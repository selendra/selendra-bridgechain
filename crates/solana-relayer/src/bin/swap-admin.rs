//! `swap-admin` — the on-chain client for the Solana swap pool.
//!
//! Same role `gate-admin` plays for the bridge gate: it builds and signs
//! transactions, and grants no authority of its own (every governance path is
//! owner- or oracle-gated ON-CHAIN).
//!
//! It lives in `solana-relayer` for the reason that crate exists at all:
//! `solana-client` pins `zeroize <1.4` and alloy needs `^1.5`, so no EVM-side
//! crate can host a Solana client.
//!
//! The instruction enum, account layouts and pricing math are IMPORTED from
//! `solana-swap` rather than mirrored here. Hand-copied definitions are how the
//! gate's two `Sent` structs drifted apart while both sides kept compiling.
//!
//!   swap-admin --rpc <url> --keypair <path> --program <pubkey> <command>
//!
//!     init --hub-mint <pubkey> --hub-vault <pubkey>
//!          [--fee-bps N] [--deviation-bps N] [--min-price-interval SECS]
//!          [--guardian <pubkey>] [--oracle <pubkey>]
//!     list-token --mint <pubkey> --vault <pubkey> --price <PRICE_ONE-scaled>
//!     set-price  --mint <pubkey> --price <PRICE_ONE-scaled>
//!     seed       --mint <pubkey> --amount N --from <token account>
//!     withdraw   --mint <pubkey> --amount N --to <token account>
//!     swap       --mint-in <pubkey> --mint-out <pubkey> --amount N
//!                --from <token account> --to <token account> [--min-out N]
//!     quote      --mint-in <pubkey> --mint-out <pubkey> --amount N
//!     pause | unpause | set-fee --fee-bps N | set-oracle --oracle <pubkey>
//!     show
//!
//! Prices are PRICE_ONE-scaled (1e18), the same fixed point `SwapPool.sol` uses,
//! so a price of "one hub unit" is 1000000000000000000.

use std::str::FromStr;

use borsh::BorshDeserialize;
use solana_client::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{read_keypair_file, Signer};
use solana_sdk::transaction::Transaction;
use solana_swap::{
    math, InitPoolArgs, Pool, SwapInstruction, TokenRec, POOL_SEED, PRICE_ONE, TOKEN_SEED,
    VAULT_AUTHORITY_SEED,
};

/// The shared account layouts hold plain 32-byte keys (so the read API can link
/// them too); this is the display/compare boundary.
fn pk(b: &[u8; 32]) -> Pubkey {
    Pubkey::new_from_array(*b)
}

const SPL_TOKEN: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

/// Minimal flag reader: `--name value`.
struct Args(Vec<String>);
impl Args {
    fn get(&self, name: &str) -> Option<String> {
        self.0.iter().position(|a| a == name).and_then(|i| self.0.get(i + 1)).cloned()
    }
    fn req(&self, name: &str) -> anyhow::Result<String> {
        self.get(name).ok_or_else(|| anyhow::anyhow!("missing required flag {name}"))
    }
    fn key(&self, name: &str) -> anyhow::Result<Pubkey> {
        Ok(Pubkey::from_str(&self.req(name)?)?)
    }
    fn num<T: FromStr>(&self, name: &str, default: T) -> anyhow::Result<T>
    where
        T::Err: std::fmt::Display,
    {
        match self.get(name) {
            None => Ok(default),
            Some(v) => v.parse::<T>().map_err(|e| anyhow::anyhow!("bad {name}: {e}")),
        }
    }
}

fn main() -> anyhow::Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cmd = argv
        .iter()
        .enumerate()
        .find(|(i, a)| {
            !a.starts_with("--") && !argv.get(i.wrapping_sub(1)).is_some_and(|p| p.starts_with("--"))
        })
        .map(|(_, a)| a.clone())
        .ok_or_else(|| anyhow::anyhow!("no command; see the header of this file"))?;
    let args = Args(argv);

    let rpc_url = args.req("--rpc")?;
    let program_id = args.key("--program")?;
    let payer = read_keypair_file(args.req("--keypair")?)
        .map_err(|e| anyhow::anyhow!("reading keypair: {e}"))?;
    let rpc = RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed());
    let token_program = Pubkey::from_str(SPL_TOKEN)?;

    let (pool_pda, _) = Pubkey::find_program_address(&[POOL_SEED], &program_id);
    let (vault_authority, _) = Pubkey::find_program_address(&[VAULT_AUTHORITY_SEED], &program_id);
    let token_pda = |mint: &Pubkey| {
        Pubkey::find_program_address(&[TOKEN_SEED, mint.as_ref()], &program_id).0
    };

    // --- read-only commands -------------------------------------------------
    if cmd == "show" {
        println!("program        : {program_id}");
        println!("pool PDA       : {pool_pda}");
        println!("vault authority: {vault_authority}");
        match rpc.get_account(&pool_pda) {
            Err(_) => println!("pool account   : NOT INITIALIZED (run `init`)"),
            Ok(acct) => {
                let pool = Pool::deserialize(&mut &acct.data[..])?;
                println!("  owner        : {}", pk(&pool.owner));
                println!("  oracle       : {}", pk(&pool.oracle));
                println!(
                    "  guardian     : {}",
                    if pool.guardian == [0u8; 32] { "none".into() } else { pk(&pool.guardian).to_string() }
                );
                println!("  hub mint     : {}", pk(&pool.hub_mint));
                println!("  fee          : {} bps", pool.fee_bps);
                println!("  price guards : max {} bps per {}s", pool.max_price_deviation_bps, pool.min_price_update_interval);
                println!("  paused       : {}", pool.paused);
                // The listed set is not enumerable from the pool account (each
                // token is its own PDA), so `show` reports the ones named on the
                // command line — pass --mint repeatedly to inspect them.
                for m in args.0.iter().enumerate().filter(|(_, a)| a.as_str() == "--mint").filter_map(|(i, _)| args.0.get(i + 1)) {
                    let mint = Pubkey::from_str(m)?;
                    match rpc.get_account(&token_pda(&mint)) {
                        Err(_) => println!("  token {mint}: NOT LISTED"),
                        Ok(a) => {
                            let r = TokenRec::deserialize(&mut &a.data[..])?;
                            println!(
                                "  token {} : price {} ({} dp) reserve {} vault {}",
                                pk(&r.mint), r.price, r.decimals, r.reserve, pk(&r.vault)
                            );
                        }
                    }
                }
            }
        }
        return Ok(());
    }

    if cmd == "quote" {
        let mint_in = args.key("--mint-in")?;
        let mint_out = args.key("--mint-out")?;
        let amount: u64 = args.req("--amount")?.parse()?;
        let pool = Pool::deserialize(&mut &rpc.get_account(&pool_pda)?.data[..])?;
        let ri = TokenRec::deserialize(&mut &rpc.get_account(&token_pda(&mint_in))?.data[..])?;
        let ro = TokenRec::deserialize(&mut &rpc.get_account(&token_pda(&mint_out))?.data[..])?;
        let out = math::amount_out(amount, ri.price, ri.decimals, ro.price, ro.decimals, pool.fee_bps)
            .ok_or_else(|| anyhow::anyhow!("quote overflows"))?;
        println!("{out}");
        if out > ro.reserve {
            eprintln!(
                "warning: {out} exceeds the pool's {} reserve — the swap would hit the lock",
                ro.reserve
            );
        }
        return Ok(());
    }

    // --- transactions -------------------------------------------------------
    let (data, accounts) = match cmd.as_str() {
        "init" => {
            let hub_mint = args.key("--hub-mint")?;
            let hub_vault = args.key("--hub-vault")?;
            let (program_data, _) = Pubkey::find_program_address(
                &[program_id.as_ref()],
                &solana_sdk::bpf_loader_upgradeable::id(),
            );
            (
                SwapInstruction::Init(InitPoolArgs {
                    fee_bps: args.num("--fee-bps", 0u16)?,
                    max_price_deviation_bps: args.num("--deviation-bps", 1000u16)?,
                    min_price_update_interval: args.num("--min-price-interval", 3600i64)?,
                    guardian: match args.get("--guardian") {
                        Some(g) => Pubkey::from_str(&g)?,
                        None => Pubkey::default(),
                    },
                    oracle: match args.get("--oracle") {
                        Some(o) => Pubkey::from_str(&o)?,
                        None => Pubkey::default(),
                    },
                })
                .to_bytes(),
                vec![
                    AccountMeta::new(pool_pda, false),
                    AccountMeta::new(payer.pubkey(), true),
                    AccountMeta::new_readonly(hub_mint, false),
                    AccountMeta::new_readonly(hub_vault, false),
                    AccountMeta::new(token_pda(&hub_mint), false),
                    AccountMeta::new_readonly(token_program, false),
                    AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
                    AccountMeta::new_readonly(program_id, false),
                    AccountMeta::new_readonly(program_data, false),
                ],
            )
        }
        "list-token" => {
            let mint = args.key("--mint")?;
            let vault = args.key("--vault")?;
            (
                SwapInstruction::ListToken { price: args.req("--price")?.parse()? }.to_bytes(),
                vec![
                    AccountMeta::new_readonly(pool_pda, false),
                    AccountMeta::new(payer.pubkey(), true),
                    AccountMeta::new_readonly(mint, false),
                    AccountMeta::new_readonly(vault, false),
                    AccountMeta::new(token_pda(&mint), false),
                    AccountMeta::new_readonly(token_program, false),
                    AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
                ],
            )
        }
        "set-price" => {
            let mint = args.key("--mint")?;
            (
                SwapInstruction::SetPrice { price: args.req("--price")?.parse()? }.to_bytes(),
                vec![
                    AccountMeta::new_readonly(pool_pda, false),
                    AccountMeta::new_readonly(payer.pubkey(), true),
                    AccountMeta::new(token_pda(&mint), false),
                ],
            )
        }
        "seed" | "withdraw" => {
            let mint = args.key("--mint")?;
            let amount: u64 = args.req("--amount")?.parse()?;
            let ata = if cmd == "seed" { args.key("--from")? } else { args.key("--to")? };
            let rec = TokenRec::deserialize(&mut &rpc.get_account(&token_pda(&mint))?.data[..])?;
            let ix = if cmd == "seed" {
                SwapInstruction::SeedLiquidity { amount }
            } else {
                SwapInstruction::WithdrawLiquidity { amount }
            };
            (
                ix.to_bytes(),
                vec![
                    AccountMeta::new_readonly(pool_pda, false),
                    AccountMeta::new(payer.pubkey(), true),
                    AccountMeta::new(token_pda(&mint), false),
                    AccountMeta::new(ata, false),
                    AccountMeta::new(pk(&rec.vault), false),
                    AccountMeta::new_readonly(mint, false),
                    AccountMeta::new_readonly(vault_authority, false),
                    AccountMeta::new_readonly(token_program, false),
                ],
            )
        }
        "swap" => {
            let mint_in = args.key("--mint-in")?;
            let mint_out = args.key("--mint-out")?;
            let amount_in: u64 = args.req("--amount")?.parse()?;
            let user_in = args.key("--from")?;
            let user_out = args.key("--to")?;
            let ri = TokenRec::deserialize(&mut &rpc.get_account(&token_pda(&mint_in))?.data[..])?;
            let ro = TokenRec::deserialize(&mut &rpc.get_account(&token_pda(&mint_out))?.data[..])?;
            (
                SwapInstruction::Swap { amount_in, min_amount_out: args.num("--min-out", 0u64)? }
                    .to_bytes(),
                vec![
                    AccountMeta::new_readonly(pool_pda, false),
                    AccountMeta::new_readonly(payer.pubkey(), true),
                    AccountMeta::new(token_pda(&mint_in), false),
                    AccountMeta::new(token_pda(&mint_out), false),
                    AccountMeta::new(user_in, false),
                    AccountMeta::new(user_out, false),
                    AccountMeta::new(pk(&ri.vault), false),
                    AccountMeta::new(pk(&ro.vault), false),
                    AccountMeta::new_readonly(mint_in, false),
                    AccountMeta::new_readonly(mint_out, false),
                    AccountMeta::new_readonly(vault_authority, false),
                    AccountMeta::new_readonly(token_program, false),
                ],
            )
        }
        "pause" | "unpause" => (
            if cmd == "pause" { SwapInstruction::Pause } else { SwapInstruction::Unpause }.to_bytes(),
            vec![
                AccountMeta::new(pool_pda, false),
                AccountMeta::new_readonly(payer.pubkey(), true),
            ],
        ),
        "set-fee" => (
            SwapInstruction::SetFee { fee_bps: args.req("--fee-bps")?.parse()? }.to_bytes(),
            vec![
                AccountMeta::new(pool_pda, false),
                AccountMeta::new_readonly(payer.pubkey(), true),
            ],
        ),
        "set-oracle" => (
            SwapInstruction::SetOracle { oracle: args.key("--oracle")? }.to_bytes(),
            vec![
                AccountMeta::new(pool_pda, false),
                AccountMeta::new_readonly(payer.pubkey(), true),
            ],
        ),
        "set-guardian" => (
            SwapInstruction::SetGuardian { guardian: args.key("--guardian")? }.to_bytes(),
            vec![
                AccountMeta::new(pool_pda, false),
                AccountMeta::new_readonly(payer.pubkey(), true),
            ],
        ),
        other => anyhow::bail!("unknown command {other:?}"),
    };

    let _ = PRICE_ONE; // documented unit; referenced so the import is not stale
    let ix = Instruction { program_id, accounts, data };
    let blockhash = rpc.get_latest_blockhash()?;
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);
    let sig = rpc.send_and_confirm_transaction(&tx)?;
    println!("{cmd} OK — tx {sig}");
    Ok(())
}
