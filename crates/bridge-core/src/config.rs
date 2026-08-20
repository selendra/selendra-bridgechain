//! Small config-validation helpers shared by every service's `Config::from_toml`.
//!
//! Each service configures a list of chain blocks and must reject duplicates in
//! it: two `[[sources]]` for one chain_id means two scan loops racing on one
//! cursor, two `[[targets]]` means two claim loops contending for one nonce.
//! Every service hand-rolled the same nested-loop scan for that, so the check
//! lives here once — and as a set lookup rather than the O(n²) pairwise walk.

use std::collections::BTreeSet;
use std::fmt::Display;

/// Fail if any two entries of `items` share a `key`.
///
/// `what` names the field for the error message ("target chain_id",
/// "state_file"), which is what an operator actually needs to see: the message
/// reads `duplicate <what> <value> in config`.
pub fn ensure_unique<'a, T: 'a, K, F>(items: &'a [T], key: F, what: &str) -> anyhow::Result<()>
where
    K: Ord + Display,
    F: Fn(&'a T) -> K,
{
    let mut seen: BTreeSet<K> = BTreeSet::new();
    for item in items {
        let k = key(item);
        if !seen.insert(k) {
            // Re-derive for the message: `insert` consumed the key.
            anyhow::bail!("duplicate {what} {} in config", key(item));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_distinct_keys() {
        ensure_unique(&[1u64, 2, 3], |n| *n, "chain_id").unwrap();
    }

    #[test]
    fn rejects_a_repeat_and_names_it() {
        let err = ensure_unique(&[1u64, 2, 1], |n| *n, "chain_id").unwrap_err().to_string();
        assert!(err.contains("duplicate chain_id 1"), "got: {err}");
    }

    #[test]
    fn an_empty_list_is_fine() {
        ensure_unique::<u64, u64, _>(&[], |n| *n, "chain_id").unwrap();
    }
}
