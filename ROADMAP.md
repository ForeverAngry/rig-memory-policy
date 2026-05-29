# rig-memory-policy — Roadmap

This roadmap is intentionally minimal. `rig-memory-policy` exists to provide
backend-neutral memory-policy primitives that every Rig memory-store adapter
can consume; it deliberately stays small and dependency-light.

The authoritative scope rules live in [AGENTS.md](AGENTS.md): no dependency
on `memvid-core`, `rig-compose`, `rig-resources`, `rig-mcp`, `rig-memvid`,
or `rig-retrieval-evals`. Anything that needs one of those crates belongs
in the adapter that consumes them, not here.

## Landed (Phase 1 — `0.1.x` → `0.2.x`)

- **`dedup`** — content-hash dedup helpers for hooks and compactors that
  must satisfy Rig's idempotency contract on `(conversation_id, messages)`
  ([src/dedup.rs](src/dedup.rs)).
- **`metadata`** — typed `FrameMetadata` + `FrameKind` envelope for
  per-entry backend metadata maps so downstream tooling (evals, memory
  inspectors, RAG pipelines) can reason about lifecycle
  ([src/metadata.rs](src/metadata.rs)).
- **`store`** — small capability traits (`TextWriter` + `Committable`) for
  hooks, compactors, and adapters that need to write text and explicitly
  flush it without depending on a concrete storage backend
  ([src/store.rs](src/store.rs)).
- **`inmem`** — deterministic no-disk lexical reference store (`Episode`,
  `InMemoryStore`, `InMemoryHit`) for tests, examples, offline modes, and
  backend-neutral fixtures ([src/inmem.rs](src/inmem.rs)).
- **`error::PolicyError`** — neutral error type shared by the helpers
  above ([src/error.rs](src/error.rs)).
- **`rig-memvid` adapter integration** — `rig-memvid` re-exports the
  surfaces above as compatibility shims so historic call sites
  (`rig_memvid::{Episode, InMemoryStore, InMemoryHit}`) keep working.

## Next Work (Phase 2 candidates)

These are *candidate* items only. None will land until a concrete adapter
needs the surface; speculative additions are explicitly out of scope.

1. **Backend-neutral retention / scope projection helpers** — formalize
   the patterns that adapters currently re-invent for "keep last N
   turns", "scope by conversation", and "TTL on metadata frames" so the
   logic lives in one place. *Triggered by:* a second backend adapter
   (SQLite, LanceDB, Qdrant, or filesystem) reaching parity with the
   `rig-memvid` adapter.
2. **Encryption-policy hook surface** — a backend-neutral trait that
   adapters with encryption support (e.g. `memvid-core`'s encrypted
   archives) can implement, so policy code can decide *when* to encrypt
   without knowing *how*. *Triggered by:* a second backend with native
   encryption.
3. **Object-safe backend adapter trait** — a minimal `MemoryBackend`
   trait that hooks/compactors can hold `dyn`-style without pulling in
   `rig-core`'s vector-store traits. *Triggered by:* a concrete need
   from `rig-retrieval-evals` or a host application that wants to swap
   backends behind a feature flag at runtime.
4. **Frame-kind extensions** — additional `FrameKind` variants as new
   memory lifecycles appear in adapter code (e.g. `Reflection`,
   `ToolObservation`). Each variant is additive and backwards-compatible
   under `#[non_exhaustive]`.

## Out of scope

- Anything that requires a concrete storage backend.
- Anything that requires `tokio` or another async runtime in
  `[dependencies]`.
- Vendoring or wrapping `rig-core`, `memvid-core`, `rig-compose`,
  `rig-resources`, `rig-mcp`, `rig-memvid`, or `rig-retrieval-evals`.

## Validation

Each release is gated by `just check` (`cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings` over the default and
all-features matrices, `cargo test --all-features`, MSRV build, and
`cargo doc --no-deps` with strict lints). See [justfile](justfile) for
the exact commands.
