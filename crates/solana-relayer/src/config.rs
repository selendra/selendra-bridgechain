use serde::Deserialize;

/// Relayer configuration. Mirrors the validator's shape (and its fail-closed
/// posture) so the two read alike, despite living in separate processes.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub source: SourceChain,
    pub signer: Signer,
    pub store: Store,
    /// Optional claim-submitting half. Absent => this process only SIGNS
    /// (Solana->EVM) and never delivers (EVM->Solana), which is a valid split:
    /// a validator should not have to be a keeper.
    #[serde(default)]
    pub target: Option<TargetChain>,
}

/// EVM -> Solana delivery. Mirrors the EVM keeper's `[[targets]]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetChain {
    /// Path to the Solana keypair that pays fees and signs claim transactions.
    /// This key holds NO bridge authority — the validator signatures carry it —
    /// so it only needs enough SOL for fees and rent.
    pub payer_keypair: String,
    #[serde(default = "default_poll")]
    pub poll_interval_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceChain {
    /// deBridge's chain id for Solana. Bound into every submissionId, so a wrong
    /// value silently produces ids no EVM gate will ever accept.
    pub chain_id: u64,
    /// Solana JSON-RPC endpoint.
    pub rpc: String,
    /// The deployed gate program, base58.
    pub program_id: String,
    /// **SECURITY CRITICAL.** Which commitment the scanner reads at.
    ///
    /// This is Solana's equivalent of the EVM side's `block_confirmation`: signing
    /// a `Sent` that a fork later discards lets the destination pay out against a
    /// deposit that no longer exists. `finalized` is the only safe choice on a real
    /// cluster — `confirmed` and `processed` can both be rolled back.
    /// [`Config::load`] refuses anything but `finalized` unless the operator opts
    /// out explicitly, exactly as the validator refuses a zero finality buffer.
    #[serde(default = "default_commitment")]
    pub commitment: String,
    /// Opt out of the `finalized`-only rule. ONLY for a local test validator.
    #[serde(default)]
    pub allow_unfinalized: bool,
    #[serde(default = "default_poll")]
    pub poll_interval_ms: u64,
    /// Where the resumable cursor (last processed signature) is persisted.
    #[serde(default = "default_state_file")]
    pub state_file: String,
    /// Cap on signatures fetched per tick.
    #[serde(default = "default_batch")]
    pub max_batch: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signer {
    /// The validator's secp256k1 key — the SAME key it uses on the EVM side, so
    /// one validator set signs for both VMs. `0x`-prefixed hex.
    #[serde(default)]
    pub private_key: Option<String>,
    /// Read the key from this environment variable instead (preferred).
    #[serde(default)]
    pub private_key_env: Option<String>,
}

impl Signer {
    /// Resolve the signing key, preferring the environment variable so a key need
    /// never be written to disk.
    pub fn resolve(&self) -> anyhow::Result<[u8; 32]> {
        let raw = match (&self.private_key_env, &self.private_key) {
            (Some(var), _) => std::env::var(var)
                .map_err(|_| anyhow::anyhow!("signer env var {var} is unset"))?,
            (None, Some(k)) => k.clone(),
            (None, None) => anyhow::bail!("no signing key: set private_key or private_key_env"),
        };
        let hex_str = raw.trim().strip_prefix("0x").unwrap_or_else(|| raw.trim());
        let bytes = hex::decode(hex_str).map_err(|_| anyhow::anyhow!("signer key is not hex"))?;
        let key: [u8; 32] =
            bytes.try_into().map_err(|_| anyhow::anyhow!("signer key must be 32 bytes"))?;
        Ok(key)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Store {
    /// Base URL of the sig-store, e.g. http://sig-store:8080
    pub url: String,
    /// Env var holding the bearer token. Defaults to the validator-scoped token,
    /// since this process does exactly what a validator does: it signs.
    #[serde(default = "default_token_env")]
    pub token_env: String,
}

fn default_commitment() -> String {
    "finalized".into()
}
fn default_poll() -> u64 {
    2000
}
fn default_state_file() -> String {
    "solana-relayer-state.json".into()
}
fn default_batch() -> usize {
    100
}
fn default_token_env() -> String {
    "SIG_STORE_VALIDATOR_TOKEN".into()
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading config {path}: {e}"))?;
        Self::from_toml(&raw)
    }

    /// Parse + validate. Split out from [`load`] so the fail-closed rule below is
    /// unit-testable without the filesystem.
    pub fn from_toml(raw: &str) -> anyhow::Result<Self> {
        let cfg: Config = toml::from_str(raw)?;

        // SECURITY: reading below `finalized` means signing a `Sent` a fork can
        // still discard — the destination would release liquidity against a
        // deposit that no longer exists. Same class of defect as signing at the
        // EVM chain tip.
        if cfg.source.commitment != "finalized" && !cfg.source.allow_unfinalized {
            anyhow::bail!(
                "commitment = {:?} — the relayer would sign Sent events that a fork can still \
                 discard, so the destination could pay out against a deposit that never settles. \
                 Use \"finalized\", or set allow_unfinalized = true ONLY for a local test \
                 validator.",
                cfg.source.commitment
            );
        }
        if !matches!(cfg.source.commitment.as_str(), "finalized" | "confirmed" | "processed") {
            anyhow::bail!("unknown commitment {:?}", cfg.source.commitment);
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(body: &str) -> String {
        format!(
            "[source]\n\
             chain_id = 7565164\n\
             rpc = \"http://127.0.0.1:8899\"\n\
             program_id = \"11111111111111111111111111111111\"\n\
             {body}\n\
             [signer]\n\
             private_key = \"0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d\"\n\
             [store]\n\
             url = \"http://127.0.0.1:8080\"\n"
        )
    }

    #[test]
    fn defaults_to_finalized() {
        let c = Config::from_toml(&cfg("")).expect("should load");
        assert_eq!(c.source.commitment, "finalized");
        assert!(!c.source.allow_unfinalized);
    }

    /// The fail-closed rule: anything a fork can still discard is refused.
    #[test]
    fn unfinalized_commitment_is_rejected_by_default() {
        for level in ["confirmed", "processed"] {
            let err = Config::from_toml(&cfg(&format!("commitment = \"{level}\"")))
                .unwrap_err()
                .to_string();
            assert!(err.contains("fork"), "{level} gave: {err}");
        }
    }

    #[test]
    fn unfinalized_opt_in_is_honored() {
        let c = Config::from_toml(&cfg("commitment = \"confirmed\"\nallow_unfinalized = true"))
            .expect("opt-in should load");
        assert_eq!(c.source.commitment, "confirmed");
    }

    #[test]
    fn misspelled_field_is_rejected_not_ignored() {
        let err = Config::from_toml(&cfg("commitmentt = \"finalized\"")).unwrap_err().to_string();
        assert!(err.contains("commitmentt") || err.contains("unknown field"), "got: {err}");
    }

    #[test]
    fn signer_key_resolves_from_hex() {
        let c = Config::from_toml(&cfg("")).unwrap();
        assert_eq!(c.signer.resolve().unwrap().len(), 32);
    }

    #[test]
    fn signer_without_any_source_is_rejected() {
        let s = Signer { private_key: None, private_key_env: None };
        assert!(s.resolve().is_err());
    }
}
