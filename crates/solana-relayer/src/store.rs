//! The sig-store wire contract.
//!
//! This duplicates `bridge_core::store::SubmissionRecord` on purpose. That crate
//! depends unconditionally on `alloy-primitives` (it backs the core hashing), and
//! alloy's `zeroize ^1.5` cannot coexist with `solana-client`'s `<1.4` pin — so
//! this process cannot link it at all. What crosses the boundary here is a JSON
//! wire format, not shared logic, and the server re-derives and re-verifies
//! everything it accepts. The real protocol logic still comes from
//! `bridge-solana`, which IS shared.
//!
//! Field names must stay byte-identical to `SubmissionRecord`'s serde names.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignerSig {
    pub signer: String,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubmissionRecord {
    pub submission_id: String,
    /// `0x`-prefixed 32-byte deployment domain, read from the gate's `["config"]`
    /// PDA. NOT optional and NOT defaultable here: the store parses it strictly
    /// and rejects the whole submission if it is empty, which is the point — a
    /// record with no domain belongs to no generation and must never be signed
    /// for. Omitting it from this struct is what silently killed the entire
    /// Solana -> EVM direction, since the store's own field IS `#[serde(default)]`
    /// and so accepted the missing key, then failed on the empty value.
    pub bridge_domain: String,
    pub debridge_id: String,
    /// decimal string (uint256 on the EVM side)
    pub amount: String,
    pub chain_id_from: u64,
    pub chain_id_to: u64,
    pub nonce: u64,
    pub receiver: String,
    pub auto_params: String,
    pub native_sender: String,
    #[serde(default)]
    pub token: String,
    pub signatures: Vec<SignerSig>,
    #[serde(default)]
    pub cancel_signatures: Vec<SignerSig>,
    #[serde(default)]
    pub refund_signatures: Vec<SignerSig>,
}

/// Minimal HTTP client for the sig-store, carrying the validator-scoped bearer
/// token (finding L-5: this process signs, so `Sign` is the only scope it needs).
pub struct Store {
    base: String,
    client: reqwest::Client,
}

impl Store {
    pub fn new(base: &str, token: Option<String>) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(token) = token.as_deref().filter(|t| !t.is_empty()) {
            if let Ok(mut v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")) {
                v.set_sensitive(true);
                headers.insert(reqwest::header::AUTHORIZATION, v);
            }
        }
        Store {
            base: base.trim_end_matches('/').to_string(),
            client: reqwest::Client::builder().default_headers(headers).build().unwrap_or_default(),
        }
    }

    /// POST a record plus this validator's signature. The server enforces the
    /// id⇄params binding and signature authenticity, so a bug here cannot poison
    /// the store — it can only get us rejected.
    pub async fn upsert(&self, record: &SubmissionRecord) -> anyhow::Result<()> {
        let res =
            self.client.post(format!("{}/submissions", self.base)).json(record).send().await?;
        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            anyhow::bail!("sig-store rejected the submission ({status}): {body}");
        }
        Ok(())
    }
}

impl Store {
    /// Submissions the store has flagged as stuck. Candidates only — the caller
    /// MUST still verify both ends on-chain before signing anything, which is
    /// exactly what `refund::Attester` does.
    pub async fn refund_candidates(&self) -> anyhow::Result<Vec<SubmissionRecord>> {
        let res = self.client.get(format!("{}/refund-candidates", self.base)).send().await?;
        if !res.status().is_success() {
            anyhow::bail!("sig-store refund-candidates failed ({})", res.status());
        }
        Ok(res.json().await?)
    }

    /// Deposit one cancel/refund attestation. The server re-derives the digest
    /// for `kind` specifically, so a transfer signature posted as a cancel
    /// recovers to the wrong address and is refused — the three quorums stay
    /// independent.
    pub async fn post_attestation(
        &self,
        submission_id: &str,
        kind: &str,
        signer: &str,
        signature: &str,
    ) -> anyhow::Result<()> {
        let body = serde_json::json!({ "kind": kind, "signer": signer, "signature": signature });
        let res = self
            .client
            .post(format!("{}/submissions/{submission_id}/attestations", self.base))
            .json(&body)
            .send()
            .await?;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            anyhow::bail!("sig-store rejected the {kind} attestation ({status}): {text}");
        }
        Ok(())
    }

    /// Every record the store holds. The keeper half filters these down to the
    /// ones bound for Solana.
    pub async fn list(&self) -> anyhow::Result<Vec<SubmissionRecord>> {
        let res = self.client.get(format!("{}/submissions", self.base)).send().await?;
        if !res.status().is_success() {
            anyhow::bail!("sig-store list failed ({})", res.status());
        }
        Ok(res.json().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_record() -> SubmissionRecord {
        SubmissionRecord {
            submission_id: format!("0x{}", "11".repeat(32)),
            bridge_domain: format!("0x{}", "22".repeat(32)),
            debridge_id: format!("0x{}", "33".repeat(32)),
            amount: "1000000".into(),
            chain_id_from: 7_565_164,
            chain_id_to: 11_155_111,
            nonce: 0,
            receiver: format!("0x{}", "44".repeat(20)),
            auto_params: "0x".into(),
            native_sender: format!("0x{}", "55".repeat(32)),
            token: String::new(),
            signatures: vec![],
            cancel_signatures: vec![],
            refund_signatures: vec![],
        }
    }

    /// THE REGRESSION. This struct is a hand-maintained duplicate of
    /// `bridge_core::store::SubmissionRecord` (the crates cannot link: alloy needs
    /// `zeroize ^1.5`, solana-client pins `<1.4`). When `bridge_domain` was added
    /// to the canonical record it was NOT added here, and because the canonical
    /// field is `#[serde(default)]` the store happily accepted the missing key —
    /// then rejected the empty value with `malformed field: bridge_domain`, and
    /// every Solana-origin transfer failed to store. Nothing in either crate's
    /// tests noticed, because neither can see the other's struct.
    ///
    /// So the wire format is pinned HERE, as an explicit key list.
    #[test]
    fn the_wire_format_carries_every_field_the_store_requires() {
        let json = serde_json::to_value(a_record()).expect("serializes");
        let obj = json.as_object().expect("an object");

        // Exactly the serde names `bridge_core::store::SubmissionRecord` reads.
        for key in [
            "submission_id",
            "bridge_domain",
            "debridge_id",
            "amount",
            "chain_id_from",
            "chain_id_to",
            "nonce",
            "receiver",
            "auto_params",
            "native_sender",
            "token",
            "signatures",
            "cancel_signatures",
            "refund_signatures",
        ] {
            assert!(obj.contains_key(key), "the store requires `{key}`, and it is not on the wire");
        }
    }

    /// The store parses `bridge_domain` with `B256::from_str`, which accepts only
    /// `0x` + 64 hex. An empty string — the value a missing field defaults to —
    /// is the exact failure that took the Solana leg down, so pin the shape.
    #[test]
    fn bridge_domain_is_sent_as_an_0x_prefixed_32_byte_hex_string() {
        let json = serde_json::to_value(a_record()).expect("serializes");
        let d = json["bridge_domain"].as_str().expect("a string");
        assert!(d.starts_with("0x"), "must be 0x-prefixed, got {d}");
        assert_eq!(d.len(), 66, "must be 0x + 64 hex chars, got {} in {d}", d.len());
        assert!(d[2..].chars().all(|c| c.is_ascii_hexdigit()), "must be hex: {d}");
        assert_ne!(d, "0x", "an empty domain is rejected by the store, not defaulted");
    }
}
