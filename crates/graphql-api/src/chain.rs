//! Optional on-chain execution lookups for destination gates.
//!
//! The signature store only knows what validators have *signed*; it cannot say
//! whether the keeper has actually delivered a transfer. Given a destination
//! `chainId -> (rpc, gate)` mapping, this asks the gate's `executed(submissionId)`
//! so the API can distinguish "enough signatures" (READY) from "claimed on-chain"
//! (EXECUTED). Chains the API wasn't configured with simply report `null`.

use std::collections::BTreeMap;
use std::str::FromStr;

use alloy::primitives::{Address, B256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use anyhow::Context;
use bridge_core::abi::Gate;
use serde::{Deserialize, Serialize};

/// A network the bridge UI can target, served to the frontend via the `chains`
/// query so it discovers configured networks instead of hardcoding them.
///
/// Only `chain_id` and `name` are required; `gate`/`token` are deployed fresh by
/// the local scripts each run, so they're optional here and may be supplied (or
/// overridden) from the UI. When `rpc_url` + `gate` are present, the API also
/// registers the gate for on-chain `executed()` lookups (see [`Chains::add`]).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChainInfo {
    pub chain_id: u64,
    pub name: String,
    #[serde(default)]
    pub rpc_url: Option<String>,
    #[serde(default)]
    pub gate: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    /// Deployed `SwapRouter` on this chain, for cross-chain-swap (Phase F). Like
    /// `gate`/`token`, optional and re-deployed fresh by local scripts each run.
    #[serde(default)]
    pub router: Option<String>,
}

/// Load the chain registry from a JSON file (an array of [`ChainInfo`]).
pub fn load_registry(path: &str) -> anyhow::Result<Vec<ChainInfo>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading chains file {path}"))?;
    let chains: Vec<ChainInfo> =
        serde_json::from_str(&raw).with_context(|| format!("parsing chains file {path}"))?;
    for c in &chains {
        if let Some(g) = &c.gate {
            Address::from_str(g.trim())
                .with_context(|| format!("bad gate address for chain {} in {path}", c.chain_id))?;
        }
    }
    Ok(chains)
}

/// Split a `CHAINID=RPC,ADDR` spec (the shape both `--gate` and `--swap` use)
/// into its parts. `flag` names the flag in error messages.
pub(crate) fn split_spec<'s>(spec: &'s str, flag: &str) -> anyhow::Result<(u64, &'s str, &'s str)> {
    let (id_s, rest) = spec
        .split_once('=')
        .with_context(|| format!("{flag} must be CHAINID=RPC,ADDR, got {spec:?}"))?;
    let (rpc, addr_s) = rest
        .split_once(',')
        .with_context(|| format!("{flag} must be CHAINID=RPC,ADDR, got {spec:?}"))?;
    let chain_id: u64 =
        id_s.trim().parse().with_context(|| format!("bad chainId in {flag} {spec:?}"))?;
    Ok((chain_id, rpc, addr_s))
}

/// Parse an address + RPC url and build an (erased) HTTP provider. `ctx` names
/// the source in error messages (e.g. `"--gate 1338=...,0xabc"` or `"chain 1338"`).
pub(crate) fn provider_for(addr: &str, rpc: &str, ctx: &str) -> anyhow::Result<(DynProvider, Address)> {
    let address: Address =
        addr.trim().parse().with_context(|| format!("bad address in {ctx}"))?;
    let url = rpc.trim().parse().with_context(|| format!("bad rpc url in {ctx}"))?;
    let provider = ProviderBuilder::new().connect_http(url).erased();
    Ok((provider, address))
}

/// Destination gates the API can read execution status from. Cheap to clone
/// (each `DynProvider` is an `Arc` internally); share freely across resolvers.
#[derive(Clone, Default)]
pub struct Chains {
    gates: BTreeMap<u64, (DynProvider, Address)>,
}

impl Chains {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a destination from a `CHAINID=RPC,GATE` spec, e.g.
    /// `1338=http://127.0.0.1:8546,0xabc...`. The HTTP provider is built eagerly
    /// (no network I/O yet); the first `executed()` call is what hits the chain.
    pub fn add_spec(&mut self, spec: &str) -> anyhow::Result<u64> {
        let (chain_id, rpc, gate_s) = split_spec(spec, "--gate")?;
        let (provider, gate) = provider_for(gate_s, rpc, &format!("--gate {spec:?}"))?;
        self.gates.insert(chain_id, (provider, gate));
        Ok(chain_id)
    }

    /// Register a destination gate from already-parsed parts (used to fold the
    /// `--chains-file` registry into the executed-gate map). A no-op if this
    /// chain already has a gate (an explicit `--gate` wins).
    pub fn add(&mut self, chain_id: u64, rpc: &str, gate: &str) -> anyhow::Result<()> {
        if self.gates.contains_key(&chain_id) {
            return Ok(());
        }
        let (provider, gate) = provider_for(gate, rpc, &format!("chain {chain_id}"))?;
        self.gates.insert(chain_id, (provider, gate));
        Ok(())
    }

    /// The destination chainIds this API can report execution status for.
    pub fn configured(&self) -> Vec<u64> {
        self.gates.keys().copied().collect()
    }

    /// `executed(submissionId)` on the destination gate. `None` when `chain_id_to`
    /// isn't configured, the id is malformed, or the RPC call fails — a flaky RPC
    /// must never fail the whole GraphQL query, only leave status unknown.
    pub async fn executed(&self, chain_id_to: u64, submission_id: &str) -> Option<bool> {
        let (provider, gate) = self.gates.get(&chain_id_to)?;
        let id = B256::from_str(submission_id).ok()?;
        Gate::new(*gate, provider).executed(id).call().await.ok()
    }

    /// `cancelled(submissionId)` on the destination gate.
    ///
    /// `executed` alone cannot distinguish "delivered" from "burned so it could
    /// be refunded" — `cancel` sets the same flag. Reporting a cancelled
    /// transfer as EXECUTED would tell a user their funds arrived when in fact
    /// they were returned on the source chain, so the two are read together.
    pub async fn cancelled(&self, chain_id_to: u64, submission_id: &str) -> Option<bool> {
        let (provider, gate) = self.gates.get(&chain_id_to)?;
        let id = B256::from_str(submission_id).ok()?;
        Gate::new(*gate, provider).cancelled(id).call().await.ok()
    }
}
