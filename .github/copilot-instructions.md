# rig-memory-policy — Copilot Instructions

See [AGENTS.md](../AGENTS.md) for the authoritative copy. Summary:

- Rust 2024, MSRV 1.89. Library is runtime-agnostic — do not add `tokio`
  to `[dependencies]`.
- Errors: typed `thiserror::Error` in [src/error.rs](../src/error.rs).
  Return `Result<_, PolicyError>`.
- Never `.await` while holding a `Mutex`/`RwLock` guard
  (`clippy::await_holding_lock = deny`).
- No `unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!`/`dbg!`/indexing
  in library code (clippy `deny`/`forbid`). Use `?`,
  `ok_or(PolicyError::…)`, `get(..)`, pattern matching.
- `unwrap`/`expect` allowed in `tests/`, `#[cfg(test)]` blocks (gate with
  `#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]`).
- Use `tracing` for logs; no `println!` in library code.
- Document new `pub` items with `///` rustdoc. Re-export from
  [src/lib.rs](../src/lib.rs).

## Validation

```sh
just check
# fmt --check + clippy (default + all-features) + tests + msrv + rustdoc strict
```

## Scope

Do not depend on `memvid-core`, `rig-compose`, `rig-resources`, `rig-mcp`,
`rig-memvid`, or `rig-retrieval-evals`. The crate must remain consumable
by every memory-store backend.
