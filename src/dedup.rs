//! In-process content-hash dedup for memory-store hooks and compactors.
//!
//! Rig's `DemotionHook` / `Compactor` traits require implementations to be
//! idempotent on `(conversation_id, messages)`. Append-only backends (such as
//! `.mv2` archives) have no unique-key enforcement, so this module provides a
//! small content-hash gate that lives alongside the hook/compactor instance
//! and prevents the same entry from being appended twice within a single
//! process lifetime.
//!
//! # Scope of the guarantee
//!
//! - **Within a process:** invoking the same operation more than once with the
//!   same `(kind, conversation_id, role, scope, text)` produces exactly one
//!   entry in the backing store.
//! - **Across process restarts:** dedup state is not persisted by default.
//!   Callers that need cross-restart idempotency can snapshot the set via
//!   [`DedupSet::snapshot`] before shutdown and replay it into a fresh
//!   instance via [`DedupSet::extend_from_snapshot`].
//!
//! # Example
//!
//! ```
//! use rig_memory_policy::dedup::{DedupSet, compute_key};
//!
//! let set = DedupSet::new();
//! let key = compute_key("demoted_message", "conv-1", "user", None, "hello");
//! assert!(!set.contains(&key).unwrap());
//! set.insert(key).unwrap();
//! assert!(set.contains(&key).unwrap());
//! ```

use std::collections::HashSet;
use std::sync::RwLock;

use crate::error::PolicyError;

/// 32-byte content fingerprint produced by [`blake3::hash`].
pub type DedupKey = [u8; 32];

/// Compute the dedup key for a single entry.
///
/// Inputs are joined by NUL so that two distinct field tuples cannot collide
/// via concatenation (e.g. `("ab", "c")` vs `("a", "bc")`).
pub fn compute_key(
    kind: &str,
    conversation_id: &str,
    role: &str,
    scope: Option<&str>,
    text: &str,
) -> DedupKey {
    let mut hasher = blake3::Hasher::new();
    hasher.update(kind.as_bytes());
    hasher.update(&[0]);
    hasher.update(conversation_id.as_bytes());
    hasher.update(&[0]);
    hasher.update(role.as_bytes());
    hasher.update(&[0]);
    hasher.update(scope.unwrap_or("").as_bytes());
    hasher.update(&[0]);
    hasher.update(text.as_bytes());
    *hasher.finalize().as_bytes()
}

/// In-memory set of dedup keys with snapshot / restore for opt-in
/// cross-process persistence.
#[derive(Default)]
pub struct DedupSet {
    seen: RwLock<HashSet<DedupKey>>,
}

impl DedupSet {
    /// Construct an empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if `key` has already been recorded.
    ///
    /// Returns [`PolicyError::Poisoned`] if a previous holder of the internal
    /// lock panicked.
    pub fn contains(&self, key: &DedupKey) -> Result<bool, PolicyError> {
        let guard = self.seen.read().map_err(|_| PolicyError::Poisoned)?;
        Ok(guard.contains(key))
    }

    /// Record `key` as seen. Idempotent.
    pub fn insert(&self, key: DedupKey) -> Result<(), PolicyError> {
        let mut guard = self.seen.write().map_err(|_| PolicyError::Poisoned)?;
        guard.insert(key);
        Ok(())
    }

    /// Snapshot the current set as a sorted list of hex-encoded keys, suitable
    /// for writing to a sidecar file.
    pub fn snapshot(&self) -> Result<Vec<String>, PolicyError> {
        let guard = self.seen.read().map_err(|_| PolicyError::Poisoned)?;
        let mut out: Vec<String> = guard.iter().map(hex_encode).collect();
        out.sort();
        Ok(out)
    }

    /// Replay a snapshot produced by [`Self::snapshot`] back into this set.
    /// Malformed entries are skipped with a `tracing::warn!`.
    pub fn extend_from_snapshot(&self, hexes: &[String]) -> Result<(), PolicyError> {
        let mut guard = self.seen.write().map_err(|_| PolicyError::Poisoned)?;
        for hex in hexes {
            match hex_decode(hex) {
                Some(key) => {
                    guard.insert(key);
                }
                None => {
                    tracing::warn!(
                        target: "rig_memory_policy::dedup",
                        invalid = %hex,
                        "skipping malformed dedup snapshot entry",
                    );
                }
            }
        }
        Ok(())
    }

    /// Number of recorded keys. Test/diagnostic surface.
    #[cfg(test)]
    pub(crate) fn len(&self) -> Result<usize, PolicyError> {
        let guard = self.seen.read().map_err(|_| PolicyError::Poisoned)?;
        Ok(guard.len())
    }
}

impl std::fmt::Debug for DedupSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.seen.read().map(|g| g.len()).unwrap_or_default();
        f.debug_struct("DedupSet").field("entries", &count).finish()
    }
}

fn hex_encode(key: &DedupKey) -> String {
    let mut out = String::with_capacity(64);
    for b in key {
        out.push(nibble_to_hex(b >> 4));
        out.push(nibble_to_hex(b & 0x0f));
    }
    out
}

/// Hex-encode a [`DedupKey`] as 64 lowercase ASCII chars. Callers stamp this
/// into per-entry metadata so the dedup decision survives in the archive.
pub fn hex_encode_key(key: &DedupKey) -> String {
    hex_encode(key)
}

fn nibble_to_hex(n: u8) -> char {
    // `n` is masked to 0..=15 before reaching this function (or is already a
    // high-nibble shifted into 0..=15). The branch keeps
    // clippy::indexing_slicing happy without sacrificing readability.
    let n = n & 0x0f;
    if n < 10 {
        (b'0' + n) as char
    } else {
        (b'a' + n - 10) as char
    }
}

fn hex_decode(hex: &str) -> Option<DedupKey> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    let bytes = hex.as_bytes();
    for i in 0..32 {
        let hi = nibble(bytes.get(i * 2).copied()?)?;
        let lo = nibble(bytes.get(i * 2 + 1).copied()?)?;
        if let Some(slot) = out.get_mut(i) {
            *slot = (hi << 4) | lo;
        }
    }
    Some(out)
}

fn nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Near-duplicate detection via SimHash.
///
/// Exact content hashing (the rest of this module) only catches verbatim
/// repeats. Agent memory frequently accumulates *paraphrases* — the same fact
/// restated with different whitespace, punctuation, or word order. [`SimHash`]
/// maps a text to a 64-bit fingerprint whose Hamming distance approximates
/// token-set dissimilarity, so backends can gate near-duplicates without an
/// embedder or model.
///
/// This is opt-in via the `near-dedup` feature to honour the crate's
/// dependency-light, deterministic stance; the algorithm itself adds no
/// dependencies and reuses [`blake3`] for per-token hashing.
///
/// # Example
///
/// ```
/// # #[cfg(feature = "near-dedup")] {
/// use rig_memory_policy::dedup::SimHash;
///
/// let a = SimHash::from_text("the scheduled maintenance window is tonight");
/// let b = SimHash::from_text("the maintenance window is scheduled tonight");
/// // Reordered words stay within a small Hamming distance.
/// assert!(a.hamming_distance(&b) <= 8);
/// # }
/// ```
#[cfg(feature = "near-dedup")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SimHash(u64);

#[cfg(feature = "near-dedup")]
impl SimHash {
    /// Compute the SimHash fingerprint of `text`.
    ///
    /// Tokens are whitespace-delimited, lowercased, and trimmed of leading and
    /// trailing non-alphanumeric characters (matching the normalization used
    /// by the in-memory lexical store). An empty token set yields a zero
    /// fingerprint.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        let mut columns = [0i32; 64];
        let mut any = false;
        for token in text.split_whitespace() {
            let token = token.trim_matches(|ch: char| !ch.is_alphanumeric());
            if token.is_empty() {
                continue;
            }
            any = true;
            let lowered = token.to_lowercase();
            let hash = u64::from_le_bytes(token_hash_8(lowered.as_bytes()));
            for (bit, column) in columns.iter_mut().enumerate() {
                if (hash >> bit) & 1 == 1 {
                    *column += 1;
                } else {
                    *column -= 1;
                }
            }
        }
        if !any {
            return Self(0);
        }
        let mut fingerprint = 0u64;
        for (bit, column) in columns.iter().enumerate() {
            if *column > 0 {
                fingerprint |= 1u64 << bit;
            }
        }
        Self(fingerprint)
    }

    /// Return the raw 64-bit fingerprint.
    #[must_use]
    pub fn bits(&self) -> u64 {
        self.0
    }

    /// Hamming distance between two fingerprints. Smaller means more similar;
    /// `0` means the token multisets produced identical fingerprints.
    #[must_use]
    pub fn hamming_distance(&self, other: &Self) -> u32 {
        (self.0 ^ other.0).count_ones()
    }

    /// Whether `other` is within `max_distance` bits of `self`.
    #[must_use]
    pub fn is_near(&self, other: &Self, max_distance: u32) -> bool {
        self.hamming_distance(other) <= max_distance
    }
}

/// First 8 bytes of the BLAKE3 hash of `bytes`, used as a per-token hash for
/// SimHash column accumulation.
#[cfg(feature = "near-dedup")]
fn token_hash_8(bytes: &[u8]) -> [u8; 8] {
    let full = blake3::hash(bytes);
    let mut out = [0u8; 8];
    for (slot, byte) in out.iter_mut().zip(full.as_bytes().iter()) {
        *slot = *byte;
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn distinct_inputs_produce_distinct_keys() {
        let a = compute_key("demoted_message", "c1", "user", None, "hello");
        let b = compute_key("demoted_message", "c1", "user", None, "hello world");
        let c = compute_key("compaction_summary", "c1", "user", None, "hello");
        let d = compute_key("demoted_message", "c2", "user", None, "hello");
        let e = compute_key("demoted_message", "c1", "assistant", None, "hello");
        let f = compute_key("demoted_message", "c1", "user", Some("s"), "hello");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
        assert_ne!(a, e);
        assert_ne!(a, f);
    }

    #[test]
    fn identical_inputs_produce_identical_keys() {
        let a = compute_key("demoted_message", "c1", "user", None, "hello");
        let b = compute_key("demoted_message", "c1", "user", None, "hello");
        assert_eq!(a, b);
    }

    #[test]
    fn boundary_collision_resistance() {
        // ("ab","c") vs ("a","bc") would collide under naive concat.
        let a = compute_key("ab", "c", "user", None, "");
        let b = compute_key("a", "bc", "user", None, "");
        assert_ne!(a, b);
    }

    #[test]
    fn set_round_trips_via_snapshot() {
        let set = DedupSet::new();
        let k1 = compute_key("kind", "conv", "user", None, "one");
        let k2 = compute_key("kind", "conv", "user", None, "two");
        set.insert(k1).unwrap();
        set.insert(k2).unwrap();
        let snap = set.snapshot().unwrap();
        assert_eq!(snap.len(), 2);

        let restored = DedupSet::new();
        restored.extend_from_snapshot(&snap).unwrap();
        assert!(restored.contains(&k1).unwrap());
        assert!(restored.contains(&k2).unwrap());
    }

    #[test]
    fn malformed_snapshot_entries_are_skipped() {
        let set = DedupSet::new();
        let good = compute_key("k", "c", "user", None, "x");
        let bad = "not-hex".to_string();
        let snap = vec![hex_encode(&good), bad];
        set.extend_from_snapshot(&snap).unwrap();
        assert_eq!(set.len().unwrap(), 1);
        assert!(set.contains(&good).unwrap());
    }

    #[cfg(feature = "near-dedup")]
    #[test]
    fn simhash_identical_text_has_zero_distance() {
        let a = SimHash::from_text("scheduled maintenance window tonight");
        let b = SimHash::from_text("scheduled maintenance window tonight");
        assert_eq!(a.hamming_distance(&b), 0);
    }

    #[cfg(feature = "near-dedup")]
    #[test]
    fn simhash_word_reorder_stays_close() {
        let a = SimHash::from_text("the scheduled maintenance window is tonight");
        let b = SimHash::from_text("the maintenance window is scheduled tonight");
        // Pure reorder: identical token multiset -> identical fingerprint.
        assert_eq!(a.hamming_distance(&b), 0);
        assert!(a.is_near(&b, 4));
    }

    #[cfg(feature = "near-dedup")]
    #[test]
    fn simhash_normalizes_case_and_punctuation() {
        let a = SimHash::from_text("PowerShell, scheduled-task!");
        let b = SimHash::from_text("powershell scheduled-task");
        assert!(a.is_near(&b, 4));
    }

    #[cfg(feature = "near-dedup")]
    #[test]
    fn simhash_distinct_topics_are_far_apart() {
        let a = SimHash::from_text("scheduled maintenance window tonight");
        let b = SimHash::from_text("quarterly revenue forecast spreadsheet");
        assert!(a.hamming_distance(&b) > 8);
    }

    #[cfg(feature = "near-dedup")]
    #[test]
    fn simhash_empty_text_is_zero() {
        assert_eq!(SimHash::from_text("   ").bits(), 0);
    }

    #[test]
    fn hex_round_trips_for_many_keys() {
        for i in 0..512u32 {
            let key = compute_key("kind", "conv", "role", None, &i.to_string());
            let encoded = hex_encode(&key);
            assert_eq!(encoded.len(), 64);
            let decoded = hex_decode(&encoded).unwrap();
            assert_eq!(decoded, key);
        }
    }

    #[test]
    fn hex_decode_rejects_malformed_inputs() {
        // Wrong length.
        assert!(hex_decode("").is_none());
        assert!(hex_decode("ab").is_none());
        assert!(hex_decode(&"a".repeat(63)).is_none());
        assert!(hex_decode(&"a".repeat(65)).is_none());
        // Correct length, non-hex characters.
        assert!(hex_decode(&"g".repeat(64)).is_none());
        assert!(hex_decode(&"z".repeat(64)).is_none());
        // Correct length, embedded non-hex char.
        let mut bad = "a".repeat(63);
        bad.push('!');
        assert!(hex_decode(&bad).is_none());
    }

    #[test]
    fn hex_decode_accepts_mixed_case() {
        let key = compute_key("k", "c", "r", None, "payload");
        let lower = hex_encode(&key);
        let upper = lower.to_uppercase();
        assert_eq!(hex_decode(&lower), hex_decode(&upper));
        assert_eq!(hex_decode(&upper), Some(key));
    }
}
