//! Sign and submit a `BridgeOutbound::submit(destination, payload)`
//! extrinsic to a local bridgechain node. Used by the relayer's e2e harness
//! to drive the full Substrate→Ethereum message-passing leg.
//!
//! On success, prints two lines to stdout:
//!   block_hash=0x...
//!   block_number=N
//!
//! These let the test caller pin the exact block whose MMR leaf will carry
//! the message's commitment under `leaf_extra`.

use anyhow::{anyhow, Result};
use clap::Parser;
use subxt::{OnlineClient, PolkadotConfig};
use subxt_signer::sr25519::dev;

#[derive(Parser, Debug)]
#[command(about = "Submit a test message via BridgeOutbound::submit on bridgechain")]
struct Args {
    /// Substrate WebSocket RPC endpoint.
    #[arg(long, default_value = "ws://127.0.0.1:9944")]
    rpc: String,
    /// Destination Ethereum address (0x-prefixed 20 bytes hex).
    #[arg(long, default_value = "0x000000000000000000000000000000000000beef")]
    destination: String,
    /// Payload bytes (0x-prefixed hex).
    #[arg(long, default_value = "0xdeadbeef")]
    payload: String,
}

fn decode_hex(s: &str) -> Result<Vec<u8>> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    hex::decode(s).map_err(|e| anyhow!("decode hex {s}: {e}"))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let dest_vec = decode_hex(&args.destination)?;
    if dest_vec.len() != 20 {
        return Err(anyhow!(
            "destination must be 20 bytes, got {}",
            dest_vec.len()
        ));
    }
    let mut destination = [0u8; 20];
    destination.copy_from_slice(&dest_vec);
    let payload = decode_hex(&args.payload)?;

    let api = OnlineClient::<PolkadotConfig>::from_url(&args.rpc).await?;
    let at = api.at_current_block().await?;

    // BridgeOutbound::submit(destination: H160, payload: BoundedVec<u8, ..>).
    // H160 is `[u8; 20]`, BoundedVec encodes like Vec<u8>; both map cleanly
    // from native Rust types via subxt's EncodeAsType.
    let tx = subxt::dynamic::tx("BridgeOutbound", "submit", (destination, payload));

    let signer = dev::alice();
    let progress = at
        .transactions()
        .sign_and_submit_then_watch_default(&tx, &signer)
        .await?;

    let in_block = progress.wait_for_finalized().await?;
    let block_hash = in_block.block_hash();

    // Surface ExtrinsicFailed events so the harness fails loudly instead of
    // silently relaying a no-op block.
    let events = in_block.wait_for_success().await?;
    drop(events);

    let block_at = in_block.at().await?;
    let block_number = block_at.block_number();

    println!("block_hash=0x{}", hex::encode(block_hash.0));
    println!("block_number={}", block_number);
    Ok(())
}
