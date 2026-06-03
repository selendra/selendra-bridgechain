use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub target: TargetChain,
    pub keeper: Keeper,
    pub store: Store,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TargetChain {
    pub chain_id: u64,
    pub rpc: String,
    pub gate: String,
    #[serde(default = "default_interval")]
    pub poll_interval_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Keeper {
    /// funded key on the target chain that pays gas for `claim()`
    pub private_key: String,
}

/// Where the keeper reads signatures: a local directory (`dir`) or the HTTP
/// sig-store (`url`). `url` wins when both are set.
#[derive(Debug, Clone, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

fn default_interval() -> u64 {
    1000
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading config {path}: {e}"))?;
        Ok(toml::from_str(&raw)?)
    }
}
