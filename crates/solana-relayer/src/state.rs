use serde::{Deserialize, Serialize};

/// Resumable scan cursor: the last Solana transaction signature fully handled.
///
/// Persisted after each transaction, so a restart re-scans from there rather than
/// from genesis — and never skips a `Sent` that was observed but not yet stored.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Cursor {
    pub last_signature: Option<String>,
}

impl Cursor {
    pub fn load_or_init(path: &str) -> anyhow::Result<Cursor> {
        match std::fs::read_to_string(path) {
            Ok(raw) => Ok(serde_json::from_str(&raw)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Cursor::default()),
            Err(e) => Err(anyhow::anyhow!("reading cursor {path}: {e}")),
        }
    }

    pub fn save(&self, path: &str) -> anyhow::Result<()> {
        if let Some(dir) = std::path::Path::new(path).parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}
