//! Shared test helpers: real alloy secp256k1 validators that sign exactly the
//! way the production validator node does (EIP-191 eth_sign, r||s||v, v∈{27,28}).
#![allow(dead_code)]

use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;

/// A validator: an alloy signer plus its 20-byte EVM address.
pub struct Validator {
    pub signer: PrivateKeySigner,
}

impl Validator {
    pub fn random() -> Self {
        Self { signer: PrivateKeySigner::random() }
    }

    /// Deterministic validator from a 32-byte seed (stable addresses across runs).
    pub fn from_seed(seed: u8) -> Self {
        let mut key = [0u8; 32];
        key[31] = seed;
        // never zero (invalid key); seed starts at 1 in callers
        let signer = PrivateKeySigner::from_bytes(&key.into()).expect("valid key");
        Self { signer }
    }

    pub fn address(&self) -> [u8; 20] {
        self.signer.address().into_array()
    }

    /// EIP-191 `eth_sign` over the raw 32-byte id, encoded as 65 bytes r||s||v
    /// with v∈{27,28} — byte-identical to the validator node's `encode_signature`.
    pub async fn sign(&self, id: &[u8; 32]) -> Vec<u8> {
        let sig = self.signer.sign_message(id).await.expect("sign");
        bridge_core::signer::signature_bytes(&sig).to_vec()
    }
}

/// Sort validators by address so `from_seed` sets can be indexed deterministically
/// if a test needs a known signer order.
pub fn sorted(mut vs: Vec<Validator>) -> Vec<Validator> {
    vs.sort_by_key(|v| v.address());
    vs
}
