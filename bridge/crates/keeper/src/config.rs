use bridge_core::signer::SignerConfig;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Legacy single-target form: `[target]`. Folded into `targets` on load.
    #[serde(default)]
    pub target: Option<TargetChain>,
    /// Multi-target form: one `[[targets]]` block per destination chain the
    /// keeper should deliver claims to (e.g. chainB *and* chainC).
    #[serde(default)]
    pub targets: Vec<TargetChain>,
    /// How the keeper holds the funded gas-payer key that signs `claim()` txs
    /// (raw dev key, env var, or an encrypted keystore). See [`SignerConfig`].
    pub keeper: SignerConfig,
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
        let mut cfg: Config = toml::from_str(&raw)?;

        // Backward compatibility: a single `[target]` is just a one-element list.
        if let Some(t) = cfg.target.take() {
            cfg.targets.insert(0, t);
        }
        if cfg.targets.is_empty() {
            anyhow::bail!("config needs at least one [[targets]] block (or a legacy [target])");
        }

        // Guard against two blocks claiming the same destination chain.
        for i in 0..cfg.targets.len() {
            for j in (i + 1)..cfg.targets.len() {
                if cfg.targets[i].chain_id == cfg.targets[j].chain_id {
                    anyhow::bail!(
                        "duplicate target chain_id {} in config",
                        cfg.targets[i].chain_id
                    );
                }
            }
        }

        Ok(cfg)
    }
}
