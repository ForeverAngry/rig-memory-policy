//! Reconciliation decisions for consolidating new memory against existing
//! entries.
//!
//! State-of-the-art memory systems (e.g. Mem0) do not blindly append every
//! candidate. Instead they *consolidate*: each incoming item is compared
//! against what is already stored and resolved to one of a small set of
//! operations — add it as new, update an existing entry, delete a now-stale
//! entry, or do nothing. This module provides the neutral **decision type**
//! ([`ReconcileOp`]) and a **trait seam** ([`DuplicateResolver`]) for that
//! comparison, plus an exact-content-hash resolver
//! ([`ExactHashResolver`]) that needs no embedder or model.
//!
//! Backends with richer similarity signals (embeddings, an LLM judge, a
//! knowledge graph) implement [`DuplicateResolver`] themselves; this crate
//! stays free of any model or runtime dependency.
//!
//! # Example
//!
//! ```
//! use rig_memory_policy::reconcile::{DuplicateResolver, ExactHashResolver, ReconcileOp};
//!
//! let resolver = ExactHashResolver::new(["the sky is blue"]);
//! assert_eq!(resolver.reconcile("the sky is blue"), ReconcileOp::Noop);
//! assert_eq!(resolver.reconcile("the grass is green"), ReconcileOp::Add);
//! ```

use std::collections::HashSet;

use crate::dedup::{DedupKey, hex_encode_key};

/// The operation a backend should apply when consolidating an incoming memory
/// item against existing storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileOp {
    /// The item is novel; store it as a new entry.
    Add,
    /// The item refines an existing entry identified by `target`; the backend
    /// should update that entry in place (or supersede it).
    Update {
        /// Identifier of the existing entry to update.
        target: String,
    },
    /// The item indicates an existing entry identified by `target` is now
    /// stale and should be removed or invalidated.
    Delete {
        /// Identifier of the existing entry to delete.
        target: String,
    },
    /// The item is a duplicate of existing memory; take no action.
    Noop,
}

/// A backend-neutral seam for deciding how an incoming item reconciles against
/// existing memory.
///
/// Implementors may use any similarity signal — exact hashing, embeddings, an
/// LLM judge, or a graph lookup. This crate ships only the exact-hash
/// implementation ([`ExactHashResolver`]); everything richer lives in the
/// adapter that owns the model or index.
pub trait DuplicateResolver {
    /// Decide the reconciliation operation for `incoming`.
    fn reconcile(&self, incoming: &str) -> ReconcileOp;
}

/// A [`DuplicateResolver`] that treats an item as a duplicate only when its
/// content hash exactly matches a previously seen item.
///
/// This is the deterministic, dependency-free floor: it catches verbatim
/// repeats (the most common source of memory bloat) and defers every other
/// case to [`ReconcileOp::Add`]. Backends that need paraphrase-level matching
/// layer their own resolver on top.
#[derive(Debug, Default, Clone)]
pub struct ExactHashResolver {
    seen: HashSet<String>,
}

impl ExactHashResolver {
    /// Construct a resolver seeded with the content hashes of `existing`.
    #[must_use]
    pub fn new<I, S>(existing: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let seen = existing
            .into_iter()
            .map(|text| content_hash(text.as_ref()))
            .collect();
        Self { seen }
    }

    /// Record `text` as seen so future identical items reconcile to
    /// [`ReconcileOp::Noop`].
    pub fn observe(&mut self, text: &str) {
        self.seen.insert(content_hash(text));
    }

    /// Number of distinct content hashes currently tracked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    /// Whether the resolver has observed any content yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

impl DuplicateResolver for ExactHashResolver {
    fn reconcile(&self, incoming: &str) -> ReconcileOp {
        if self.seen.contains(&content_hash(incoming)) {
            ReconcileOp::Noop
        } else {
            ReconcileOp::Add
        }
    }
}

/// Hex-encoded BLAKE3 hash of a single text payload, used as the exact-match
/// key for reconciliation.
fn content_hash(text: &str) -> String {
    let key: DedupKey = *blake3::hash(text.as_bytes()).as_bytes();
    hex_encode_key(&key)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn exact_duplicate_is_noop() {
        let resolver = ExactHashResolver::new(["hello world"]);
        assert_eq!(resolver.reconcile("hello world"), ReconcileOp::Noop);
    }

    #[test]
    fn novel_item_is_add() {
        let resolver = ExactHashResolver::new(["hello world"]);
        assert_eq!(resolver.reconcile("something else"), ReconcileOp::Add);
    }

    #[test]
    fn whitespace_difference_is_not_an_exact_match() {
        let resolver = ExactHashResolver::new(["hello world"]);
        // Exact hashing intentionally treats this as novel; near-duplicate
        // detection is a separate, opt-in concern (see `dedup::simhash`).
        assert_eq!(resolver.reconcile("hello  world"), ReconcileOp::Add);
    }

    #[test]
    fn observe_makes_future_items_noop() {
        let mut resolver = ExactHashResolver::default();
        assert!(resolver.is_empty());
        assert_eq!(resolver.reconcile("note"), ReconcileOp::Add);
        resolver.observe("note");
        assert_eq!(resolver.len(), 1);
        assert_eq!(resolver.reconcile("note"), ReconcileOp::Noop);
    }
}
