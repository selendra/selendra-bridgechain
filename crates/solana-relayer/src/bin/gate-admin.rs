//! `gate-admin` — the on-chain client for the Solana gate's governance
//! instructions.
//!
//! `scripts/testing/solana-onchain.sh` states the gap this fills: "driving
//! init/send/claim on-chain needs a client that…" — there wasn't one. The gate
//! could be built and deployed but never *configured*, so nothing downstream
//! (corridors, assets, the relayer) could be exercised against a real cluster.
//!
//! It lives in `solana-relayer` because that crate already carries the only
//! dependency set that can talk to Solana: `solana-client` pins `zeroize <1.4`,
//! which cannot coexist with alloy's `^1.5`, so no EVM-side crate can host it.
//!
//! Every subcommand is owner- or upgrade-authority-gated ON-CHAIN. This tool
//! only builds and signs transactions; it grants no authority of its own.
//!
//!   gate-admin --rpc <url> --keypair <path> --program <pubkey> <command>
//!
//!     init --chain-id N --threshold N --validator 0x.. [--validator 0x..]
//!          --bridge-domain <0x…32 bytes>
//!          [--max-validators N] [--max-corridors N] [--guardian <pubkey>]
//!     register-corridor --chain-id-to N
//!     register-asset --debridge-id 0x.. --mint <pubkey> --vault <pubkey>
//!     set-threshold --threshold N
//!     set-validator --validator 0x.. --active <bool>
//!     send --debridge-id 0x.. --amount N --chain-id-to N --receiver 0x..
//!          --from-token-account <pubkey>
//!     cancel --debridge-id 0x.. --amount N --chain-id-from N --nonce N
//!            --receiver 0x.. --native-sender 0x.. --signature 0x.. [--signature 0x..]
//!     refund --debridge-id 0x.. --amount N --chain-id-to N --nonce N
//!            --receiver 0x.. --native-sender 0x.. --to-token-account <pubkey>
//!            --signature 0x.. [--signature 0x..]
//!     digest --submission-id 0x.. — print the cancel/refund digests to sign
//!     show
//!
//! `cancel`/`refund` take signatures as INPUT rather than signing themselves:
//! they are validator attestations over domain-separated digests, and a tool that
//! could mint them would be a tool that could burn or claw back any transfer.
//! Use `digest` to get the bytes, sign them with the validator keys wherever
//! those live, and pass the results back.

use std::str::FromStr;

use bridge_solana::instruction::{GateInstruction, InitArgs};
use solana_client::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{read_keypair_file, Signer};
use solana_sdk::transaction::Transaction;

/// The BPF upgradeable loader — `init` proves the caller is the program's
/// upgrade authority, which means reading the loader's ProgramData account.
const BPF_LOADER_UPGRADEABLE: &str = "BPFLoaderUpgradeab1e11111111111111111111111";
const SPL_TOKEN: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

/// `BridgeHash` domain prefixes. A transfer signature must never authorise a
/// burn, and a cancel must never authorise a payout, so each lives in its own
/// keccak domain — mirrored byte-for-byte from `solana-gate` and `BridgeHash.sol`.
const CANCEL_PREFIX: u64 = 2;
const REFUND_PREFIX: u64 = 3;

fn be32(v: u64) -> [u8; 32] {
    let mut o = [0u8; 32];
    o[24..].copy_from_slice(&v.to_be_bytes());
    o
}

/// keccak(prefix || submissionId) — the digest validators sign for cancel/refund.
fn domain_id(prefix: u64, submission_id: &[u8; 32]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(&be32(prefix));
    buf.extend_from_slice(submission_id);
    bridge_solana::hash::keccak(&buf)
}

/// Parse a repeated `--signature 0x..` into 65-byte r||s||v arrays.
fn parse_sigs(args: &Args) -> anyhow::Result<Vec<Vec<u8>>> {
    let out: Vec<Vec<u8>> = args
        .all("--signature")
        .iter()
        .map(|s| {
            let h = s.strip_prefix("0x").unwrap_or(s);
            hex::decode(h).map_err(|_| anyhow::anyhow!("signature {s:?} is not hex"))
        })
        .collect::<Result<_, _>>()?;
    anyhow::ensure!(!out.is_empty(), "at least one --signature is required");
    for s in &out {
        anyhow::ensure!(s.len() == 65, "each signature must be 65 bytes, got {}", s.len());
    }
    Ok(out)
}

fn parse_evm(s: &str) -> anyhow::Result<[u8; 20]> {
    let h = s.strip_prefix("0x").unwrap_or(s);
    let b = hex::decode(h).map_err(|_| anyhow::anyhow!("validator {s:?} is not hex"))?;
    b.try_into().map_err(|_| anyhow::anyhow!("validator {s:?} must be 20 bytes"))
}

fn parse_b32(s: &str) -> anyhow::Result<[u8; 32]> {
    let h = s.strip_prefix("0x").unwrap_or(s);
    let b = hex::decode(h).map_err(|_| anyhow::anyhow!("{s:?} is not hex"))?;
    b.try_into().map_err(|_| anyhow::anyhow!("{s:?} must be 32 bytes"))
}

/// Minimal flag reader: `--name value`. Repeated flags collect.
struct Args(Vec<String>);
impl Args {
    fn get(&self, name: &str) -> Option<String> {
        self.0.iter().position(|a| a == name).and_then(|i| self.0.get(i + 1)).cloned()
    }
    fn all(&self, name: &str) -> Vec<String> {
        self.0
            .iter()
            .enumerate()
            .filter(|(_, a)| a.as_str() == name)
            .filter_map(|(i, _)| self.0.get(i + 1).cloned())
            .collect()
    }
    fn req(&self, name: &str) -> anyhow::Result<String> {
        self.get(name).ok_or_else(|| anyhow::anyhow!("missing required flag {name}"))
    }
}

fn main() -> anyhow::Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // The command is the first bare token that is NOT a flag's value. Skipping
    // only `--`-prefixed tokens is not enough: `--rpc https://…` would make the
    // URL look like the command.
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
    let program_id = Pubkey::from_str(&args.req("--program")?)?;
    let payer = read_keypair_file(args.req("--keypair")?)
        .map_err(|e| anyhow::anyhow!("reading keypair: {e}"))?;
    let rpc = RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed());

    let (config_pda, _) = Pubkey::find_program_address(&[b"config"], &program_id);
    let (vault_authority, _) = Pubkey::find_program_address(&[b"vault_authority"], &program_id);

    if cmd == "digest" {
        let id = parse_b32(&args.req("--submission-id")?)?;
        println!("submissionId : 0x{}", hex::encode(id));
        println!("cancelId     : 0x{}", hex::encode(domain_id(CANCEL_PREFIX, &id)));
        println!("refundId     : 0x{}", hex::encode(domain_id(REFUND_PREFIX, &id)));
        println!();
        println!("Validators sign the EIP-191 digest of the id above — the same");
        println!("`personal_sign` shape as the EVM side, so `cast wallet sign` works:");
        println!("  cast wallet sign --private-key <key> <cancelId|refundId>");
        return Ok(());
    }

    if cmd == "show" {
        println!("program        : {program_id}");
        println!("config PDA     : {config_pda}");
        println!("vault authority: {vault_authority}");
        match rpc.get_account(&config_pda) {
            Ok(acct) => {
                println!("config account : {} bytes, owner {}", acct.data.len(), acct.owner);
                // owner(32) guardian(32) len(4) validators(20n) threshold(4) chain_id(8) paused(1)
                let d = &acct.data;
                if d.len() >= 68 {
                    let n = u32::from_le_bytes(d[64..68].try_into()?) as usize;
                    let end = 68 + n * 20;
                    println!("  owner        : {}", Pubkey::new_from_array(d[0..32].try_into()?));
                    println!("  validators   : {n}");
                    for i in 0..n {
                        println!("    0x{}", hex::encode(&d[68 + i * 20..88 + i * 20]));
                    }
                    if d.len() >= end + 13 {
                        println!("  threshold    : {}", u32::from_le_bytes(d[end..end + 4].try_into()?));
                        println!("  chain_id     : {}", u64::from_le_bytes(d[end + 4..end + 12].try_into()?));
                        println!("  paused       : {}", d[end + 12] != 0);
                    }
                }
            }
            Err(_) => println!("config account : NOT INITIALIZED (run `init`)"),
        }
        return Ok(());
    }

    let (ix_data, accounts) = match cmd.as_str() {
        "init" => {
            let validators: Vec<[u8; 20]> =
                args.all("--validator").iter().map(|v| parse_evm(v)).collect::<Result<_, _>>()?;
            anyhow::ensure!(!validators.is_empty(), "init needs at least one --validator");
            let threshold: u32 = args.req("--threshold")?.parse()?;
            let chain_id: u64 = args.req("--chain-id")?.parse()?;
            let max_validators: u32 =
                args.get("--max-validators").unwrap_or_else(|| "8".into()).parse()?;
            let max_corridors: u32 =
                args.get("--max-corridors").unwrap_or_else(|| "8".into()).parse()?;
            // Required, with no default: a defaulted domain shared by every
            // deployment would be the same as having none.
            let bridge_domain = parse_b32(&args.req("--bridge-domain")?)?;
            let guardian = match args.get("--guardian") {
                Some(g) => Pubkey::from_str(&g)?.to_bytes(),
                None => [0u8; 32],
            };

            let loader = Pubkey::from_str(BPF_LOADER_UPGRADEABLE)?;
            let (program_data, _) =
                Pubkey::find_program_address(&[program_id.as_ref()], &loader);

            (
                GateInstruction::Init(InitArgs {
                    bridge_domain,
                    validators,
                    threshold,
                    chain_id,
                    max_validators,
                    max_corridors,
                    guardian,
                })
                .to_bytes(),
                vec![
                    AccountMeta::new(config_pda, false),
                    AccountMeta::new(payer.pubkey(), true),
                    AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
                    AccountMeta::new_readonly(program_id, false),
                    AccountMeta::new_readonly(program_data, false),
                ],
            )
        }
        "register-corridor" => (
            GateInstruction::RegisterCorridor { chain_id_to: args.req("--chain-id-to")?.parse()? }
                .to_bytes(),
            vec![
                AccountMeta::new(config_pda, false),
                AccountMeta::new_readonly(payer.pubkey(), true),
            ],
        ),
        "register-asset" => {
            let debridge_id = parse_b32(&args.req("--debridge-id")?)?;
            let mint = Pubkey::from_str(&args.req("--mint")?)?;
            let vault = Pubkey::from_str(&args.req("--vault")?)?;
            let (asset_pda, _) =
                Pubkey::find_program_address(&[b"asset", &debridge_id], &program_id);
            (
                GateInstruction::RegisterAsset { debridge_id }.to_bytes(),
                vec![
                    AccountMeta::new_readonly(config_pda, false),
                    AccountMeta::new(payer.pubkey(), true),
                    AccountMeta::new(asset_pda, false),
                    AccountMeta::new_readonly(mint, false),
                    AccountMeta::new_readonly(vault, false),
                    AccountMeta::new_readonly(Pubkey::from_str(SPL_TOKEN)?, false),
                    AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
                ],
            )
        }
        "set-threshold" => (
            GateInstruction::SetThreshold { threshold: args.req("--threshold")?.parse()? }
                .to_bytes(),
            vec![
                AccountMeta::new(config_pda, false),
                AccountMeta::new_readonly(payer.pubkey(), true),
            ],
        ),
        // Solana -> EVM. Locks the caller's SPL tokens into the registered vault
        // and emits the `Sent` event the relayer signs.
        //
        // The `["sent", submissionId]` record PDA has to be derived client-side,
        // which means recomputing the id exactly as the program does — same
        // fields, same order. `bridge_solana::hash` is the shared implementation
        // that Phase 3 locks against the Solidity fixtures, so this cannot drift
        // from either VM.
        "send" => {
            let debridge_id = parse_b32(&args.req("--debridge-id")?)?;
            let amount: u64 = args.req("--amount")?.parse()?;
            let chain_id_to: u64 = args.req("--chain-id-to")?.parse()?;
            let receiver = {
                let h = args.req("--receiver")?;
                let h = h.strip_prefix("0x").unwrap_or(&h).to_string();
                hex::decode(&h).map_err(|_| anyhow::anyhow!("--receiver is not hex"))?
            };
            anyhow::ensure!(
                receiver.len() == 20 || receiver.len() == 32,
                "receiver must be 20 bytes (EVM) or 32 (Solana), got {}",
                receiver.len()
            );
            let user_token = Pubkey::from_str(&args.req("--from-token-account")?)?;

            // chain_id and the per-corridor nonce come from the config; the
            // program uses exactly these to build the id.
            let cfg_acct = rpc.get_account(&config_pda)?;
            let d = &cfg_acct.data;
            // Borsh Config layout, in declaration order and with no header:
            //   owner(32) | bridge_domain(32) | guardian(32) | validators(4+20n) | …
            // Adding a field ahead of these shifts every offset below, which is
            // why the domain read and the length read are derived from the same
            // running total rather than two independent magic numbers.
            let bridge_domain: [u8; 32] = d[32..64].try_into()?;
            let validators_off = 32 + 32 + 32;
            let n = u32::from_le_bytes(
                d[validators_off..validators_off + 4].try_into()?,
            ) as usize;
            let after_validators = validators_off + 4 + n * 20;
            let chain_id = u64::from_le_bytes(
                d[after_validators + 4..after_validators + 12].try_into()?,
            );
            // …then paused(1) max_validators(4) max_corridors(4), then nonce_to.
            let nonce_off = after_validators + 12 + 1 + 4 + 4;
            let entries = u32::from_le_bytes(d[nonce_off..nonce_off + 4].try_into()?) as usize;
            let mut nonce = None;
            for i in 0..entries {
                let o = nonce_off + 4 + i * 16;
                if u64::from_le_bytes(d[o..o + 8].try_into()?) == chain_id_to {
                    nonce = Some(u64::from_le_bytes(d[o + 8..o + 16].try_into()?));
                    break;
                }
            }
            let nonce = nonce.ok_or_else(|| {
                anyhow::anyhow!("corridor {chain_id_to} is not registered — run register-corridor")
            })?;

            // No auto-params here, so `native_sender` is NOT part of the hash —
            // it only enters via `keccak(nativeSender)` in the auto tail, exactly
            // as `BridgeHash.sol` defines it. Using the with-auto form here would
            // produce an id the gate never derives.
            let id = bridge_solana::hash::submission_id(
                &bridge_domain,
                &debridge_id,
                &bridge_solana::hash::amount_word(amount as u128),
                chain_id,
                chain_id_to,
                nonce,
                &receiver,
            );

            let (asset_pda, _) =
                Pubkey::find_program_address(&[b"asset", &debridge_id], &program_id);
            let (sent_pda, _) = Pubkey::find_program_address(&[b"sent", &id], &program_id);
            let asset_acct = rpc.get_account(&asset_pda)?;
            anyhow::ensure!(asset_acct.data.len() >= 96, "asset account is malformed");
            let vault = Pubkey::new_from_array(asset_acct.data[64..96].try_into()?);

            println!("submissionId : 0x{}", hex::encode(id));
            println!("nonce        : {nonce}  corridor {chain_id} -> {chain_id_to}");
            println!("vault        : {vault}");

            (
                GateInstruction::Send(bridge_solana::instruction::SendArgs {
                    debridge_id,
                    amount,
                    chain_id_to,
                    receiver,
                    auto: None,
                })
                .to_bytes(),
                vec![
                    AccountMeta::new(config_pda, false),
                    AccountMeta::new_readonly(asset_pda, false),
                    AccountMeta::new(payer.pubkey(), true),
                    AccountMeta::new(user_token, false),
                    AccountMeta::new(vault, false),
                    AccountMeta::new_readonly(Pubkey::from_str(SPL_TOKEN)?, false),
                    AccountMeta::new(sent_pda, false),
                    AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
                ],
            )
        }
        // M-2, DESTINATION side: burn the transfer so it can never be claimed.
        // Moves no funds; it only unlocks the source-side refund.
        "cancel" => {
            let a = bridge_solana::instruction::CancelArgs {
                debridge_id: parse_b32(&args.req("--debridge-id")?)?,
                amount: args.req("--amount")?.parse()?,
                chain_id_from: args.req("--chain-id-from")?.parse()?,
                nonce: args.req("--nonce")?.parse()?,
                receiver: hex::decode(
                    args.req("--receiver")?.strip_prefix("0x").unwrap_or(&args.req("--receiver")?),
                )?,
                auto: None,
                native_sender: hex::decode(
                    args.req("--native-sender")?
                        .strip_prefix("0x")
                        .unwrap_or(&args.req("--native-sender")?),
                )?,
                signatures: parse_sigs(&args)?,
            };
            let id = parse_b32(&args.req("--submission-id")?)?;
            let (executed, _) = Pubkey::find_program_address(&[b"executed", &id], &program_id);
            println!("burning {} (executed PDA {})", hex::encode(id), executed);
            (
                GateInstruction::Cancel(a).to_bytes(),
                vec![
                    AccountMeta::new_readonly(config_pda, false),
                    AccountMeta::new(executed, false),
                    AccountMeta::new(payer.pubkey(), true),
                    AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
                ],
            )
        }
        // M-2, SOURCE side: return the locked funds, but ONLY once the
        // destination burn is on-chain. The gate checks that itself.
        "refund" => {
            let debridge_id = parse_b32(&args.req("--debridge-id")?)?;
            let a = bridge_solana::instruction::RefundArgs {
                debridge_id,
                amount: args.req("--amount")?.parse()?,
                chain_id_to: args.req("--chain-id-to")?.parse()?,
                nonce: args.req("--nonce")?.parse()?,
                receiver: hex::decode(
                    args.req("--receiver")?.strip_prefix("0x").unwrap_or(&args.req("--receiver")?),
                )?,
                auto: None,
                native_sender: hex::decode(
                    args.req("--native-sender")?
                        .strip_prefix("0x")
                        .unwrap_or(&args.req("--native-sender")?),
                )?,
                signatures: parse_sigs(&args)?,
            };
            let id = parse_b32(&args.req("--submission-id")?)?;
            let to_token = Pubkey::from_str(&args.req("--to-token-account")?)?;
            let (asset_pda, _) =
                Pubkey::find_program_address(&[b"asset", &debridge_id], &program_id);
            let (sent_pda, _) = Pubkey::find_program_address(&[b"sent", &id], &program_id);
            let (refunded_pda, _) =
                Pubkey::find_program_address(&[b"refunded", &id], &program_id);
            let asset_acct = rpc.get_account(&asset_pda)?;
            anyhow::ensure!(asset_acct.data.len() >= 96, "asset account is malformed");
            let vault = Pubkey::new_from_array(asset_acct.data[64..96].try_into()?);
            println!("refunding {} from vault {vault}", hex::encode(id));
            (
                GateInstruction::Refund(a).to_bytes(),
                vec![
                    AccountMeta::new_readonly(config_pda, false),
                    AccountMeta::new_readonly(asset_pda, false),
                    AccountMeta::new(sent_pda, false),
                    AccountMeta::new(refunded_pda, false),
                    AccountMeta::new(payer.pubkey(), true),
                    AccountMeta::new(vault, false),
                    AccountMeta::new(to_token, false),
                    AccountMeta::new_readonly(vault_authority, false),
                    AccountMeta::new_readonly(Pubkey::from_str(SPL_TOKEN)?, false),
                    AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
                ],
            )
        }
        "set-validator" => (
            GateInstruction::SetValidator {
                validator: parse_evm(&args.req("--validator")?)?,
                active: args.get("--active").unwrap_or_else(|| "true".into()) == "true",
            }
            .to_bytes(),
            vec![
                AccountMeta::new(config_pda, false),
                AccountMeta::new_readonly(payer.pubkey(), true),
            ],
        ),
        other => anyhow::bail!("unknown command {other:?}"),
    };

    let ix = Instruction { program_id, accounts, data: ix_data };
    let blockhash = rpc.get_latest_blockhash()?;
    let tx =
        Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);
    let sig = rpc.send_and_confirm_transaction(&tx)?;
    println!("{cmd} OK — tx {sig}");
    Ok(())
}
