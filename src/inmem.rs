//! In-memory append-only episode store with lexical retrieval.
//!
//! [`InMemoryStore`] is a no-disk reference memory backend: it keeps
//! episodes in a process-local `Vec` and ranks them with a deterministic
//! token-overlap score. It is intended for tests, examples, offline modes,
//! and adapters that need an [`Episode`]-shaped retrieval surface without
//! standing up a persistent store.
//!
//! Compared to durable backends, this store
//!
//! - has no I/O, no file lock, and no embedder;
//! - returns at most `k` hits sorted by descending lexical score; and
//! - is generic over a user-defined [`Episode`] payload, so callers keep
//!   their own domain types without forcing them through a backend-specific
//!   serialization format.
//!
//! ```no_run
//! use rig_memory_policy::inmem::{Episode, InMemoryStore};
//!
//! #[derive(Clone)]
//! struct MyEpisode {
//!     summary: String,
//! }
//!
//! impl Episode for MyEpisode {
//!     fn summary(&self) -> &str { &self.summary }
//! }
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let store = InMemoryStore::<MyEpisode>::new();
//! let key = store
//!     .append(MyEpisode { summary: "scheduled maintenance".into() })
//!     .await?;
//! let hits = store.retrieve_similar("maintenance", 5).await?;
//! assert_eq!(hits.first().map(|h| h.episode.summary.as_str()),
//!            Some("scheduled maintenance"));
//! let _ = (key, hits);
//! # Ok(()) }
//! ```

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::PolicyError;

/// User-defined episode payload with a searchable natural-language
/// summary.
///
/// Implementors keep their own domain shape; only the summary string is
/// observed by the lexical scorer.
pub trait Episode: Clone + Send + Sync + 'static {
    /// The natural-language summary used to rank hits against a query.
    /// Tokens are split on whitespace and scored by the fraction of
    /// distinct normalized query tokens present in the summary.
    fn summary(&self) -> &str;
}

/// A retrieval hit returned by [`InMemoryStore::retrieve_similar`].
#[derive(Debug, Clone)]
pub struct InMemoryHit<E> {
    /// The stored episode.
    pub episode: E,
    /// Lexical similarity score in `[0, 1]`. Higher is more similar.
    pub score: f32,
    /// Stable storage key assigned by [`InMemoryStore::append`].
    pub key: String,
}

/// Append-only in-memory episode store with deterministic lexical
/// retrieval.
///
/// `E` is the caller's episode payload; only [`Episode::summary`] is
/// inspected during retrieval.
#[derive(Debug)]
pub struct InMemoryStore<E: Episode> {
    inner: Mutex<Inner<E>>,
}

#[derive(Debug)]
struct Inner<E: Episode> {
    next_key: u64,
    episodes: Vec<(String, E)>,
}

impl<E: Episode> Default for InMemoryStore<E> {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                next_key: 0,
                episodes: Vec::new(),
            }),
        }
    }
}

impl<E: Episode> InMemoryStore<E> {
    /// Create a fresh store wrapped in [`Arc`] for cheap cloning across
    /// agent tasks.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Append a new episode and return its storage key.
    ///
    /// Keys are stable across the lifetime of the store and follow the
    /// `ep-{:016x}` template so they sort lexicographically by insertion
    /// order. If the process appends more than `u64::MAX` episodes, the
    /// key counter saturates and subsequent appends reuse the terminal key.
    pub async fn append(&self, episode: E) -> Result<String, PolicyError> {
        let mut inner = self.inner.lock().map_err(|_| PolicyError::Poisoned)?;
        inner.next_key = inner.next_key.saturating_add(1);
        let key = format!("ep-{:016x}", inner.next_key);
        inner.episodes.push((key.clone(), episode));
        Ok(key)
    }

    /// Return up to `k` episodes most similar to `query`, sorted by
    /// descending lexical score. Hits with score `0.0` are skipped.
    pub async fn retrieve_similar(
        &self,
        query: &str,
        k: usize,
    ) -> Result<Vec<InMemoryHit<E>>, PolicyError> {
        if k == 0 {
            return Ok(Vec::new());
        }
        let snapshot: Vec<(String, E)> = {
            let inner = self.inner.lock().map_err(|_| PolicyError::Poisoned)?;
            inner.episodes.clone()
        };
        let mut hits: Vec<InMemoryHit<E>> = snapshot
            .into_iter()
            .filter_map(|(key, episode)| {
                let score = lexical_score(query, episode.summary());
                if score > 0.0 {
                    Some(InMemoryHit {
                        episode,
                        score,
                        key,
                    })
                } else {
                    None
                }
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(k);
        Ok(hits)
    }

    /// Direct lookup by storage key.
    pub async fn get(&self, key: &str) -> Result<E, PolicyError> {
        let inner = self.inner.lock().map_err(|_| PolicyError::Poisoned)?;
        inner
            .episodes
            .iter()
            .find(|(stored_key, _)| stored_key == key)
            .map(|(_, episode)| episode.clone())
            .ok_or_else(|| PolicyError::NotFound(key.to_string()))
    }

    /// Number of episodes currently stored.
    pub async fn len(&self) -> Result<usize, PolicyError> {
        let inner = self.inner.lock().map_err(|_| PolicyError::Poisoned)?;
        Ok(inner.episodes.len())
    }

    /// Whether the store is empty.
    pub async fn is_empty(&self) -> Result<bool, PolicyError> {
        Ok(self.len().await? == 0)
    }
}

/// Token-overlap score in `[0, 1]`.
///
/// Returns the fraction of distinct normalized query tokens that appear
/// in `summary`. Normalization is intentionally simple and deterministic:
/// Unicode-aware lowercase via [`str::to_lowercase`] on each
/// whitespace-delimited token, and trim leading/trailing non-alphanumeric
/// characters. An empty query yields `0.0`.
fn lexical_score(query: &str, summary: &str) -> f32 {
    let query_tokens = normalized_tokens(query);
    if query_tokens.is_empty() {
        return 0.0;
    }
    let summary_tokens = normalized_tokens(summary);
    let intersection = query_tokens.intersection(&summary_tokens).count() as f32;
    intersection / query_tokens.len() as f32
}

fn normalized_tokens(input: &str) -> HashSet<String> {
    input
        .split_whitespace()
        .map(|token| token.trim_matches(|ch: char| !ch.is_alphanumeric()))
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    struct TestEpisode(&'static str);

    impl Episode for TestEpisode {
        fn summary(&self) -> &str {
            self.0
        }
    }

    #[test]
    fn append_assigns_unique_ordered_keys() {
        let store = InMemoryStore::<TestEpisode>::new();
        let key_one = pollster::block_on(store.append(TestEpisode("a"))).unwrap();
        let key_two = pollster::block_on(store.append(TestEpisode("b"))).unwrap();
        assert!(key_one < key_two);
    }

    #[test]
    fn retrieve_returns_top_k_by_score() {
        let store = InMemoryStore::<TestEpisode>::new();
        pollster::block_on(store.append(TestEpisode("powershell maintenance scheduled task")))
            .unwrap();
        pollster::block_on(store.append(TestEpisode("ddos amplification spike"))).unwrap();
        let hits = pollster::block_on(store.retrieve_similar("powershell scheduled", 5)).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits.first().unwrap().episode.0.contains("powershell"));
    }

    #[test]
    fn retrieve_skips_zero_score_hits() {
        let store = InMemoryStore::<TestEpisode>::new();
        pollster::block_on(store.append(TestEpisode("alpha bravo"))).unwrap();
        let hits = pollster::block_on(store.retrieve_similar("zulu", 5)).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn retrieve_matches_case_insensitively() {
        let store = InMemoryStore::<TestEpisode>::new();
        pollster::block_on(store.append(TestEpisode("PowerShell scheduled task"))).unwrap();
        let hits = pollster::block_on(store.retrieve_similar("powershell", 5)).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn retrieve_trims_simple_punctuation() {
        let store = InMemoryStore::<TestEpisode>::new();
        pollster::block_on(store.append(TestEpisode("powershell, scheduled-task beacon"))).unwrap();
        let hits =
            pollster::block_on(store.retrieve_similar("powershell scheduled-task", 5)).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn retrieve_handles_unicode_case_folding() {
        let store = InMemoryStore::<TestEpisode>::new();
        pollster::block_on(store.append(TestEpisode("ПОЛЬЗОВАТЕЛЬ logged in"))).unwrap();
        let hits = pollster::block_on(store.retrieve_similar("пользователь", 5)).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn retrieve_trims_unicode_punctuation() {
        let store = InMemoryStore::<TestEpisode>::new();
        pollster::block_on(store.append(TestEpisode("「scheduled-task」 beacon"))).unwrap();
        let hits = pollster::block_on(store.retrieve_similar("scheduled-task", 5)).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn get_returns_not_found_for_unknown_key() {
        let store = InMemoryStore::<TestEpisode>::new();
        let err = pollster::block_on(store.get("nope")).unwrap_err();
        assert!(matches!(err, PolicyError::NotFound(_)));
    }

    #[test]
    fn len_and_is_empty_track_inserts() {
        let store = InMemoryStore::<TestEpisode>::new();
        assert!(pollster::block_on(store.is_empty()).unwrap());
        pollster::block_on(store.append(TestEpisode("x"))).unwrap();
        assert_eq!(pollster::block_on(store.len()).unwrap(), 1);
        assert!(!pollster::block_on(store.is_empty()).unwrap());
    }

    #[test]
    fn k_zero_returns_empty() {
        let store = InMemoryStore::<TestEpisode>::new();
        pollster::block_on(store.append(TestEpisode("alpha"))).unwrap();
        assert!(
            pollster::block_on(store.retrieve_similar("alpha", 0))
                .unwrap()
                .is_empty()
        );
    }
}
