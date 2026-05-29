# AGENTS.md

Guidance for AI coding agents working in `rig-memory-policy`.

## Project

Backend-agnostic memory-policy primitives shared across Rig memory-store
adapters. Phase 1 surface:

- `dedup` — content-hash dedup for hooks/compactors.
- `metadata` — typed `FrameMetadata` / `FrameKind` envelope.
- `error::PolicyError` — neutral error type.

This crate has **no** dependency on `memvid-core` or any specific storage
engine. It is consumed by `rig-memvid` and (in future phases) by other
backend adapters.

## Rules

- Rust 2024, MSRV 1.89. Library is runtime-agnostic; do not add `tokio`
  to `[dependencies]`.
- Errors: typed `thiserror` enum in [src/error.rs](src/error.rs); return
  `Result<_, PolicyError>`. No ad-hoc `Box<dyn Error>` or `String` error
  types.
- Never `.await` while holding a `Mutex`/`RwLock` guard
  (`clippy::await_holding_lock = deny`).
- No `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, `dbg!`,
  indexing/slicing, or `unreachable!` in library code — clippy
  `deny`/`forbid`. Use `?`, `ok_or(PolicyError::...)`, `get(..)`, `match`.
  Allowed in `tests/`, `#[cfg(test)]` blocks (gate with
  `#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]`).
- Use `tracing` for logs; no `println!` in library code.
- Document new `pub` items with `///` rustdoc.
- Re-export new public items from [src/lib.rs](src/lib.rs).

## Validation

```sh
just check
# fmt --check + clippy (default + all-features) + tests + msrv + rustdoc strict
```

## Scope

Do not add a dependency on `memvid-core`, `rig-compose`, `rig-resources`,
`rig-mcp`, `rig-memvid`, or `rig-retrieval-evals`. This crate must remain
consumable by every memory-store backend. The only allowed runtime
dependencies are pure-Rust utility crates (`blake3`, `serde`, `serde_json`,
`thiserror`, `tracing`).
