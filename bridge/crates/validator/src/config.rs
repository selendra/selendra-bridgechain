use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub source: SourceChain,
    pub signer: Signer,
    pub store: Store,
    /// Optional operator HTTP API (pause/resume/rescan/status).
    #[serde(default)]
    pub api: Option<Api>,
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

#[derive(Debug, Clone, Deserialize)]
pub struct Signer {
    /// dev-only raw key; production would use an encrypted keystore
    pub private_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Store {
    pub dir: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Api {
    /// e.g. "127.0.0.1:9090"
    pub bind: String,
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

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading config {path}: {e}"))?;
        let cfg: Config = toml::from_str(&raw)?;
        Ok(cfg)
    }
}
