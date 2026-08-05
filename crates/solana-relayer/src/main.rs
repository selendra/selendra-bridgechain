//! solana-relayer — the Solana leg's off-chain runner (finding M-3).
//!
//! Runs as its OWN process rather than inside `validator`, and has to: adding
//! `solana-client` to the validator fails to resolve, because Solana 1.18 pins
//! `ed25519-dalek 1.0.1 -> curve25519-dalek 3.2.1 -> zeroize <1.4` while alloy
//! requires `zeroize ^1.5`. The two dependency trees are mutually exclusive.
//! Splitting the process is the correct boundary anyway — it shares no chain
//! client with the EVM side, only the sig-store, which it reaches over HTTP.
//!
//! It performs the validator's job for Solana: scan the gate for `Sent`,
//! independently recompute the submissionId, sign it with the SAME secp256k1 key
//! the EVM validator uses, and store the signature.

mod config;
mod source;
mod state;
mod store;
mod target;

use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "solana_relayer=info".into()),
        )
        .init();

    let path = std::env::args().nth(1).unwrap_or_else(|| "solana-relayer.toml".into());
    let cfg = config::Config::load(&path)?;
    let key = cfg.signer.resolve()?;

    let token = std::env::var(&cfg.store.token_env).ok();
    if token.is_none() {
        tracing::warn!(
            var = %cfg.store.token_env,
            "no sig-store token set — requests will be unauthenticated"
        );
    }
    let store = store::Store::new(&cfg.store.url, token);

    // The claim submitter, when configured. Spawned alongside the scanner and
    // isolated the same way the EVM validator isolates its loops: a dead
    // submitter must never stop this node from signing.
    let submitter = match cfg.target.as_ref() {
        Some(t) => {
            let store = store::Store::new(&cfg.store.url, std::env::var(&cfg.store.token_env).ok());
            Some(target::Submitter::new(&cfg.source, t, store)?)
        }
        None => {
            info!("no [target] block — this relayer signs but never delivers claims");
            None
        }
    };

    let scanner = source::Scanner::new(cfg.source, key, store)?;
    info!(validator = %scanner.signer_address(), "solana-relayer started");

    match submitter {
        Some(s) => {
            let scan = tokio::spawn(scanner.run());
            let submit = tokio::spawn(s.run());
            tokio::select! {
                r = scan => r??,
                r = submit => r??,
            }
            Ok(())
        }
        None => scanner.run().await,
    }
}
