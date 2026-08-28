//! Reading a Solana swap pool, without a Solana client.
//!
//! `solana-client` pins `zeroize <1.4` and alloy needs `^1.5`, so this binary
//! cannot link one. It does not need to: the pool's whole state is a handful of
//! program-owned accounts, and `getProgramAccounts` returns them over plain
//! JSON-RPC. They are decoded with `swap-math`, the same crate the on-chain
//! program links — so the layouts and the pricing here are not a copy of the
//! program's, they are the program's.
//!
//! Accounts are addressed by DERIVING their PDAs (`swap-math`'s `pda` feature)
//! and fetching them with `getMultipleAccounts` — one call, and one that works
//! on any endpoint. The obvious alternative, `getProgramAccounts`, is blocked on
//! most hosted free tiers ("not available on the Free tier" on Alchemy), so it
//! is kept only as the fallback for a self-hosted node when no mints are
//! configured to derive from.

use std::collections::BTreeMap;

use base64::Engine;
use swap_math::{PoolState, TokenState, POOL_SPACE, TOKEN_SPACE};

/// A configured Solana pool: an RPC endpoint and the program that owns it.
#[derive(Clone, Debug)]
pub struct SolanaPool {
    rpc: String,
    /// Base58 program id — also what the UI sends its swap instruction to.
    pub program: String,
    /// mint (base58) -> symbol, from the chain registry. SPL mints carry no
    /// on-chain symbol (that lives in Metaplex metadata), so the registry is
    /// the only honest source; an unlisted mint shows as a truncated address
    /// rather than an invented ticker.
    pub symbols: BTreeMap<String, String>,
}

/// One decoded pool snapshot.
pub struct Snapshot {
    pub pool: PoolState,
    pub tokens: Vec<TokenState>,
}

impl SolanaPool {
    pub fn new(rpc: impl Into<String>, program: impl Into<String>) -> Self {
        Self { rpc: rpc.into(), program: program.into(), symbols: BTreeMap::new() }
    }

    /// The pool and its listed tokens.
    ///
    /// Prefers derived addresses + `getMultipleAccounts`; falls back to
    /// `getProgramAccounts` when there are no configured mints to derive from.
    /// Errors are returned rather than swallowed so the caller can log them; a
    /// GraphQL resolver still turns them into `null` rather than a 500.
    pub async fn snapshot(&self) -> anyhow::Result<Snapshot> {
        if !self.symbols.is_empty() {
            return self.snapshot_by_pda().await;
        }
        self.snapshot_by_scan().await
    }

    /// One `getMultipleAccounts` over the pool PDA and one record per known
    /// mint. Unknown mints are simply not in the registry — a pool token nobody
    /// named is one the UI could not label anyway.
    async fn snapshot_by_pda(&self) -> anyhow::Result<Snapshot> {
        let program = from_b58(&self.program)
            .ok_or_else(|| anyhow::anyhow!("program id {} is not base58", self.program))?;
        let pool_pda = swap_math::pda::pool_address(&program)
            .ok_or_else(|| anyhow::anyhow!("no pool PDA for {}", self.program))?;

        let mints: Vec<[u8; 32]> =
            self.symbols.keys().filter_map(|m| from_b58(m)).collect();
        let mut addrs = vec![b58(&pool_pda)];
        for m in &mints {
            let rec = swap_math::pda::token_address(&program, m)
                .ok_or_else(|| anyhow::anyhow!("no token PDA"))?;
            addrs.push(b58(&rec));
        }

        let values = self.get_multiple_accounts(&addrs).await?;
        let pool_raw = values
            .first()
            .and_then(|v| v.clone())
            .ok_or_else(|| anyhow::anyhow!("program {} has no pool account — is it initialized?", self.program))?;
        let pool = swap_math::decode::<PoolState>(&pool_raw)
            .ok_or_else(|| anyhow::anyhow!("pool account does not decode"))?;

        let mut tokens = Vec::new();
        for raw in values.into_iter().skip(1).flatten() {
            if let Some(t) = swap_math::decode::<TokenState>(&raw) {
                if t.listed {
                    tokens.push(t);
                }
            }
        }
        tokens.sort_by_key(|t| (t.mint != pool.hub_mint, t.mint));
        Ok(Snapshot { pool, tokens })
    }

    /// `getMultipleAccounts`, returning each account's raw data (or `None` when
    /// the account does not exist).
    pub(crate) async fn get_multiple_accounts(&self, addrs: &[String]) -> anyhow::Result<Vec<Option<Vec<u8>>>> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getMultipleAccounts",
            "params": [addrs, {"encoding": "base64", "commitment": "confirmed"}],
        });
        let resp: serde_json::Value =
            reqwest::Client::new().post(&self.rpc).json(&body).send().await?.json().await?;
        if let Some(err) = resp.get("error") {
            anyhow::bail!("getMultipleAccounts failed: {err}");
        }
        let arr = resp["result"]["value"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("getMultipleAccounts returned no value array"))?;
        let mut out = Vec::with_capacity(arr.len());
        for a in arr {
            if a.is_null() {
                out.push(None);
                continue;
            }
            let b64 = a["data"][0].as_str().ok_or_else(|| anyhow::anyhow!("account data is not base64"))?;
            out.push(Some(base64::engine::general_purpose::STANDARD.decode(b64)?));
        }
        Ok(out)
    }

    async fn snapshot_by_scan(&self) -> anyhow::Result<Snapshot> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getProgramAccounts",
            "params": [self.program, {"encoding": "base64", "commitment": "confirmed"}],
        });
        let resp: serde_json::Value = reqwest::Client::new()
            .post(&self.rpc)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        if let Some(err) = resp.get("error") {
            anyhow::bail!("getProgramAccounts failed: {err}");
        }
        let accounts = resp["result"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("getProgramAccounts returned no result array"))?;

        let mut pool = None;
        let mut tokens = Vec::new();
        for a in accounts {
            let b64 = a["account"]["data"][0]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("account data is not base64"))?;
            let raw = base64::engine::general_purpose::STANDARD.decode(b64)?;
            match raw.len() {
                n if n == POOL_SPACE => pool = swap_math::decode::<PoolState>(&raw),
                n if n == TOKEN_SPACE => {
                    if let Some(t) = swap_math::decode::<TokenState>(&raw) {
                        // A record whose `listed` flag is false is not a pool
                        // member; skipping it here keeps the flag meaningful
                        // rather than decorative.
                        if t.listed {
                            tokens.push(t);
                        }
                    }
                }
                _ => {}
            }
        }
        let pool = pool.ok_or_else(|| {
            anyhow::anyhow!("program {} owns no pool account — is it initialized?", self.program)
        })?;
        // Stable order for a stable UI: the hub first, then by mint.
        tokens.sort_by_key(|t| (t.mint != pool.hub_mint, t.mint));
        Ok(Snapshot { pool, tokens })
    }

    /// An SPL mint's decimals — needed to show a sane amount for an asset the
    /// registry does not describe.
    pub async fn mint_decimals(&self, mint: &str) -> anyhow::Result<u8> {
        let accounts = self.get_multiple_accounts(&[mint.to_string()]).await?;
        let raw = accounts
            .into_iter()
            .next()
            .flatten()
            .ok_or_else(|| anyhow::anyhow!("mint {mint} not found"))?;
        // SPL Mint layout: supply(8) at 36..44, decimals at byte 44.
        raw.get(44).copied().ok_or_else(|| anyhow::anyhow!("mint account is too short"))
    }

    /// A recent blockhash, for a transaction the BROWSER builds and a wallet
    /// signs.
    ///
    /// The UI cannot fetch this itself: the configured endpoint is a credential
    /// (a hosted RPC URL carries its API key), and hardcoding a public cluster
    /// URL into the app is a different deployment's problem. So the API — which
    /// already holds the credential — passes through this one opaque value.
    /// Everything else in the transaction is derived in the browser, because a
    /// destination account taken on trust from a server is a destination that
    /// can be swapped for someone else's.
    pub async fn latest_blockhash(&self) -> anyhow::Result<String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "getLatestBlockhash",
            "params": [{"commitment": "confirmed"}],
        });
        let resp: serde_json::Value =
            reqwest::Client::new().post(&self.rpc).json(&body).send().await?.json().await?;
        if let Some(err) = resp.get("error") {
            anyhow::bail!("getLatestBlockhash failed: {err}");
        }
        resp["result"]["value"]["blockhash"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("no blockhash in response"))
    }

    /// Confirmation state of a signature: `confirmed`, `finalized`, `failed`,
    /// or `pending` while the cluster has not seen it yet.
    pub async fn signature_status(&self, signature: &str) -> anyhow::Result<String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "getSignatureStatuses",
            "params": [[signature], {"searchTransactionHistory": true}],
        });
        let resp: serde_json::Value =
            reqwest::Client::new().post(&self.rpc).json(&body).send().await?.json().await?;
        if let Some(err) = resp.get("error") {
            anyhow::bail!("getSignatureStatuses failed: {err}");
        }
        let v = &resp["result"]["value"][0];
        if v.is_null() {
            return Ok("pending".into());
        }
        if !v["err"].is_null() {
            return Ok("failed".into());
        }
        Ok(v["confirmationStatus"].as_str().unwrap_or("processed").to_string())
    }

    /// The SPL balance of a token account, as a decimal string.
    ///
    /// The UI derives the account address itself and passes it in; this only
    /// reads it, so a wrong answer here can mislead a balance display but can
    /// never redirect funds.
    pub async fn token_balance(&self, account: &str) -> anyhow::Result<String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "getTokenAccountBalance",
            "params": [account, {"commitment": "confirmed"}],
        });
        let resp: serde_json::Value =
            reqwest::Client::new().post(&self.rpc).json(&body).send().await?.json().await?;
        // A missing account is "no balance", not an error: a user who has never
        // held this mint simply has no associated account yet.
        if resp.get("error").is_some() {
            return Ok("0".into());
        }
        Ok(resp["result"]["value"]["amount"].as_str().unwrap_or("0").to_string())
    }

    /// `symbol` for a mint, falling back to a truncated address.
    pub fn symbol_of(&self, mint: &str) -> String {
        self.symbols.get(mint).cloned().unwrap_or_else(|| {
            let short: String = mint.chars().take(4).collect();
            format!("{short}…")
        })
    }
}

/// What a browser needs to build a gate `send` on Solana.
///
/// Every field here decides only whether the transaction SUCCEEDS, never where
/// the funds go: the receiver, amount and destination chain are typed by the
/// user and packed into the instruction by the browser. A wrong nonce or domain
/// yields a submissionId the program does not derive, so the `["sent", id]`
/// account mismatches and the transaction fails.
#[derive(Clone, Debug, serde::Serialize)]
pub struct GateSendContext {
    pub program_id: String,
    pub bridge_domain: String,
    pub chain_id: u64,
    /// Next nonce for the destination corridor.
    pub nonce: u64,
    pub debridge_id: String,
    /// The registered vault for that asset — the program refuses any other.
    pub vault: String,
    pub decimals: u8,
    pub paused: bool,
}

/// A configured Solana bridge gate — the program a `send` locks tokens into.
#[derive(Clone, Debug)]
pub struct SolanaGate {
    rpc: String,
    pub program: String,
}

impl SolanaGate {
    pub fn new(rpc: impl Into<String>, program: impl Into<String>) -> Self {
        Self { rpc: rpc.into(), program: program.into() }
    }

    /// Everything a browser needs to build a `send` for one asset + destination.
    pub async fn send_context(
        &self,
        debridge_id_hex: &str,
        chain_id_to: u64,
    ) -> anyhow::Result<GateSendContext> {
        let program = from_b58(&self.program)
            .ok_or_else(|| anyhow::anyhow!("gate program is not base58"))?;
        let debridge_id = hex_32(debridge_id_hex)
            .ok_or_else(|| anyhow::anyhow!("debridgeId must be 0x + 64 hex"))?;

        let config_pda = swap_math::pda::find_program_address(&[b"config"], &program)
            .ok_or_else(|| anyhow::anyhow!("no config PDA"))?
            .0;
        let asset_pda =
            swap_math::pda::find_program_address(&[b"asset", &debridge_id], &program)
                .ok_or_else(|| anyhow::anyhow!("no asset PDA"))?
                .0;

        let pool = SolanaPool::new(self.rpc.clone(), self.program.clone());
        let accounts = pool.get_multiple_accounts(&[b58(&config_pda), b58(&asset_pda)]).await?;
        let cfg: bridge_solana::account::ConfigAccount = accounts
            .first()
            .and_then(|a| a.as_ref())
            .and_then(|raw| bridge_solana::account::decode(raw))
            .ok_or_else(|| anyhow::anyhow!("gate {} is not initialized", self.program))?;
        let asset: bridge_solana::account::AssetAccount = accounts
            .get(1)
            .and_then(|a| a.as_ref())
            .and_then(|raw| bridge_solana::account::decode(raw))
            .ok_or_else(|| {
                anyhow::anyhow!("no asset registered for {debridge_id_hex} — it cannot be bridged")
            })?;
        // A corridor the gate has not registered has no nonce, and `send` would
        // refuse it — so say so here rather than let the wallet find out.
        let nonce = cfg.nonce(chain_id_to).ok_or_else(|| {
            anyhow::anyhow!("corridor to chain {chain_id_to} is not registered on this gate")
        })?;

        let decimals = pool.mint_decimals(&b58(&asset.mint)).await.unwrap_or(0);
        Ok(GateSendContext {
            program_id: self.program.clone(),
            bridge_domain: format!("0x{}", hex_encode(&cfg.bridge_domain)),
            chain_id: cfg.chain_id,
            nonce,
            debridge_id: debridge_id_hex.to_string(),
            vault: b58(&asset.vault),
            decimals,
            paused: cfg.paused,
        })
    }
}

fn hex_32(s: &str) -> Option<[u8; 32]> {
    let h = s.strip_prefix("0x").unwrap_or(s);
    if h.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Base58, the form every Solana address is written in.
pub fn b58(key: &[u8; 32]) -> String {
    bs58::encode(key).into_string()
}

/// Decode a base58 address into the raw key the account layouts store.
pub fn from_b58(s: &str) -> Option<[u8; 32]> {
    let v = bs58::decode(s.trim()).into_vec().ok()?;
    v.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base58_round_trips() {
        let key = [7u8; 32];
        assert_eq!(from_b58(&b58(&key)), Some(key));
    }

    #[test]
    fn an_unknown_mint_is_not_given_an_invented_symbol() {
        let p = SolanaPool::new("http://localhost", "prog");
        assert_eq!(p.symbol_of("A4btyAotRPBvJZAusD5MwCzmR6m9UuKtoohRcH3VM28G"), "A4bt…");
    }
}
