//! The swap pool's pricing math and account layouts — the ONE definition of
//! each, shared by the on-chain program and every off-chain reader.
//!
//! ## Why the layouts live here too
//!
//! `graphql-api` has to decode pool accounts to quote a swap, and it cannot
//! link the program crate: `solana-program` pins `zeroize <1.4` and alloy needs
//! `^1.5`. The tempting shortcut is to re-declare the structs on the API side.
//! That is precisely how the gate's two `Sent` definitions drifted apart — both
//! compiled, and the mismatch only showed up as silence at runtime. So the
//! structs are declared once, here, with plain `[u8; 32]` keys rather than a
//! `Pubkey`, which is the only thing that kept them from being shareable.
//!
//! ## The pricing
//!
//! Every operation mirrors `SwapPool.sol` exactly, INCLUDING its rounding:
//! a quote the UI shows and a swap the program executes must agree to the last
//! unit across both VMs, the same way `BridgeHash` keeps the submissionId
//! identical. `tests/parity.rs` checks this against fixtures produced by the
//! Solidity contract itself.

use uint::construct_uint;

construct_uint! {
    /// 256-bit intermediate, because the Solidity pool computes in `uint256` and
    /// a `u128` product overflows at realistic inputs: a 1e19-unit amount times a
    /// 1e21-scaled price is ~1e40, and `u128::MAX` is ~3.4e38.
    pub struct U256(4);
}

/// USD prices are fixed-point, scaled by this (matches `SwapPool.PRICE_ONE`).
pub const PRICE_ONE: u128 = 1_000_000_000_000_000_000;
/// Basis-point denominator for the fee and the price-deviation guard.
pub const BPS_DENOM: u16 = 10_000;

/// `floor(a * b / d)` — OpenZeppelin `Math.mulDiv` with the default (floor)
/// rounding. `None` on a zero divisor or a result past `u128`.
pub fn mul_div_floor(a: u128, b: u128, d: u128) -> Option<u128> {
    mul_div(a, b, d, false)
}

/// `ceil(a * b / d)` — `Math.mulDiv(..., Math.Rounding.Ceil)`.
pub fn mul_div_ceil(a: u128, b: u128, d: u128) -> Option<u128> {
    mul_div(a, b, d, true)
}

fn mul_div(a: u128, b: u128, d: u128, ceil: bool) -> Option<u128> {
    if d == 0 {
        return None;
    }
    let n = U256::from(a).checked_mul(U256::from(b))?;
    let dd = U256::from(d);
    let q = n / dd;
    let q = if ceil && q * dd != n { q.checked_add(U256::one())? } else { q };
    if q > U256::from(u128::MAX) {
        return None;
    }
    Some(q.as_u128())
}

/// `10^dec`, refusing a decimals value no SPL mint or ERC-20 can have rather
/// than silently overflowing.
pub fn pow10(dec: u8) -> Option<u128> {
    if dec > 38 {
        return None;
    }
    10u128.checked_pow(dec as u32)
}

/// The swap output for `amount_in`, byte-for-byte the Solidity `_amountOut`:
///
/// ```text
///   usd = floor(amount_in * price_in / 10^dec_in)
///   usd -= ceil(usd * fee_bps / 10_000)        // fee rounds UP against the user
///   out = floor(usd * 10^dec_out / price_out)
/// ```
///
/// The intermediate `usd` is PRICE_ONE-scaled. Returns `None` on any overflow,
/// a zero price, or a result that does not fit an SPL amount — never a wrapped
/// or truncated number, because a quote that silently wraps is a quote that
/// pays out the wrong amount.
pub fn amount_out(
    amount_in: u64,
    price_in: u128,
    dec_in: u8,
    price_out: u128,
    dec_out: u8,
    fee_bps: u16,
) -> Option<u64> {
    if price_in == 0 || price_out == 0 {
        return None;
    }
    let usd = mul_div_floor(amount_in as u128, price_in, pow10(dec_in)?)?;
    let usd = if fee_bps != 0 {
        let fee = mul_div_ceil(usd, fee_bps as u128, BPS_DENOM as u128)?;
        usd.checked_sub(fee)?
    } else {
        usd
    };
    let out = mul_div_floor(usd, pow10(dec_out)?, price_out)?;
    u64::try_from(out).ok()
}

/// The USD value of a reserve, as `maxSwapOut` reports it.
pub fn usd_value(amount: u64, price: u128, dec: u8) -> Option<u128> {
    mul_div_floor(amount as u128, price, pow10(dec)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_to_token_is_the_price_ratio() {
        // 1000 TST (9dp, $1) -> WRAP (9dp, $3180): 1000/3180 = 0.314465408…
        //
        // Nine decimals, not eighteen, because an SPL amount is a u64: an
        // 18-decimal mint can only hold ~18.4 whole tokens. Bridged EVM assets
        // are given a 6- or 9-decimal mint on this side for that reason.
        let out = amount_out(1_000_000_000_000, PRICE_ONE, 9, 3180 * PRICE_ONE, 9, 0);
        assert_eq!(out, Some(314_465_408));
    }

    #[test]
    fn decimals_are_normalised_in_both_directions() {
        // 1 WRAP (9dp, $3180) -> USDC (6dp, $1) = 3180.000000
        assert_eq!(
            amount_out(1_000_000_000, 3180 * PRICE_ONE, 9, PRICE_ONE, 6, 0),
            Some(3_180_000_000)
        );
        // and back
        assert_eq!(
            amount_out(3_180_000_000, PRICE_ONE, 6, 3180 * PRICE_ONE, 9, 0),
            Some(1_000_000_000)
        );
    }

    #[test]
    fn the_fee_rounds_up_against_the_user() {
        // 1 unit of USD value at 1 bps: the fee is 0.0001 units, which rounds UP
        // to 1 and leaves 0 — the pool keeps the dust, exactly as Solidity's
        // `Math.Rounding.Ceil` does. A floor here would hand the user free value.
        let out = amount_out(1, PRICE_ONE, 0, PRICE_ONE, 0, 1);
        assert_eq!(out, Some(0));
    }

    #[test]
    fn overflow_is_refused_not_wrapped() {
        // u64::MAX units at a 1e21-scaled price overflows a u128 product; the
        // 256-bit intermediate carries it, and the result no longer fits u64.
        assert_eq!(amount_out(u64::MAX, 3180 * PRICE_ONE, 0, 1, 18, 0), None);
        assert_eq!(amount_out(1, 0, 18, PRICE_ONE, 18, 0), None, "zero price in");
        assert_eq!(amount_out(1, PRICE_ONE, 18, 0, 18, 0), None, "zero price out");
    }

    #[test]
    fn mul_div_rounding_matches_openzeppelin() {
        assert_eq!(mul_div_floor(7, 3, 2), Some(10)); // 10.5 -> 10
        assert_eq!(mul_div_ceil(7, 3, 2), Some(11)); // 10.5 -> 11
        assert_eq!(mul_div_ceil(10, 2, 5), Some(4), "exact division does not round up");
        assert_eq!(mul_div_floor(1, 1, 0), None);
    }
}

// ---------------------------------------------------------------------------
// Account layouts — Borsh, and identical on both sides of the wire.
// ---------------------------------------------------------------------------

use borsh::{BorshDeserialize, BorshSerialize};

/// A 32-byte Solana account key. Plain bytes, not `Pubkey`, so this crate stays
/// linkable from a binary that also links alloy (see the module docs).
pub type Key = [u8; 32];

/// The pool account (`["pool"]` PDA).
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolState {
    pub owner: Key,
    pub oracle: Key,
    pub guardian: Key,
    /// The unit of account; its price is pinned at [`PRICE_ONE`] forever.
    pub hub_mint: Key,
    pub fee_bps: u16,
    pub max_price_deviation_bps: u16,
    pub min_price_update_interval: i64,
    pub paused: bool,
}

/// One listed mint (`["token", mint]` PDA). `reserve` is INTERNAL accounting —
/// the swap cap reads it, never `balanceOf`, so a donation cannot raise the
/// payout ceiling.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct TokenState {
    pub mint: Key,
    pub vault: Key,
    pub decimals: u8,
    pub price: u128,
    pub reserve: u64,
    /// 0 = never repriced since listing; the first update is always allowed.
    pub last_price_update: i64,
    pub listed: bool,
}

/// Borsh-serialized size of [`PoolState`], plus slack for a later field.
pub const POOL_SPACE: usize = 32 * 4 + 2 + 2 + 8 + 1 + 32;
/// Borsh-serialized size of [`TokenState`], plus slack.
pub const TOKEN_SPACE: usize = 32 + 32 + 1 + 16 + 8 + 8 + 1 + 32;

pub const POOL_SEED: &[u8] = b"pool";
pub const TOKEN_SEED: &[u8] = b"token";
pub const VAULT_AUTHORITY_SEED: &[u8] = b"vault_authority";

/// The `Swapped` event, Borsh-framed through `sol_log_data` so an indexer can
/// decode it the way it decodes the gate's `Sent`. Versioned from the start.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct SwappedEvent {
    pub version: u8,
    pub sender: Key,
    pub mint_in: Key,
    pub mint_out: Key,
    pub amount_in: u64,
    pub amount_out: u64,
    pub to: Key,
}

/// Decode an account whose trailing bytes are rent padding.
///
/// `try_from_slice` refuses trailing data, and every account here is sized with
/// slack — so the strict form reads a perfectly good record as corrupt.
pub fn decode<T: BorshDeserialize>(data: &[u8]) -> Option<T> {
    T::deserialize(&mut &data[..]).ok()
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn accounts_fit_the_space_they_are_allocated() {
        assert!(borsh::to_vec(&PoolState::default()).unwrap().len() <= POOL_SPACE);
        assert!(borsh::to_vec(&TokenState::default()).unwrap().len() <= TOKEN_SPACE);
    }

    #[test]
    fn padding_does_not_make_a_record_unreadable() {
        let rec = TokenState { decimals: 9, price: PRICE_ONE, reserve: 42, listed: true, ..Default::default() };
        let mut bytes = borsh::to_vec(&rec).unwrap();
        bytes.resize(TOKEN_SPACE, 0);
        assert_eq!(decode::<TokenState>(&bytes), Some(rec));
    }
}

// ---------------------------------------------------------------------------
// PDA derivation (off-chain readers only)
// ---------------------------------------------------------------------------

/// Client-side `find_program_address`, behind the `pda` feature.
///
/// An off-chain reader needs the pool and token account addresses, and the
/// obvious way to get them — `getProgramAccounts` — is blocked on most hosted
/// free tiers (Alchemy answers "not available on the Free tier"). Deriving them
/// instead turns the read into one `getMultipleAccounts` call that works on any
/// endpoint, and is cheaper besides.
///
/// The program itself never uses this: on-chain derivation is a syscall.
#[cfg(feature = "pda")]
pub mod pda {
    use super::Key;
    use curve25519_dalek::edwards::CompressedEdwardsY;
    use sha2::{Digest, Sha256};

    const PDA_MARKER: &[u8] = b"ProgramDerivedAddress";

    /// True when the bytes are a valid Ed25519 point — i.e. a real keypair could
    /// exist for them, which is exactly what a PDA must NOT be.
    fn is_on_curve(bytes: &Key) -> bool {
        CompressedEdwardsY(*bytes).decompress().is_some()
    }

    /// The address for `seeds` under `program_id`, plus the bump that produced
    /// it. Walks the bump from 255 down, as Solana does, so it returns the
    /// canonical address every on-chain derivation will agree with.
    pub fn find_program_address(seeds: &[&[u8]], program_id: &Key) -> Option<(Key, u8)> {
        for bump in (0u8..=255).rev() {
            let mut h = Sha256::new();
            for s in seeds {
                h.update(s);
            }
            h.update([bump]);
            h.update(program_id);
            h.update(PDA_MARKER);
            let candidate: Key = h.finalize().into();
            if !is_on_curve(&candidate) {
                return Some((candidate, bump));
            }
        }
        None
    }

    /// The pool account for a program.
    pub fn pool_address(program_id: &Key) -> Option<Key> {
        find_program_address(&[super::POOL_SEED], program_id).map(|(k, _)| k)
    }

    /// The record for one listed mint.
    pub fn token_address(program_id: &Key, mint: &Key) -> Option<Key> {
        find_program_address(&[super::TOKEN_SEED, mint], program_id).map(|(k, _)| k)
    }

    /// The authority that owns every vault.
    pub fn vault_authority(program_id: &Key) -> Option<Key> {
        find_program_address(&[super::VAULT_AUTHORITY_SEED], program_id).map(|(k, _)| k)
    }
}
