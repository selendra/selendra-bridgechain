use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Postgres connection string. Falls back to the `DATABASE_URL` env var.
    #[serde(default)]
    pub database_url: Option<String>,
    /// One block per chain to mirror events from.
    pub chains: Vec<ChainCfg>,
    /// How long an unclaimed transfer sits before being flagged refund-eligible,
    /// which nominates it for a validator cancel attestation (the validators
    /// re-check the destination on-chain before acting). See
    /// `bridge_db::Db::sweep_refund_eligible`.
    #[serde(default = "default_refund_timeout_secs")]
    pub refund_timeout_secs: i64,
    /// How often the eligibility sweep runs. The default suits production; tests
    /// lower it so a stranded transfer is nominated promptly.
    #[serde(default = "default_sweep_interval_secs")]
    pub sweep_interval_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChainCfg {
    pub chain_id: u64,
    pub rpc: String,
    /// Gate address on this chain — indexes `Sent`/`Claimed`. Omit to skip.
    #[serde(default)]
    pub gate: Option<String>,
    /// SwapRouter address on this chain — indexes `SwapBridged`/`Finalized`/
    /// `FinalizeFallback`. Omit to skip.
    #[serde(default)]
    pub router: Option<String>,
    /// SwapPool address on this chain — indexes `Swapped`. Omit to skip.
    #[serde(default)]
    pub pool: Option<String>,
    #[serde(default)]
    pub start_block: u64,
    /// Finality buffer: only process up to `latest - block_confirmation`.
    #[serde(default)]
    pub block_confirmation: u64,
    #[serde(default = "default_interval")]
    pub poll_interval_ms: u64,
    #[serde(default = "default_range")]
    pub max_block_range: u64,
}

fn default_interval() -> u64 {
    2000
}
fn default_range() -> u64 {
    2000
}
fn default_refund_timeout_secs() -> i64 {
    24 * 60 * 60
}
fn default_sweep_interval_secs() -> u64 {
    60
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading config {path}: {e}"))?;
        let cfg: Config = toml::from_str(&raw)?;
        anyhow::ensure!(!cfg.chains.is_empty(), "config needs at least one [[chains]] block");
        for i in 0..cfg.chains.len() {
            for j in (i + 1)..cfg.chains.len() {
                if cfg.chains[i].chain_id == cfg.chains[j].chain_id {
                    anyhow::bail!("duplicate chain_id {} in config", cfg.chains[i].chain_id);
                }
            }
        }
        Ok(cfg)
    }

    pub fn resolved_database_url(&self) -> anyhow::Result<String> {
        self.database_url
            .clone()
            .or_else(|| std::env::var("DATABASE_URL").ok())
            .ok_or_else(|| anyhow::anyhow!("no database_url configured and DATABASE_URL env unset"))
    }
}
