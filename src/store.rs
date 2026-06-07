//! Backend-agnostic memory-store capability traits.
//!
//! These traits express the **lowest common denominator** of what a memory
//! backend must provide so that lifecycle helpers (persistence hooks,
//! compactors, demotion policies) can be generic over the storage engine.
//!
//! ## Scope
//!
//! Phase 2 of the [`rig-memvid` extraction
//! plan](https://github.com/ForeverAngry/rig-memvid/issues/28) audited
//! `MemvidStore` and `InMemoryStore<E>` side-by-side. The two had **zero**
//! intersecting public method signatures: payloads differ (`&str` vs
//! `E: Episode`), commit semantics differ (durable vs append-only memory),
//! search semantics differ (semantic vs lexical), and error types differ.
//!
//! Forcing a unifying base trait would either flatten meaningful
//! differences or balloon associated types past the point of usefulness.
//! Instead, this module ships only the **two operations** every
//! durable-text backend genuinely shares — write a `&str` with
//! backend-specific options, and commit buffered writes. Backend-specific
//! surface (memory cards, entity graphs, vector search) remains inherent
//! on each store and is **not** part of this trait family.
//!
//! ## Async-ness
//!
//! Both traits use `async fn` in trait (stable since Rust 1.75; MSRV 1.89
//! satisfies this). Backends whose underlying API is synchronous can
//! implement the trait by wrapping the sync call in an `async` block at
//! zero cost. Generic consumers that require `Send`-bounded futures (e.g.
//! when spawning) can add `where <S as TextWriter>::write_text(..): Send`
//! style bounds at the call site, or use the `trait_variant` macro
//! pattern in a follow-up release.
//!
//! ## Example
//!
//! ```rust
//! use rig_memory_policy::store::{Committable, TextWriter};
//! use std::sync::Mutex;
//!
//! struct StringBuf {
//!     pending: Mutex<Vec<String>>,
//!     committed: Mutex<Vec<String>>,
//! }
//!
//! #[derive(Debug, thiserror::Error)]
//! #[error("lock poisoned")]
//! struct BufErr;
//!
//! impl TextWriter for StringBuf {
//!     type Options = ();
//!     type Id = usize;
//!     type Error = BufErr;
//!     async fn write_text(&self, text: &str, _: ()) -> Result<usize, BufErr> {
//!         let mut guard = self.pending.lock().map_err(|_| BufErr)?;
//!         guard.push(text.to_owned());
//!         Ok(guard.len() - 1)
//!     }
//! }
//!
//! impl Committable for StringBuf {
//!     type Error = BufErr;
//!     async fn commit(&self) -> Result<(), BufErr> {
//!         let mut pending = self.pending.lock().map_err(|_| BufErr)?;
//!         let mut committed = self.committed.lock().map_err(|_| BufErr)?;
//!         committed.append(&mut pending);
//!         Ok(())
//!     }
//! }
//! ```

/// A backend that accepts text writes keyed by some backend-defined id.
///
/// Implementations may buffer writes until [`Committable::commit`] is called.
/// Whether `write_text` is durable on return is backend-defined; consumers
/// that need a durability guarantee should always pair `write_text` with
/// `commit`.
///
/// `Options` carries backend-specific per-write metadata (tags, scope,
/// dedup hints, frame envelope, etc.). Backends with no per-write options
/// should use `type Options = ();`.
pub trait TextWriter {
    /// Backend-specific options accompanying each write.
    type Options;
    /// Backend-specific identifier returned per successful write.
    type Id;
    /// Error type for failed writes.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Buffer or persist a text payload with the given options, returning
    /// the backend's identifier for the resulting entry.
    ///
    /// The returned future is **not** required to be `Send`; consumers
    /// that need to spawn it across threads must add an explicit `Send`
    /// bound at the call site.
    fn write_text(
        &self,
        text: &str,
        options: Self::Options,
    ) -> impl core::future::Future<Output = Result<Self::Id, Self::Error>>;
}

/// A backend that supports an explicit commit step to durably persist
/// previously buffered writes.
///
/// Backends that persist on every write may still implement `Committable`
/// as a no-op so they can be used interchangeably in generic code.
pub trait Committable {
    /// Error type for failed commits.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Flush any pending writes to durable storage.
    fn commit(&self) -> impl core::future::Future<Output = Result<(), Self::Error>>;
}

/// A backend that supports explicit deletion of previously written text.
pub trait TextDeleter {
    /// Backend-specific identifier used to target a deletion.
    type Id;
    /// Error type for failed deletions.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Delete a previously written text payload by its backend-assigned identifier.
    fn delete_text(
        &self,
        id: Self::Id,
    ) -> impl core::future::Future<Output = Result<(), Self::Error>>;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, thiserror::Error)]
    #[error("test backend error")]
    struct TestErr;

    struct CountingStore {
        pending: Mutex<Vec<String>>,
        committed: Mutex<Vec<String>>,
    }

    impl CountingStore {
        fn new() -> Self {
            Self {
                pending: Mutex::new(Vec::new()),
                committed: Mutex::new(Vec::new()),
            }
        }
    }

    impl TextWriter for CountingStore {
        type Options = ();
        type Id = u64;
        type Error = TestErr;
        async fn write_text(&self, text: &str, _: ()) -> Result<u64, TestErr> {
            let mut guard = self.pending.lock().map_err(|_| TestErr)?;
            guard.push(text.to_owned());
            Ok((guard.len() - 1) as u64)
        }
    }

    impl Committable for CountingStore {
        type Error = TestErr;
        async fn commit(&self) -> Result<(), TestErr> {
            let mut pending = self.pending.lock().map_err(|_| TestErr)?;
            let mut committed = self.committed.lock().map_err(|_| TestErr)?;
            committed.append(&mut pending);
            Ok(())
        }
    }

    /// Toy generic helper proving a hook-like function can be written
    /// against the trait pair alone (no backend knowledge).
    async fn persist_all<S>(
        store: &S,
        lines: &[&str],
    ) -> Result<Vec<<S as TextWriter>::Id>, <S as TextWriter>::Error>
    where
        S: TextWriter<Options = ()> + Committable<Error = <S as TextWriter>::Error>,
    {
        let mut ids = Vec::with_capacity(lines.len());
        for line in lines {
            ids.push(store.write_text(line, ()).await?);
        }
        store.commit().await?;
        Ok(ids)
    }

    #[test]
    fn write_then_commit_moves_pending_to_committed() {
        let store = CountingStore::new();
        let ids = pollster::block_on(persist_all(&store, &["alpha", "beta", "gamma"])).unwrap();
        assert_eq!(ids, vec![0, 1, 2]);
        assert!(store.pending.lock().unwrap().is_empty());
        assert_eq!(
            store.committed.lock().unwrap().clone(),
            vec!["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()],
        );
    }

    #[test]
    fn write_without_commit_keeps_pending() {
        let store = CountingStore::new();
        let id = pollster::block_on(store.write_text("orphan", ())).unwrap();
        assert_eq!(id, 0);
        assert_eq!(store.pending.lock().unwrap().len(), 1);
        assert!(store.committed.lock().unwrap().is_empty());
    }
}
