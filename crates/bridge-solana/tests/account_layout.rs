//! The host-side account mirrors, checked against the DEPLOYED gate.
//!
//! A layout mirror that is only tested against itself proves nothing: it will
//! happily round-trip a definition that has drifted from the program's. So this
//! decodes a real account captured from the devnet gate and asserts the values
//! match what that gate is known to hold.

use bridge_solana::account::{decode, ConfigAccount};

#[test]
fn the_config_mirror_decodes_the_live_devnet_account() {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/devnet_gate_config.json"
    ))
    .expect("fixture");
    let fx: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        fx["base64"].as_str().unwrap(),
    )
    .expect("base64");

    let cfg: ConfigAccount = decode(&bytes).expect("the live account must decode");
    let want = &fx["expect"];

    assert_eq!(
        format!("0x{}", hex::encode(cfg.bridge_domain)),
        want["bridge_domain"].as_str().unwrap(),
        "bridge domain"
    );
    assert_eq!(cfg.threshold as u64, want["threshold"].as_u64().unwrap());
    assert_eq!(cfg.chain_id, want["chain_id"].as_u64().unwrap());
    assert_eq!(cfg.validators.len() as u64, want["validators"].as_u64().unwrap());
    assert_eq!(cfg.paused, want["paused"].as_bool().unwrap());

    let corridors: Vec<u64> = want["corridors"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    for c in &corridors {
        assert!(cfg.nonce(*c).is_some(), "corridor {c} should be registered");
    }
    // And a destination nobody registered has no nonce — the same answer `send`
    // gives before it refuses.
    assert_eq!(cfg.nonce(424242), None);
}
