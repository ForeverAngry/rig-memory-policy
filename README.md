# rig-memory-policy

[![Crates.io](https://img.shields.io/crates/v/rig-memory-policy.svg)](https://crates.io/crates/rig-memory-policy)
[![Documentation](https://docs.rs/rig-memory-policy/badge.svg)](https://docs.rs/rig-memory-policy)
[![CI](https://github.com/ForeverAngry/rig-memory-policy/actions/workflows/ci.yml/badge.svg)](https://github.com/ForeverAngry/rig-memory-policy/actions/workflows/ci.yml)

Backend-agnostic memory-policy primitives shared across [Rig](https://crates.io/crates/rig-core)
memory-store adapters (e.g. [`rig-memvid`](https://crates.io/crates/rig-memvid)
and future SQLite / LanceDB / Qdrant / filesystem backends).

This crate has **no** dependency on `memvid-core` or any specific storage
engine. Adapters wrap these primitives in their own backend-specific code.

## Phase 1 surface

- [`dedup`](https://docs.rs/rig-memory-policy/latest/rig_memory_policy/dedup/index.html)
  — in-process content-hash dedup for hooks/compactors that must satisfy
  `rig::memory::{DemotionHook, Compactor}`'s idempotency contract on
  `(conversation_id, messages)`.
- [`metadata`](https://docs.rs/rig-memory-policy/latest/rig_memory_policy/metadata/index.html)
  — typed envelope (`FrameMetadata` + `FrameKind`) written into a backend's
  per-entry metadata map so downstream tools (evals, memory inspectors, RAG
  pipelines) can reason about the lifecycle that produced each entry.
- [`error::PolicyError`](https://docs.rs/rig-memory-policy/latest/rig_memory_policy/error/enum.PolicyError.html)
  — neutral error type for the helpers above.

Trait surface (`MemoryStore`, capability sub-traits, generic
`PersistHook`/`DemotionHook`/`StoringCompactor`) lands in subsequent phases —
see the tracking issue in
[`rig-memvid#28`](https://github.com/ForeverAngry/rig-memvid/issues/28).

## License

MIT
