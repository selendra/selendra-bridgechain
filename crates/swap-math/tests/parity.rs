//! The Solana pool's pricing must equal `SwapPool.sol`'s, to the unit.
//!
//! The fixtures are produced by the Solidity contract itself
//! (`contracts/test/GenSwapMathFixtures.t.sol` -> `contracts/fixtures/swap_math.json`),
//! so this is a cross-VM equivalence check, not a restatement of the formula.
//! Regenerate with:
//!
//!     cd contracts && forge test --match-contract GenSwapMathFixtures
//!
//! A failure here means a quote shown by the UI (EVM math) and a swap executed
//! on Solana would disagree — the swap-side twin of the sacred submissionId.

use std::path::PathBuf;

use swap_math::amount_out;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/fixtures/swap_math.json")
}

#[test]
fn solana_pricing_matches_the_solidity_pool() {
    let raw = std::fs::read_to_string(fixture_path()).expect(
        "contracts/fixtures/swap_math.json missing — regenerate it with \
         `cd contracts && forge test --match-contract GenSwapMathFixtures`",
    );
    let doc: serde_json::Value = serde_json::from_str(&raw).expect("fixture is valid json");
    let cases = doc["fixtures"].as_array().expect("fixtures array");
    assert!(!cases.is_empty(), "fixture file has no cases");

    for c in cases {
        let name = c["name"].as_str().unwrap();
        let u = |k: &str| -> u128 { c[k].as_str().unwrap().parse().unwrap() };
        let n = |k: &str| -> u64 { c[k].as_u64().unwrap() };

        let got = amount_out(
            u("amountIn") as u64,
            u("priceIn"),
            n("decIn") as u8,
            u("priceOut"),
            n("decOut") as u8,
            n("feeBps") as u16,
        );
        assert_eq!(
            got,
            Some(u("amountOut") as u64),
            "case {name}: Solana pricing disagrees with SwapPool.sol"
        );
    }
    println!("{} pricing fixtures agree across VMs", cases.len());
}

/// The client-side PDA derivation must agree with the on-chain one, or an
/// off-chain reader looks at the wrong accounts and reports an empty pool.
///
/// The expected values are the ones `swap-admin` (which derives them through
/// `solana-program`) printed for the deployed program.
#[cfg(feature = "pda")]
#[test]
fn pda_derivation_matches_solana_program() {
    use swap_math::pda;

    let program = bs58_decode("E28r29Hyky3UqVBcdSvFk6qNedbRN8X2z4R8hYGDUk88");
    assert_eq!(
        bs58_encode(&pda::pool_address(&program).unwrap()),
        "2P4qWVSyeyg4k2gZ5PBNfNgGc3D374DxWhuYtKgfLwWE",
        "pool PDA"
    );
    assert_eq!(
        bs58_encode(&pda::vault_authority(&program).unwrap()),
        "DN74qCVsohNY15Sc2Dm1Kt9U36KPWfnMbUKvVv4St5BS",
        "vault authority PDA"
    );
}

#[cfg(feature = "pda")]
fn bs58_decode(s: &str) -> [u8; 32] {
    // Minimal base58 decode, so the test needs no extra dependency.
    let alphabet = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut out = vec![0u8; 32];
    for c in s.bytes() {
        let mut carry = alphabet.iter().position(|&a| a == c).expect("base58 char");
        for b in out.iter_mut().rev() {
            carry += 58 * (*b as usize);
            *b = (carry & 0xff) as u8;
            carry >>= 8;
        }
        assert_eq!(carry, 0, "base58 input overflows 32 bytes");
    }
    out.try_into().unwrap()
}

#[cfg(feature = "pda")]
fn bs58_encode(key: &[u8; 32]) -> String {
    let alphabet = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut digits: Vec<u8> = vec![0];
    for &byte in key.iter() {
        let mut carry = byte as usize;
        for d in digits.iter_mut() {
            carry += (*d as usize) << 8;
            *d = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }
    let leading = key.iter().take_while(|&&b| b == 0).count();
    let mut s = String::new();
    for _ in 0..leading {
        s.push('1');
    }
    for d in digits.iter().rev() {
        s.push(alphabet[*d as usize] as char);
    }
    s
}
