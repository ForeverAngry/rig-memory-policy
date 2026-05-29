# rig-memory-policy

[![Crates.io](https://img.shields.io/crates/v/rig-memory-policy.svg)](https://crates.io/crates/rig-memory-policy)
[![Documentation](https://docs.rs/rig-memory-policy/badge.svg)](https://docs.rs/rig-memory-policy)
[![CI](https://github.com/ForeverAngry/rig-memory-policy/actions/workflows/ci.yml/badge.svg)](https://github.com/ForeverAngry/rig-memory-policy/actions/workflows/ci.yml)

Backend-agnostic memory-policy primitives shared across [Rig](https://crates.io/crates/rig-core)
memory-store adapters (e.g. [`rig-memvid`](https://crates.io/crates/rig-memvid)
and future SQLite / LanceDB / Qdrant / filesystem backends).

This crate has **no** dependency on `memvid-core` or any specific storage
engine. Adapters wrap these primitives in their own backend-specific code.

## Public surface

- [`dedup`](https://docs.rs/rig-memory-policy/latest/rig_memory_policy/dedup/index.html)
  — in-process content-hash dedup for hooks/compactors that must satisfy
  `rig::memory::{DemotionHook, Compactor}`'s idempotency contract on
  `(conversation_id, messages)`.
- [`metadata`](https://docs.rs/rig-memory-policy/latest/rig_memory_policy/metadata/index.html)
  — typed envelope (`FrameMetadata` + `FrameKind`) written into a backend's
  per-entry metadata map so downstream tools (evals, memory inspectors, RAG
  pipelines) can reason about the lifecycle that produced each entry.
- [`store`](https://docs.rs/rig-memory-policy/latest/rig_memory_policy/store/index.html)
  — small capability traits (`TextWriter` + `Committable`) for hooks,
  compactors, and adapters that need to write text and explicitly flush it
  without depending on a concrete storage backend.
- [`inmem`](https://docs.rs/rig-memory-policy/latest/rig_memory_policy/inmem/index.html)
  — deterministic no-disk lexical reference store (`Episode`,
  `InMemoryStore`, `InMemoryHit`) for tests, examples, offline modes, and
  backend-neutral fixtures.
- [`scope`](https://docs.rs/rig-memory-policy/latest/rig_memory_policy/scope/index.html)
  — normalized exact and hierarchical scope matching helpers for backend
  isolation, tenant boundaries, and provenance projection.
- [`retention`](https://docs.rs/rig-memory-policy/latest/rig_memory_policy/retention/index.html)
  — deterministic keep/drop/defer decisions over decoded frame metadata plus
  backend-provided sequence numbers, timestamps, and retention labels.
- [`error::PolicyError`](https://docs.rs/rig-memory-policy/latest/rig_memory_policy/error/enum.PolicyError.html)
  — neutral error type shared by the helpers above.

## Adapter pattern

Backend crates keep ownership of their storage-specific APIs while leaning on
these shared primitives for policy-level behaviour. For example,
[`rig-memvid`](https://crates.io/crates/rig-memvid) wraps Memvid's `.mv2`
archive format, but re-exports this crate's dedup, metadata, in-memory, and
text-write capability surfaces to preserve its historic public paths.

New backend-neutral code should prefer importing directly from
`rig-memory-policy`:

```rust,no_run
use rig_memory_policy::{Episode, InMemoryStore};

#[derive(Clone)]
struct Finding {
    summary: String,
}

impl Episode for Finding {
    fn summary(&self) -> &str {
        &self.summary
    }
}

# async fn run() -> Result<(), rig_memory_policy::PolicyError> {
let store = InMemoryStore::<Finding>::new();
store
    .append(Finding {
        summary: "scheduled maintenance".into(),
    })
    .await?;
let hits = store.retrieve_similar("maintenance", 5).await?;
assert_eq!(hits.len(), 1);
# Ok(()) }
```

Existing `rig-memvid` callers can continue using `rig_memvid::inmem::*` and
the top-level `rig_memvid::{Episode, InMemoryStore, InMemoryHit}` re-exports;
those are compatibility shims over this crate.

## Retention and scope policies

Backends can evaluate retention decisions without adopting a shared storage
trait. Decode a frame's `FrameMetadata`, attach any backend-local facts (such
as sequence numbers or write timestamps), and run it through a pure
`RetentionPolicy` before compaction, deletion, or archival.

```rust
use rig_memory_policy::{
  FrameKind, FrameMetadata, RetentionCandidate, RetentionDecision,
  RetentionPolicy, Scope,
};

let metadata = FrameMetadata {
  schema_version: 1,
  kind: FrameKind::CompactionSummary,
  conversation_id: "conv-1".into(),
  chat_role: "assistant".into(),
  dedup_key: "abc".into(),
  scope: Some("tenant-a/project-1".into()),
};

let policy = RetentionPolicy::new()
  .keep_summaries()
  .drop_outside_scope(Some(Scope::new("tenant-a/project-1")))
  .drop_written_before(1_700_000_000_000)
  .default_decision(RetentionDecision::Drop);

let decision = policy.evaluate(
  RetentionCandidate::new(&metadata).with_written_at_unix_ms(1_600_000_000_000),
);
assert_eq!(decision, RetentionDecision::Keep);
```

The policy helpers intentionally use primitive caller-supplied timestamps and
string-backed labels. That keeps the crate runtime-agnostic and avoids baking
one backend's storage model into the shared contract.

## Status

- Crate version: `0.2.1`.
- Rust edition: 2024.
- MSRV: 1.89.
- Runtime stance: runtime-agnostic library. The only runtime dependencies
  are pure-Rust utility crates (`blake3`, `serde`, `serde_json`,
  `thiserror`, `tracing`); `tokio` is not in `[dependencies]`.
- Scope stance: backend-neutral. The crate must not depend on
  `memvid-core`, `rig-compose`, `rig-resources`, `rig-mcp`, `rig-memvid`,
  or `rig-retrieval-evals` so every memory-store backend can consume it.
- Stability: the Phase 1 public surface listed above is considered stable for
  `0.2.x`. New backend-neutral capabilities land as additive
  modules; breaking changes go through a minor-version bump and are
  flagged in [CHANGELOG.md](CHANGELOG.md).

See [ROADMAP.md](ROADMAP.md) for the published Phase 1 surface and
Phase 2 candidates.

## License

MIT
