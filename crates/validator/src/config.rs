use bridge_core::signer::SignerConfig;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Legacy single-source form: `[source]`. Folded into `sources` on load.
    #[serde(default)]
    pub source: Option<SourceChain>,
    /// Multi-source form: one `[[sources]]` block per chain to watch, so a single
    /// validator process can sign transfers originating on B *and* C.
    #[serde(default)]
    pub sources: Vec<SourceChain>,
    /// How this node holds its signing key (raw dev key, env var, or — for
    /// production — an encrypted keystore). See [`SignerConfig`].
    pub signer: SignerConfig,
    pub store: Store,
    /// Optional operator HTTP API (pause/resume/rescan/status).
    #[serde(default)]
    pub api: Option<Api>,
    /// Optional refund attestation loop. Absent => this validator never attests
    /// cancels or refunds, and stuck transfers stay stuck (safe default: a
    /// validator that cannot see the destination chain must not vote on whether
    /// a transfer was delivered).
    #[serde(default)]
    pub refund: Option<RefundConfig>,
}

/// Drives the two-phase refund attestation loop.
#[derive(Debug, Clone, Deserialize)]
pub struct RefundConfig {
    /// Advisory only, surfaced in the startup log. The unclaimed-timeout that
    /// actually gates cancel attestations is enforced upstream by the indexer's
    /// eligibility sweep (`indexer.refund_timeout_secs`), which flips a transfer
    /// to `refund_status = 'eligible'`; this loop only ever sees candidates the
    /// store has already nominated. Kept so the intended window is visible in one
    /// place; set it to match the indexer's value.
    #[serde(default = "default_refund_timeout")]
    pub timeout_secs: i64,
    #[serde(default = "default_refund_interval")]
    pub poll_interval_ms: u64,
    /// **Finality buffer — SECURITY CRITICAL.** `executed`/`cancelled`/`sentBy`
    /// are read at `latest - block_confirmation`. A refund on the source chain is
    /// irreversible once it pays out, and it is authorised solely on having read
    /// `cancelled == true` on the destination. If that read is at the chain tip
    /// (buffer 0) and the destination later reorgs the `cancel` away, the
    /// original claim signatures become live again → the transfer is paid on the
    /// destination AND refunded on the source (a double-spend of bridge
    /// liquidity). This MUST exceed the destination chain's maximum reorg depth
    /// (its finality). `Config::load` refuses to start with a 0 buffer unless
    /// `allow_zero_confirmation` is set (only safe on instant-finality dev chains
    /// such as anvil).
    #[serde(default)]
    pub block_confirmation: u64,
    /// Opt out of the non-zero `block_confirmation` requirement. ONLY for
    /// instant-finality local chains (anvil) that never reorg. Never set this
    /// against a real network.
    #[serde(default)]
    pub allow_zero_confirmation: bool,
    /// Every destination chain this validator can independently verify. A
    /// transfer bound for a chain not listed here is never attested.
    #[serde(default)]
    pub destinations: Vec<RefundChain>,
}

/// One chain the refund loop can read gate state from.
#[derive(Debug, Clone, Deserialize)]
pub struct RefundChain {
    pub chain_id: u64,
    #[serde(default)]
    pub rpc: Option<String>,
    #[serde(default)]
    pub rpcs: Vec<String>,
    pub gate: String,
}

impl RefundChain {
    pub fn endpoints(&self) -> anyhow::Result<Vec<String>> {
        let mut out = self.rpcs.clone();
        if let Some(rpc) = &self.rpc {
            if !out.iter().any(|u| u == rpc) {
                out.insert(0, rpc.clone());
            }
        }
        anyhow::ensure!(
            !out.is_empty(),
            "refund destination {} has no RPC endpoints (set `rpc` or `rpcs`)",
            self.chain_id
        );
        Ok(out)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SourceChain {
    pub chain_id: u64,
    /// Single RPC (back-compat). Prefer `rpcs` for failover.
    #[serde(default)]
    pub rpc: Option<String>,
    /// Ordered list of RPC endpoints; the validator fails over to the next on error.
    #[serde(default)]
    pub rpcs: Vec<String>,
    pub gate: String,
    #[serde(default)]
    pub start_block: u64,
    /// finality buffer: only process up to `latest - block_confirmation`
    #[serde(default)]
    pub block_confirmation: u64,
    #[serde(default = "default_interval")]
    pub poll_interval_ms: u64,
    #[serde(default = "default_range")]
    pub max_block_range: u64,
    /// Where to persist the resumable cursor + per-chain nonce state.
    #[serde(default = "default_state_file")]
    pub state_file: String,
}

impl SourceChain {
    /// Resolve the configured endpoints into a non-empty ordered list.
    pub fn endpoints(&self) -> anyhow::Result<Vec<String>> {
        let mut out = self.rpcs.clone();
        if let Some(rpc) = &self.rpc {
            if !out.iter().any(|u| u == rpc) {
                out.insert(0, rpc.clone());
            }
        }
        anyhow::ensure!(!out.is_empty(), "no RPC endpoints configured (set `rpc` or `rpcs`)");
        Ok(out)
    }
}

/// Where signatures go. Either a local directory (`dir`) or the HTTP sig-store
/// (`url`). `url` wins when both are set.
#[derive(Debug, Clone, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Api {
    /// e.g. "127.0.0.1:9090"
    pub bind: String,
    /// Bearer token guarding pause/resume/rescan. Falls back to the
    /// `VALIDATOR_API_TOKEN` env var; unset on both => unauthenticated (dev).
    #[serde(default)]
    pub token: Option<String>,
}

impl Api {
    /// The configured token, or the `VALIDATOR_API_TOKEN` env var as a fallback.
    pub fn resolved_token(&self) -> Option<String> {
        self.token
            .clone()
            .filter(|t| !t.is_empty())
            .or_else(|| std::env::var("VALIDATOR_API_TOKEN").ok().filter(|t| !t.is_empty()))
    }
}

fn default_interval() -> u64 {
    1000
}
fn default_range() -> u64 {
    1000
}
fn default_state_file() -> String {
    "validator-state.json".into()
}
fn default_refund_timeout() -> i64 {
    3600
}
fn default_refund_interval() -> u64 {
    15_000
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading config {path}: {e}"))?;
        let mut cfg: Config = toml::from_str(&raw)?;

        // Backward compatibility: a single `[source]` is a one-element list.
        if let Some(s) = cfg.source.take() {
            cfg.sources.insert(0, s);
        }
        if cfg.sources.is_empty() {
            anyhow::bail!("config needs at least one [[sources]] block (or a legacy [source])");
        }

        // Each source must be a distinct chain and own a distinct state file,
        // otherwise two scan loops would clobber each other's cursor.
        for i in 0..cfg.sources.len() {
            for j in (i + 1)..cfg.sources.len() {
                if cfg.sources[i].chain_id == cfg.sources[j].chain_id {
                    anyhow::bail!("duplicate source chain_id {} in config", cfg.sources[i].chain_id);
                }
                if cfg.sources[i].state_file == cfg.sources[j].state_file {
                    anyhow::bail!(
                        "sources for chains {} and {} share state_file {:?}; give each its own",
                        cfg.sources[i].chain_id,
                        cfg.sources[j].chain_id,
                        cfg.sources[i].state_file
                    );
                }
            }
        }

        // A refund block with no destinations can never attest anything; that is
        // almost certainly a misconfiguration rather than an intent to disable.
        if let Some(refund) = &cfg.refund {
            if refund.destinations.is_empty() {
                anyhow::bail!(
                    "[refund] has no [[refund.destinations]]; remove the block to disable \
                     refund attestation, or list the destination chains to verify"
                );
            }
            for i in 0..refund.destinations.len() {
                for j in (i + 1)..refund.destinations.len() {
                    if refund.destinations[i].chain_id == refund.destinations[j].chain_id {
                        anyhow::bail!(
                            "duplicate refund destination chain_id {}",
                            refund.destinations[i].chain_id
                        );
                    }
                }
            }

            // SECURITY: a source-chain refund is irreversible and is authorised
            // only on a destination `cancelled` read. Reading at the chain tip
            // lets a destination reorg re-enable the original claim after the
            // refund is signed → double-spend. Refuse to start at buffer 0 unless
            // the operator explicitly opts out for an instant-finality dev chain.
            if refund.block_confirmation == 0 && !refund.allow_zero_confirmation {
                anyhow::bail!(
                    "[refund] block_confirmation is 0 — refund attestations would read the \
                     destination at the chain tip, so a reorg could enable a double-spend. \
                     Set block_confirmation to exceed the destination chain's finality depth, \
                     or set allow_zero_confirmation = true ONLY for an instant-finality dev \
                     chain (e.g. anvil)."
                );
            }

            // SECURITY / liveness: the refund timeout that stops validators from
            // cancelling a still-deliverable transfer lives in the DB-backed
            // eligibility sweep (indexer + sig-store). In file-store mode there is
            // no such gate — refund_candidates would return every record and the
            // loop would attest a cancel for fresh, undelivered transfers. Require
            // the HTTP store for the refund path.
            if cfg.store.url.is_none() {
                anyhow::bail!(
                    "[refund] requires an HTTP [store] (url = \"http://sig-store…\"): the \
                     unclaimed-timeout gate is enforced by the DB-backed eligibility sweep, \
                     which the local file store does not provide. Without it the loop would \
                     cancel transfers before the keeper can deliver them."
                );
            }
        }

        Ok(cfg)
    }
}
