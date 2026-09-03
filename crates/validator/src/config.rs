use bridge_core::backend::StoreConfig;
use bridge_core::config::ensure_unique;
use bridge_core::signer::SignerConfig;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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
    pub store: StoreConfig,
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
#[serde(deny_unknown_fields)]
pub struct RefundConfig {
    /// **ENFORCED HERE — finding H-2.** How long a transfer must sit unclaimed
    /// before this validator will attest a cancel.
    ///
    /// This used to be advisory, with the real gate being the indexer's
    /// eligibility sweep flipping `refund_status = 'eligible'` in Postgres. That
    /// put the entire unclaimed-timeout on a database column no validator
    /// verified: a wrong `created_at`, clock skew, or DB write access nominated
    /// healthy in-flight transfers, and the validators attested cancels for them
    /// within one poll interval — irreversibly foreclosing payouts the keeper was
    /// still about to deliver.
    ///
    /// The loop now establishes the age itself, from the source chain, by reading
    /// `sentBy(id)` at a block whose own timestamp is at least this many seconds
    /// behind the chain head. The store still nominates candidates; it no longer
    /// decides when one is old enough. Set it to match the indexer's
    /// `refund_timeout_secs` so the two agree on the intended window — but the
    /// value here is the one that binds.
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
#[serde(deny_unknown_fields)]
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
        endpoints(&self.rpc, &self.rpcs, &format!("refund destination {}", self.chain_id))
    }
}

/// Resolve the `rpc` / `rpcs` pair either block accepts into one ordered,
/// deduplicated, non-empty endpoint list. `what` names the block in the error.
///
/// The single-`rpc` form is back-compat; when both are given the singular one
/// leads, so an operator adding `rpcs` for failover keeps their existing primary.
fn endpoints(rpc: &Option<String>, rpcs: &[String], what: &str) -> anyhow::Result<Vec<String>> {
    let mut out = rpcs.to_vec();
    if let Some(rpc) = rpc {
        if !out.iter().any(|u| u == rpc) {
            out.insert(0, rpc.clone());
        }
    }
    anyhow::ensure!(!out.is_empty(), "{what} has no RPC endpoints (set `rpc` or `rpcs`)");
    Ok(out)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// **Finality buffer — SECURITY CRITICAL.** Only process up to
    /// `latest - block_confirmation`. Signing a `Sent` event at the chain tip lets
    /// a source reorg erase the deposit *after* validators have signed and the
    /// keeper has released destination liquidity — a double-spend of bridge funds.
    /// This MUST exceed the source chain's maximum reorg depth. `Config::load`
    /// refuses to start with a 0 buffer unless `allow_zero_confirmation` is set.
    #[serde(default)]
    pub block_confirmation: u64,
    /// Opt out of the non-zero `block_confirmation` requirement. ONLY for
    /// instant-finality local chains (anvil) that never reorg. Never set this
    /// against a real network. Mirrors `RefundConfig.allow_zero_confirmation`.
    #[serde(default)]
    pub allow_zero_confirmation: bool,
    #[serde(default = "default_interval")]
    pub poll_interval_ms: u64,
    /// Delay between windows while CATCHING UP — i.e. when the last range was
    /// capped by `max_block_range` and confirmed history is still unread.
    ///
    /// Defaults to `poll_interval_ms`, which is the conservative choice: how
    /// fast a scanner may read is a property of the ENDPOINT, not of the
    /// backlog. Reading back-to-back is what clears a fast chain's gap in
    /// minutes instead of hours, but on a shared rate-limited endpoint it also
    /// starves every other consumer of the same key — the API's pool reads and
    /// the indexer included — which shows up as 429s, not as slowness. So lower
    /// it only for an endpoint you know can take it (your own node, or a public
    /// RPC with a generous cap).
    #[serde(default)]
    pub catchup_poll_interval_ms: Option<u64>,
    #[serde(default = "default_range")]
    pub max_block_range: u64,
    /// Where to persist the resumable cursor + per-chain nonce state.
    #[serde(default = "default_state_file")]
    pub state_file: String,
}

impl SourceChain {
    /// Resolve the configured endpoints into a non-empty ordered list.
    pub fn endpoints(&self) -> anyhow::Result<Vec<String>> {
        endpoints(&self.rpc, &self.rpcs, &format!("source chain {}", self.chain_id))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Api {
    /// e.g. "127.0.0.1:9090"
    pub bind: String,
    /// Bearer token guarding pause/resume/rescan. Falls back to the
    /// `VALIDATOR_API_TOKEN` env var. Unset on both means the control routes are
    /// not served at all unless `allow_unauthenticated` says otherwise.
    #[serde(default)]
    pub token: Option<String>,
    /// Serve pause/resume/rescan with NO authentication when no token is set.
    /// Dev only — those routes can halt this validator out of quorum.
    #[serde(default)]
    pub allow_unauthenticated: bool,
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
        Self::from_toml(&raw)
    }

    /// Parse + validate a config from a TOML string. Split out from [`load`] so the
    /// fail-closed checks below can be unit-tested without touching the filesystem.
    pub fn from_toml(raw: &str) -> anyhow::Result<Self> {
        let mut cfg: Config = toml::from_str(raw)?;

        // Backward compatibility: a single `[source]` is a one-element list.
        if let Some(s) = cfg.source.take() {
            cfg.sources.insert(0, s);
        }
        if cfg.sources.is_empty() {
            anyhow::bail!("config needs at least one [[sources]] block (or a legacy [source])");
        }

        // Each source must be a distinct chain and own a distinct state file,
        // otherwise two scan loops would clobber each other's cursor.
        ensure_unique(&cfg.sources, |s| s.chain_id, "source chain_id")?;
        ensure_unique(&cfg.sources, |s| s.state_file.as_str(), "source state_file")?;

        // SECURITY: signing a `Sent` event at the source chain tip lets a reorg
        // erase the deposit *after* the keeper has already released destination
        // liquidity — a double-spend of bridge funds. Refuse a 0 finality buffer
        // unless the operator explicitly opts out for an instant-finality dev
        // chain. (Mirrors the refund reader's guard below; before this check the
        // shipped `allow_zero_confirmation` on `[source]` was silently ignored.)
        for s in &cfg.sources {
            if s.block_confirmation == 0 && !s.allow_zero_confirmation {
                anyhow::bail!(
                    "source chain_id {} has block_confirmation = 0 — the validator would sign \
                     Sent events at the chain tip, so a source reorg could erase a deposit after \
                     the destination was paid (double-spend). Set block_confirmation to exceed \
                     the source chain's finality depth, or set allow_zero_confirmation = true \
                     ONLY for an instant-finality dev chain (e.g. anvil).",
                    s.chain_id
                );
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
            ensure_unique(&refund.destinations, |d| d.chain_id, "refund destination chain_id")?;

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

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal single-source config with an explicit finality buffer; each test
    // tweaks the `[source]` block to exercise the fail-closed rule.
    fn cfg(source_body: &str) -> String {
        format!(
            "[source]\n\
             chain_id = 1337\n\
             rpcs = [\"http://localhost:8545\"]\n\
             gate = \"0x0000000000000000000000000000000000000001\"\n\
             {source_body}\n\
             [signer]\n\
             private_key = \"0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d\"\n\
             [store]\n\
             dir = \"./sigs\"\n"
        )
    }

    #[test]
    fn source_zero_confirmation_is_rejected_by_default() {
        // Omitted block_confirmation defaults to 0 -> must fail closed.
        let err = Config::from_toml(&cfg("")).unwrap_err().to_string();
        assert!(err.contains("block_confirmation = 0"), "got: {err}");

        // Explicit 0 without the opt-in -> must fail closed.
        let err = Config::from_toml(&cfg("block_confirmation = 0"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("block_confirmation = 0"), "got: {err}");
    }

    #[test]
    fn source_zero_confirmation_opt_in_is_honored() {
        // The opt-in must actually be read (the H1 bug: it was silently dropped).
        let c = Config::from_toml(&cfg("block_confirmation = 0\nallow_zero_confirmation = true"))
            .expect("opt-in should load");
        assert!(c.sources[0].allow_zero_confirmation);
        assert_eq!(c.sources[0].block_confirmation, 0);
    }

    #[test]
    fn source_nonzero_confirmation_is_accepted() {
        let c = Config::from_toml(&cfg("block_confirmation = 12")).expect("nonzero should load");
        assert_eq!(c.sources[0].block_confirmation, 12);
        assert!(!c.sources[0].allow_zero_confirmation);
    }

    #[test]
    fn misspelled_field_is_rejected_not_ignored() {
        // deny_unknown_fields: a typo like `allow_zero_confirmations` (trailing s)
        // must be an error, not a silently-ignored no-op that leaves buffer 0.
        let err = Config::from_toml(&cfg(
            "block_confirmation = 0\nallow_zero_confirmations = true",
        ))
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("allow_zero_confirmations") || err.contains("unknown field"),
            "got: {err}"
        );
    }
}
