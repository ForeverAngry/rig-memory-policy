//! Neutral error type for `rig-memory-policy` helpers.

use thiserror::Error;

/// Errors produced by [`crate::dedup`] and other policy helpers.
///
/// New variants are additive. The enum is `#[non_exhaustive]` so future
/// helpers can extend it without a breaking change.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PolicyError {
    /// An internal lock was poisoned by a previous panic. The affected
    /// instance should be discarded.
    #[error("internal lock poisoned")]
    Poisoned,
}
