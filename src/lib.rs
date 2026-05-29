//! Backend-agnostic memory-policy primitives shared across Rig memory-store
//! adapters (`rig-memvid` and future backends like SQLite, LanceDB, Qdrant,
//! plain filesystem, etc.).
//!
//! Public surface:
//!
//! - [`dedup`] — in-process content-hash dedup for hooks/compactors that must
//!   satisfy `rig::memory::{DemotionHook, Compactor}`'s idempotency contract.
//! - [`metadata`] — typed envelope written into a backend's per-entry
//!   metadata so downstream tools (evals, inspectors, RAG pipelines) can
//!   reason about the lifecycle that produced each entry.
//! - [`inmem`] — a deterministic no-disk reference store for tests,
//!   examples, and offline modes.
//! - [`scope`] — normalized exact and hierarchical scope matching helpers for
//!   backend isolation and provenance projection.
//! - [`retention`] — deterministic keep/drop/defer policy evaluation over
//!   backend-provided frame metadata and optional timestamps/sequence numbers.
//! - [`store`] — the minimal [`TextWriter`] + [`Committable`] capability
//!   traits backends impl so hooks and compactors can be generic over the
//!   storage engine.
//! - [`error`] — neutral `PolicyError` for failures in the above helpers.
//!
//! This crate has **no** dependency on `memvid-core` or any specific storage
//! engine. Backends are expected to wrap these primitives in their own
//! adapter (e.g. `rig-memvid` adds `.mv2` framing on top).
//!
//! See the [`store`] module docs for the audit-driven rationale behind the
//! deliberately narrow trait surface.

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod dedup;
pub mod error;
pub mod inmem;
pub mod metadata;
pub mod retention;
pub mod scope;
pub mod store;

pub use error::PolicyError;
pub use inmem::{Episode, InMemoryHit, InMemoryStore};
pub use metadata::{FrameKind, FrameMetadata};
pub use retention::{RetentionCandidate, RetentionDecision, RetentionPolicy, RetentionRule};
pub use scope::{Scope, normalize_scope, scope_matches, scope_path};
pub use store::{Committable, TextWriter};
