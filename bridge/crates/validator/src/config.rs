use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub source: SourceChain,
    pub signer: Signer,
    pub store: Store,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SourceChain {
    pub chain_id: u64,
    pub rpc: String,
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

fn default_interval() -> u64 {
    1000
}
fn default_range() -> u64 {
    1000
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading config {path}: {e}"))?;
        let cfg: Config = toml::from_str(&raw)?;
        Ok(cfg)
    }
}
