//! Optional on-chain read view over same-chain `SwapPool`s.
//!
//! Given a destination `chainId -> (rpc, pool)` mapping (from `--swap`), this
//! reads a pool's listed tokens, prices, and reserves so the UI can render a
//! swap screen and get a live `quote` without hardcoding pool state. It is
//! strictly read-only: no swaps are ever executed here (that happens from the
//! user's wallet in the frontend). Chains the API wasn't configured with simply
//! report `null`, and any RPC failure degrades to `null` rather than failing the
//! whole GraphQL query — same posture as the executed-gate lookups in `chain.rs`.

use std::collections::BTreeMap;
use std::str::FromStr;

use alloy::primitives::{Address, U256};
use alloy::providers::{DynProvider, Provider};
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent;
use bridge_core::abi::{IERC20Mintable, SwapPool};

use crate::chain::{provider_for, split_spec};

/// One listed token in a pool, flattened for the wire. Numeric fields are
/// decimal strings (uint256) to avoid JSON precision loss, mirroring how
/// `Submission.amount` is carried.
#[derive(Clone, Debug)]
pub struct PoolToken {
    /// `0x`-prefixed token address.
    pub token: String,
    /// ERC-20 symbol (best-effort; empty if the token doesn't expose one).
    pub symbol: String,
    pub decimals: u8,
    /// USD price, PRICE_ONE(1e18)-scaled, as a decimal string.
    pub price: String,
    /// Current reserve (the swap lock), in token base units, decimal string.
    pub reserve: String,
    /// `reserve * price / 10^decimals` — the max swap OUT value in
    /// PRICE_ONE-scaled USD, decimal string. This is the "max swap up to lock".
    pub max_swap_usd: String,
    /// True for the pool's core-price stablecoin (price pinned to 1.0).
    pub is_stable: bool,
}

/// A configured pool's address + core stablecoin + its listed tokens. The
/// `address` is what the UI sends `approve`/`swap` transactions to (the `pools`
/// token list alone can't be swapped against without it).
#[derive(Clone, Debug)]
pub struct PoolInfo {
    /// `0x`-prefixed SwapPool contract address.
    pub address: String,
    /// `0x`-prefixed core stablecoin (the unit of account).
    pub stable: String,
    pub tokens: Vec<PoolToken>,
}

/// Same-chain swap pools the API can read from. Cheap to clone (each
/// `DynProvider` is an `Arc` internally); share freely across resolvers.
#[derive(Clone, Default)]
pub struct Swaps {
    pools: BTreeMap<u64, (DynProvider, Address)>,
}

impl Swaps {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a pool from a `CHAINID=RPC,POOL` spec, e.g.
    /// `1337=http://127.0.0.1:8545,0xPool...`. The HTTP provider is built eagerly
    /// (no network I/O yet); the first query is what hits the chain.
    pub fn add_spec(&mut self, spec: &str) -> anyhow::Result<u64> {
        let (chain_id, rpc, pool_s) = split_spec(spec, "--swap")?;
        let (provider, pool) = provider_for(pool_s, rpc, &format!("--swap {spec:?}"))?;
        self.pools.insert(chain_id, (provider, pool));
        Ok(chain_id)
    }

    /// The chainIds this API can report pool state for.
    pub fn configured(&self) -> Vec<u64> {
        self.pools.keys().copied().collect()
    }

    /// Discover the currently-listed token set for a pool by replaying its
    /// `TokenListed` / `TokenDelisted` logs in chain order. Returns addresses in
    /// discovery order (stable first, since it's listed in the constructor).
    async fn listed_tokens(
        provider: &DynProvider,
        pool: Address,
    ) -> anyhow::Result<Vec<Address>> {
        // Two topic0 filters folded into one ordered replay.
        let listed_f = Filter::new()
            .address(pool)
            .event_signature(SwapPool::TokenListed::SIGNATURE_HASH)
            .from_block(0);
        let delisted_f = Filter::new()
            .address(pool)
            .event_signature(SwapPool::TokenDelisted::SIGNATURE_HASH)
            .from_block(0);

        let mut events: Vec<(u64, u64, bool, Address)> = Vec::new(); // (block, idx, listed?, token)
        for log in provider.get_logs(&listed_f).await? {
            if let Ok(d) = SwapPool::TokenListed::decode_log(&log.inner) {
                events.push((
                    log.block_number.unwrap_or(0),
                    log.log_index.unwrap_or(0),
                    true,
                    d.data.token,
                ));
            }
        }
        for log in provider.get_logs(&delisted_f).await? {
            if let Ok(d) = SwapPool::TokenDelisted::decode_log(&log.inner) {
                events.push((
                    log.block_number.unwrap_or(0),
                    log.log_index.unwrap_or(0),
                    false,
                    d.data.token,
                ));
            }
        }
        events.sort_by_key(|(b, i, _, _)| (*b, *i));

        // Replay in order, preserving first-seen order for stable UI output.
        let mut order: Vec<Address> = Vec::new();
        let mut live: BTreeMap<Address, bool> = BTreeMap::new();
        for (_, _, is_listed, token) in events {
            if is_listed {
                if !live.get(&token).copied().unwrap_or(false) && !order.contains(&token) {
                    order.push(token);
                }
                live.insert(token, true);
            } else {
                live.insert(token, false);
            }
        }
        Ok(order.into_iter().filter(|t| live.get(t).copied().unwrap_or(false)).collect())
    }

    /// Full pool snapshot: every listed token with its price, reserve, and
    /// max-swap USD value. `None` when the chain isn't configured or the RPC
    /// read fails (never propagates an error into the GraphQL response).
    pub async fn pools(&self, chain_id: u64) -> Option<Vec<PoolToken>> {
        let (provider, pool_addr) = self.pools.get(&chain_id)?;
        let pool = SwapPool::new(*pool_addr, provider);

        let stable = pool.stable().call().await.ok()?;
        let tokens = Self::listed_tokens(provider, *pool_addr).await.ok()?;

        let mut out = Vec::with_capacity(tokens.len());
        for token in tokens {
            let info = pool.tokens(token).call().await.ok()?;
            let decimals = info.decimals;
            let price = info.price;
            let reserve = info.reserve;
            // reserve * price / 10^decimals (PRICE_ONE-scaled USD). Fits in U256
            // for any realistic reserve/price; saturating guards the pathological.
            let scale = U256::from(10u64).pow(U256::from(decimals));
            let max_swap_usd = reserve
                .checked_mul(price)
                .map(|v| v / scale)
                .unwrap_or(U256::ZERO);

            let symbol = IERC20Mintable::new(token, provider)
                .symbol()
                .call()
                .await
                .unwrap_or_default();

            out.push(PoolToken {
                token: format!("{token:#x}"),
                symbol,
                decimals,
                price: price.to_string(),
                reserve: reserve.to_string(),
                max_swap_usd: max_swap_usd.to_string(),
                is_stable: token == stable,
            });
        }
        Some(out)
    }

    /// Pool metadata (address + stable) alongside the full token snapshot, so a
    /// UI can build swap/approve transactions against a discovered pool. `None`
    /// on the same conditions as [`pools`](Self::pools).
    pub async fn pool_info(&self, chain_id: u64) -> Option<PoolInfo> {
        let (_, pool_addr) = self.pools.get(&chain_id)?;
        let tokens = self.pools(chain_id).await?;
        // The stable is the (only) token flagged is_stable; derive it from the
        // snapshot rather than re-reading `stable()` off-chain.
        let stable = tokens
            .iter()
            .find(|t| t.is_stable)
            .map(|t| t.token.clone())
            .unwrap_or_default();
        Some(PoolInfo { address: format!("{pool_addr:#x}"), stable, tokens })
    }

    /// On-chain `quote(tokenIn, tokenOut, amountIn)` — the pegged output for a
    /// swap, net of fee, WITHOUT the reserve-cap check (matches the contract's
    /// `quote`). Returns the amount as a decimal string, or `None` if the chain
    /// isn't configured, an address/amount is malformed, or the call reverts
    /// (e.g. a token isn't listed).
    pub async fn quote(
        &self,
        chain_id: u64,
        token_in: &str,
        token_out: &str,
        amount_in: &str,
    ) -> Option<String> {
        let (provider, pool_addr) = self.pools.get(&chain_id)?;
        let ti = Address::from_str(token_in.trim()).ok()?;
        let to = Address::from_str(token_out.trim()).ok()?;
        let amt = U256::from_str(amount_in.trim()).ok()?;
        let pool = SwapPool::new(*pool_addr, provider);
        let out = pool.quote(ti, to, amt).call().await.ok()?;
        Some(out.to_string())
    }
}
