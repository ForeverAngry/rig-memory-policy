//! Backend-agnostic memory-policy primitives shared across Rig memory-store
//! adapters (`rig-memvid` and future backends like SQLite, LanceDB, Qdrant,
//! plain filesystem, etc.).
//!
//! Phase 1 surface (pure helpers, no trait design yet):
//!
//! - [`dedup`] — in-process content-hash dedup for hooks/compactors that must
//!   satisfy `rig::memory::{DemotionHook, Compactor}`'s idempotency contract.
//! - [`metadata`] — typed envelope written into a backend's per-entry
//!   metadata so downstream tools (evals, inspectors, RAG pipelines) can
//!   reason about the lifecycle that produced each entry.
//! - [`error`] — neutral `PolicyError` for failures in the above helpers.
//!
//! This crate has **no** dependency on `memvid-core` or any specific storage
//! engine. Backends are expected to wrap these primitives in their own
//! adapter (e.g. `rig-memvid` adds `.mv2` framing on top).
//!
//! Trait surface (`MemoryStore`, capability sub-traits, generic hooks) lands
//! in subsequent phases — see the tracking issue for details.

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod dedup;
pub mod error;
pub mod metadata;

pub use error::PolicyError;
