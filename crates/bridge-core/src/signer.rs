//! Signing-key custody for the relayer nodes (validator + keeper).
//!
//! A bridge's safety rests on validators holding *distinct, well-guarded* keys:
//! the on-chain Gate needs a threshold of them to move funds, so no single key is
//! ever enough. But that guarantee is only as strong as how each node stores its
//! key. A raw `private_key` sitting in a TOML file turns "leak the config" into
//! "leak the key" — one compromised file and the attacker owns a validator seat.
//!
//! This loader keeps the secret out of the config and off disk in plaintext by
//! accepting, in order of preference:
//!   * `keystore` — an encrypted Web3 Secret Storage JSON (the `geth` / `cast
//!     wallet` format), unlocked by a password from a file, env var, or (dev)
//!     inline value;
//!   * `private_key_env` — the raw hex key injected via an env var (Docker /
//!     systemd secret), so it never touches the config;
//!   * `private_key` — the raw hex key inline. DEV ONLY; logged as a warning.
//!
//! Exactly one key source must be set, and (for a keystore) exactly one password
//! source, so a fat-fingered config fails loudly at startup rather than silently
//! preferring one secret over another.

use alloy::signers::local::PrivateKeySigner;
use anyhow::{bail, Context};
use serde::Deserialize;

/// How a relayer node obtains its signing key. Drop-in for a `[signer]` /
/// `[keeper]` config section; every field is optional so existing configs that
/// only set `private_key` keep working unchanged.
#[derive(Clone, Default, Deserialize)]
pub struct SignerConfig {
    /// Raw hex key inline. DEV ONLY — a leaked config file is a leaked key.
    #[serde(default)]
    pub private_key: Option<String>,
    /// Name of an env var holding the raw hex key (keeps it out of the file).
    #[serde(default)]
    pub private_key_env: Option<String>,
    /// Path to an encrypted keystore JSON (Web3 Secret Storage / `cast wallet`).
    #[serde(default)]
    pub keystore: Option<String>,
    /// Keystore password inline. DEV ONLY — prefer the `_file` / `_env` variants.
    #[serde(default)]
    pub keystore_password: Option<String>,
    /// Env var holding the keystore password.
    #[serde(default)]
    pub keystore_password_env: Option<String>,
    /// File whose contents are the keystore password (trailing newline trimmed).
    #[serde(default)]
    pub keystore_password_file: Option<String>,
}

impl SignerConfig {
    /// Resolve the configured key source into a usable signer.
    ///
    /// `role` is used only for log/error context (e.g. `"validator"`, `"keeper"`).
    pub fn load(&self, role: &str) -> anyhow::Result<PrivateKeySigner> {
        // Mutually-exclusive key sources. Counting them means both "none" and
        // "more than one" are caught, instead of silently picking a winner.
        match [
            self.keystore.is_some(),
            self.private_key_env.is_some(),
            self.private_key.is_some(),
        ]
        .iter()
        .filter(|set| **set)
        .count()
        {
            0 => bail!(
                "{role} signer: no key configured — set `keystore` (recommended), \
                 `private_key_env`, or `private_key`"
            ),
            1 => {}
            _ => bail!(
                "{role} signer: more than one key source set — use exactly one of \
                 `keystore`, `private_key_env`, `private_key`"
            ),
        }

        if let Some(path) = &self.keystore {
            let password = self.keystore_password()?;
            let signer = PrivateKeySigner::decrypt_keystore(path, password)
                .with_context(|| format!("{role} signer: decrypting keystore {path}"))?;
            tracing::info!(
                role, signer = %signer.address(), keystore = %path,
                "loaded signer from encrypted keystore"
            );
            return Ok(signer);
        }

        if let Some(var) = &self.private_key_env {
            let raw = std::env::var(var)
                .with_context(|| format!("{role} signer: env var {var} is not set"))?;
            let signer: PrivateKeySigner = raw
                .trim()
                .parse()
                .with_context(|| format!("{role} signer: invalid hex key in env var {var}"))?;
            tracing::info!(
                role, signer = %signer.address(), env = %var,
                "loaded signer from env var"
            );
            return Ok(signer);
        }

        // Inline raw key: dev only. `unwrap` is safe — the count above proved it.
        let raw = self.private_key.as_ref().unwrap();
        let signer: PrivateKeySigner = raw
            .trim()
            .parse()
            .with_context(|| format!("{role} signer: invalid inline private_key"))?;
        tracing::warn!(
            role, signer = %signer.address(),
            "using an INLINE private_key from config (DEV ONLY) — in production use an \
             encrypted `keystore` or `private_key_env` so the key isn't stored in plaintext"
        );
        Ok(signer)
    }

    /// Resolve the keystore password from exactly one of the three sources.
    fn keystore_password(&self) -> anyhow::Result<String> {
        match [
            self.keystore_password_file.is_some(),
            self.keystore_password_env.is_some(),
            self.keystore_password.is_some(),
        ]
        .iter()
        .filter(|set| **set)
        .count()
        {
            0 => bail!(
                "keystore is set but no password — provide `keystore_password_file` \
                 (recommended), `keystore_password_env`, or `keystore_password`"
            ),
            1 => {}
            _ => bail!(
                "more than one keystore password source set — use exactly one of \
                 `keystore_password_file`, `keystore_password_env`, `keystore_password`"
            ),
        }

        if let Some(file) = &self.keystore_password_file {
            let raw = std::fs::read_to_string(file)
                .with_context(|| format!("reading keystore_password_file {file}"))?;
            // Trim the trailing newline `printf 'pw\n' > file` / editors add.
            return Ok(raw.trim_end_matches(['\r', '\n']).to_string());
        }
        if let Some(var) = &self.keystore_password_env {
            return std::env::var(var)
                .with_context(|| format!("keystore_password_env {var} is not set"));
        }
        Ok(self.keystore_password.clone().unwrap())
    }
}

/// Encode an alloy signature as 65 bytes `r||s||v` with `v` in {27,28} — the
/// OpenZeppelin ECDSA form every `Gate` entry point expects, and the form the
/// sig-store recovers against.
///
/// One definition, because a divergent one here is silent: a signature encoded
/// with `v` in {0,1} recovers to a different address, so it is dropped as
/// "not a validator" rather than reported as malformed.
pub fn encode_signature(sig: &alloy::primitives::Signature) -> String {
    format!("0x{}", hex::encode(signature_bytes(sig)))
}

/// The raw 65 bytes behind [`encode_signature`], for callers that hand the
/// signature to a contract or an instruction rather than to the store.
pub fn signature_bytes(sig: &alloy::primitives::Signature) -> [u8; 65] {
    let mut out = [0u8; 65];
    out[..32].copy_from_slice(&sig.r().to_be_bytes::<32>());
    out[32..64].copy_from_slice(&sig.s().to_be_bytes::<32>());
    out[64] = 27 + sig.v() as u8;
    out
}

// Never let a secret leak into logs/panics via `{:?}`. The config struct that
// embeds this derives `Debug`, so redact rather than skip the impl.
impl std::fmt::Debug for SignerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redact = |o: &Option<String>| o.as_ref().map(|_| "<redacted>");
        f.debug_struct("SignerConfig")
            .field("private_key", &redact(&self.private_key))
            .field("private_key_env", &self.private_key_env)
            .field("keystore", &self.keystore)
            .field("keystore_password", &redact(&self.keystore_password))
            .field("keystore_password_env", &self.keystore_password_env)
            .field("keystore_password_file", &self.keystore_password_file)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // go-ethereum's canonical Secret-Storage test vector (pbkdf2 KDF).
    const KEYSTORE_JSON: &str = r#"{
      "crypto": {
        "cipher": "aes-128-ctr",
        "cipherparams": { "iv": "6087dab2f9fdbbfaddc31a909735c1e6" },
        "ciphertext": "5318b4d5bcd28de64ee5559e671353e16f075ecae9f99c7a79a38af5f869aa46",
        "kdf": "pbkdf2",
        "kdfparams": {
          "c": 262144,
          "dklen": 32,
          "prf": "hmac-sha256",
          "salt": "ae3cd4e7013836a3df6bd7241b12db061dbe2c6785853cce422d148a624ce0bd"
        },
        "mac": "517ead924a9d0dc3124507e3393d175ce3ff7c1e96529c6c555ce9e51205e9b2"
      },
      "id": "3198bc9c-6672-5ab3-d995-4942343ae5b6",
      "version": 3
    }"#;
    const KEYSTORE_PW: &str = "testpassword";
    const KEYSTORE_PK: &str = "0x7a28b5ba57c53603b0b07b56bba752f7784bf506fa95edc395f5cf6c7514fe9d";

    // Unique temp path (parallel tests share the same temp dir / process).
    fn temp_path(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "bridge-signer-test-{}-{}-{tag}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn write(path: &std::path::Path, contents: &str) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
    }

    fn expected_addr() -> alloy::primitives::Address {
        KEYSTORE_PK.parse::<PrivateKeySigner>().unwrap().address()
    }

    #[test]
    fn inline_private_key_loads() {
        let cfg = SignerConfig { private_key: Some(KEYSTORE_PK.into()), ..Default::default() };
        assert_eq!(cfg.load("test").unwrap().address(), expected_addr());
    }

    #[test]
    fn private_key_from_env_loads() {
        let var = "BRIDGE_TEST_KEY_ENV";
        std::env::set_var(var, KEYSTORE_PK);
        let cfg = SignerConfig { private_key_env: Some(var.into()), ..Default::default() };
        assert_eq!(cfg.load("test").unwrap().address(), expected_addr());
        std::env::remove_var(var);
    }

    #[test]
    fn keystore_with_password_file_decrypts() {
        let ks = temp_path("ks.json");
        let pw = temp_path("pw.txt");
        write(&ks, KEYSTORE_JSON);
        write(&pw, &format!("{KEYSTORE_PW}\n")); // trailing newline must be trimmed
        let cfg = SignerConfig {
            keystore: Some(ks.to_string_lossy().into_owned()),
            keystore_password_file: Some(pw.to_string_lossy().into_owned()),
            ..Default::default()
        };
        assert_eq!(cfg.load("test").unwrap().address(), expected_addr());
        let _ = std::fs::remove_file(&ks);
        let _ = std::fs::remove_file(&pw);
    }

    #[test]
    fn keystore_with_inline_password_decrypts() {
        let ks = temp_path("ks2.json");
        write(&ks, KEYSTORE_JSON);
        let cfg = SignerConfig {
            keystore: Some(ks.to_string_lossy().into_owned()),
            keystore_password: Some(KEYSTORE_PW.into()),
            ..Default::default()
        };
        assert_eq!(cfg.load("test").unwrap().address(), expected_addr());
        let _ = std::fs::remove_file(&ks);
    }

    #[test]
    fn no_key_source_is_rejected() {
        let err = SignerConfig::default().load("test").unwrap_err().to_string();
        assert!(err.contains("no key configured"), "{err}");
    }

    #[test]
    fn multiple_key_sources_are_rejected() {
        let cfg = SignerConfig {
            private_key: Some(KEYSTORE_PK.into()),
            private_key_env: Some("SOMEVAR".into()),
            ..Default::default()
        };
        let err = cfg.load("test").unwrap_err().to_string();
        assert!(err.contains("more than one key source"), "{err}");
    }

    #[test]
    fn keystore_without_password_is_rejected() {
        let cfg = SignerConfig { keystore: Some("/nope.json".into()), ..Default::default() };
        let err = cfg.load("test").unwrap_err().to_string();
        assert!(err.contains("no password"), "{err}");
    }

    #[test]
    fn wrong_keystore_password_fails() {
        let ks = temp_path("ks3.json");
        write(&ks, KEYSTORE_JSON);
        let cfg = SignerConfig {
            keystore: Some(ks.to_string_lossy().into_owned()),
            keystore_password: Some("wrong".into()),
            ..Default::default()
        };
        assert!(cfg.load("test").is_err());
        let _ = std::fs::remove_file(&ks);
    }

    #[test]
    fn debug_redacts_secrets() {
        let cfg = SignerConfig {
            private_key: Some(KEYSTORE_PK.into()),
            keystore_password: Some(KEYSTORE_PW.into()),
            ..Default::default()
        };
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains(KEYSTORE_PK), "private_key leaked: {dbg}");
        assert!(!dbg.contains(KEYSTORE_PW), "password leaked: {dbg}");
        assert!(dbg.contains("<redacted>"), "{dbg}");
    }
}
