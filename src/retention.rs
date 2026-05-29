//! Deterministic retention decisions over backend-provided memory metadata.
//!
//! This module intentionally avoids storage APIs and wall-clock dependencies.
//! Backends pass in frame metadata plus optional sequence/timestamp facts, and
//! receive a pure keep/drop/defer decision they can apply however their store
//! represents deletes, compaction, or archival tiers.

use crate::metadata::{FrameKind, FrameMetadata};
use crate::scope::{Scope, scope_matches};

/// A backend-provided candidate for retention evaluation.
///
/// The candidate borrows the stable [`FrameMetadata`] envelope and carries
/// optional facts supplied by the backend. Missing facts simply make rules that
/// depend on them non-matching.
#[derive(Debug, Clone, Copy)]
pub struct RetentionCandidate<'a> {
    /// Stable frame metadata decoded from the backend entry.
    pub metadata: &'a FrameMetadata,
    /// Backend-specific monotonic sequence number, where larger is newer.
    pub sequence: Option<u64>,
    /// Backend-supplied write timestamp in Unix milliseconds.
    pub written_at_unix_ms: Option<u64>,
    /// Backend-supplied last-access timestamp in Unix milliseconds.
    pub last_accessed_unix_ms: Option<u64>,
    /// Optional retention label stamped by the backend or host app.
    pub retention_label: Option<&'a str>,
}

impl<'a> RetentionCandidate<'a> {
    /// Construct a candidate from required frame metadata.
    #[must_use]
    pub fn new(metadata: &'a FrameMetadata) -> Self {
        Self {
            metadata,
            sequence: None,
            written_at_unix_ms: None,
            last_accessed_unix_ms: None,
            retention_label: None,
        }
    }

    /// Attach a backend-specific monotonic sequence number.
    #[must_use]
    pub fn with_sequence(mut self, sequence: u64) -> Self {
        self.sequence = Some(sequence);
        self
    }

    /// Attach a backend-supplied write timestamp in Unix milliseconds.
    #[must_use]
    pub fn with_written_at_unix_ms(mut self, written_at_unix_ms: u64) -> Self {
        self.written_at_unix_ms = Some(written_at_unix_ms);
        self
    }

    /// Attach a backend-supplied last-access timestamp in Unix milliseconds.
    #[must_use]
    pub fn with_last_accessed_unix_ms(mut self, last_accessed_unix_ms: u64) -> Self {
        self.last_accessed_unix_ms = Some(last_accessed_unix_ms);
        self
    }

    /// Attach a host-defined retention label.
    #[must_use]
    pub fn with_retention_label(mut self, retention_label: &'a str) -> Self {
        self.retention_label = Some(retention_label);
        self
    }
}

/// Result of evaluating one candidate against a retention policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionDecision {
    /// The backend should retain the candidate.
    Keep,
    /// The backend may delete, compact away, or archive the candidate.
    Drop,
    /// The policy has no opinion; the backend should apply its default.
    Defer,
}

/// One deterministic retention rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetentionRule {
    /// Keep candidates whose frame kind matches `kind`.
    KeepFrameKind(FrameKind),
    /// Keep candidates whose normalized scope exactly matches this scope.
    KeepScope(Scope),
    /// Keep candidates at or above `min_sequence`, where larger sequence
    /// values are newer.
    KeepRecent {
        /// Minimum sequence number to keep, where larger sequence values are
        /// newer.
        min_sequence: u64,
    },
    /// Drop candidates whose write timestamp is older than `older_than_unix_ms`.
    DropWrittenBefore {
        /// Exclusive Unix-millisecond threshold; candidates written before
        /// this value are dropped.
        older_than_unix_ms: u64,
    },
    /// Drop candidates whose last-access timestamp is older than
    /// `older_than_unix_ms`.
    DropLastAccessedBefore {
        /// Exclusive Unix-millisecond threshold; candidates last accessed
        /// before this value are dropped.
        older_than_unix_ms: u64,
    },
    /// Drop candidates whose normalized scope does not exactly match
    /// `required_scope`. `None` requires an unscoped candidate.
    DropOutsideScope {
        /// Required exact scope. `None` requires candidates to be unscoped.
        required_scope: Option<Scope>,
    },
    /// Keep candidates with a host-defined retention label.
    KeepLabel(String),
    /// Drop candidates with a host-defined retention label.
    DropLabel(String),
}

impl RetentionRule {
    /// Evaluate this rule against a single candidate.
    #[must_use]
    pub fn evaluate(&self, candidate: RetentionCandidate<'_>) -> RetentionDecision {
        match self {
            Self::KeepFrameKind(kind) if candidate.metadata.kind == *kind => {
                RetentionDecision::Keep
            }
            Self::KeepScope(scope) if scope.matches(candidate.metadata.scope.as_deref()) => {
                RetentionDecision::Keep
            }
            Self::KeepRecent { min_sequence } => candidate
                .sequence
                .map(|sequence| {
                    if sequence >= *min_sequence {
                        RetentionDecision::Keep
                    } else {
                        RetentionDecision::Defer
                    }
                })
                .unwrap_or(RetentionDecision::Defer),
            Self::DropWrittenBefore { older_than_unix_ms } => candidate
                .written_at_unix_ms
                .map(|written_at| {
                    if written_at < *older_than_unix_ms {
                        RetentionDecision::Drop
                    } else {
                        RetentionDecision::Defer
                    }
                })
                .unwrap_or(RetentionDecision::Defer),
            Self::DropLastAccessedBefore { older_than_unix_ms } => candidate
                .last_accessed_unix_ms
                .map(|last_accessed| {
                    if last_accessed < *older_than_unix_ms {
                        RetentionDecision::Drop
                    } else {
                        RetentionDecision::Defer
                    }
                })
                .unwrap_or(RetentionDecision::Defer),
            Self::DropOutsideScope { required_scope } => {
                let required = required_scope.as_ref().map(Scope::as_str);
                if scope_matches(required, candidate.metadata.scope.as_deref()) {
                    RetentionDecision::Defer
                } else {
                    RetentionDecision::Drop
                }
            }
            Self::KeepLabel(label) if candidate.retention_label == Some(label.as_str()) => {
                RetentionDecision::Keep
            }
            Self::DropLabel(label) if candidate.retention_label == Some(label.as_str()) => {
                RetentionDecision::Drop
            }
            _ => RetentionDecision::Defer,
        }
    }
}

/// Ordered set of retention rules.
///
/// Rules are evaluated in insertion order. The first [`RetentionDecision::Keep`]
/// or [`RetentionDecision::Drop`] wins; [`RetentionDecision::Defer`] falls
/// through to later rules and finally to `default_decision`.
///
/// # Example
///
/// ```
/// use rig_memory_policy::{
///     FrameKind, FrameMetadata, RetentionCandidate, RetentionDecision,
///     RetentionPolicy,
/// };
///
/// let metadata = FrameMetadata {
///     schema_version: 1,
///     kind: FrameKind::CompactionSummary,
///     conversation_id: "conv-1".into(),
///     chat_role: "assistant".into(),
///     dedup_key: "abc".into(),
///     scope: Some("tenant-a".into()),
/// };
/// let policy = RetentionPolicy::new()
///     .keep_summaries()
///     .drop_written_before(1_700_000_000_000)
///     .default_decision(RetentionDecision::Drop);
///
/// let decision = policy.evaluate(
///     RetentionCandidate::new(&metadata).with_written_at_unix_ms(1_600_000_000_000),
/// );
/// assert_eq!(decision, RetentionDecision::Keep);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionPolicy {
    rules: Vec<RetentionRule>,
    default_decision: RetentionDecision,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            default_decision: RetentionDecision::Defer,
        }
    }
}

impl RetentionPolicy {
    /// Construct an empty policy with [`RetentionDecision::Defer`] as default.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a custom rule to the end of the policy.
    #[must_use]
    pub fn rule(mut self, rule: RetentionRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// Set the decision returned when no rule matches.
    #[must_use]
    pub fn default_decision(mut self, decision: RetentionDecision) -> Self {
        self.default_decision = decision;
        self
    }

    /// Keep all compaction summaries.
    #[must_use]
    pub fn keep_summaries(self) -> Self {
        self.rule(RetentionRule::KeepFrameKind(FrameKind::CompactionSummary))
    }

    /// Keep all demoted messages.
    #[must_use]
    pub fn keep_demoted_messages(self) -> Self {
        self.rule(RetentionRule::KeepFrameKind(FrameKind::DemotedMessage))
    }

    /// Keep candidates in the required exact scope.
    #[must_use]
    pub fn keep_scope(self, scope: impl Into<Scope>) -> Self {
        self.rule(RetentionRule::KeepScope(scope.into()))
    }

    /// Keep candidates at or above `min_sequence`.
    #[must_use]
    pub fn keep_recent(self, min_sequence: u64) -> Self {
        self.rule(RetentionRule::KeepRecent { min_sequence })
    }

    /// Drop candidates written before `older_than_unix_ms`.
    #[must_use]
    pub fn drop_written_before(self, older_than_unix_ms: u64) -> Self {
        self.rule(RetentionRule::DropWrittenBefore { older_than_unix_ms })
    }

    /// Drop candidates last accessed before `older_than_unix_ms`.
    #[must_use]
    pub fn drop_last_accessed_before(self, older_than_unix_ms: u64) -> Self {
        self.rule(RetentionRule::DropLastAccessedBefore { older_than_unix_ms })
    }

    /// Drop candidates outside an exact scope. `None` keeps only unscoped
    /// candidates from matching this rule.
    #[must_use]
    pub fn drop_outside_scope(self, required_scope: Option<impl Into<Scope>>) -> Self {
        self.rule(RetentionRule::DropOutsideScope {
            required_scope: required_scope.map(Into::into),
        })
    }

    /// Keep candidates with the provided host-defined retention label.
    #[must_use]
    pub fn keep_label(self, label: impl Into<String>) -> Self {
        self.rule(RetentionRule::KeepLabel(label.into()))
    }

    /// Drop candidates with the provided host-defined retention label.
    #[must_use]
    pub fn drop_label(self, label: impl Into<String>) -> Self {
        self.rule(RetentionRule::DropLabel(label.into()))
    }

    /// Evaluate one candidate against this policy.
    #[must_use]
    pub fn evaluate(&self, candidate: RetentionCandidate<'_>) -> RetentionDecision {
        self.rules
            .iter()
            .map(|rule| rule.evaluate(candidate))
            .find(|decision| *decision != RetentionDecision::Defer)
            .unwrap_or(self.default_decision)
    }

    /// Return the ordered rules in this policy.
    #[must_use]
    pub fn rules(&self) -> &[RetentionRule] {
        &self.rules
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn metadata(kind: FrameKind, scope: Option<&str>) -> FrameMetadata {
        FrameMetadata {
            schema_version: 1,
            kind,
            conversation_id: "conv".to_string(),
            chat_role: "assistant".to_string(),
            dedup_key: "key".to_string(),
            scope: scope.map(str::to_string),
        }
    }

    #[test]
    fn keep_rule_wins_before_later_drop_rule() {
        let metadata = metadata(FrameKind::CompactionSummary, Some("tenant-a"));
        let policy = RetentionPolicy::new()
            .keep_summaries()
            .drop_written_before(200);
        let candidate = RetentionCandidate::new(&metadata).with_written_at_unix_ms(100);
        assert_eq!(policy.evaluate(candidate), RetentionDecision::Keep);
    }

    #[test]
    fn drop_rule_wins_before_later_keep_rule() {
        let metadata = metadata(FrameKind::CompactionSummary, Some("tenant-a"));
        let policy = RetentionPolicy::new()
            .drop_written_before(200)
            .keep_summaries();
        let candidate = RetentionCandidate::new(&metadata).with_written_at_unix_ms(100);
        assert_eq!(policy.evaluate(candidate), RetentionDecision::Drop);
    }

    #[test]
    fn recent_rule_keeps_at_or_above_min_sequence() {
        let metadata = metadata(FrameKind::DemotedMessage, Some("tenant-a"));
        let policy = RetentionPolicy::new()
            .keep_recent(10)
            .default_decision(RetentionDecision::Drop);
        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&metadata).with_sequence(10)),
            RetentionDecision::Keep
        );
        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&metadata).with_sequence(9)),
            RetentionDecision::Drop
        );
    }

    #[test]
    fn ttl_like_rule_drops_old_written_frames() {
        let metadata = metadata(FrameKind::DemotedMessage, None);
        let policy = RetentionPolicy::new().drop_written_before(1_000);
        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&metadata).with_written_at_unix_ms(999)),
            RetentionDecision::Drop
        );
        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&metadata).with_written_at_unix_ms(1_000)),
            RetentionDecision::Defer
        );
    }

    #[test]
    fn missing_optional_fields_do_not_match_field_dependent_rules() {
        let metadata = metadata(FrameKind::DemotedMessage, None);
        let policy = RetentionPolicy::new()
            .keep_recent(10)
            .drop_written_before(1_000)
            .drop_last_accessed_before(1_000);
        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&metadata)),
            RetentionDecision::Defer
        );
    }

    #[test]
    fn scope_guard_drops_candidates_outside_exact_scope() {
        let inside = metadata(FrameKind::DemotedMessage, Some("tenant-a"));
        let outside = metadata(FrameKind::DemotedMessage, Some("tenant-b"));
        let unscoped = metadata(FrameKind::DemotedMessage, None);
        let policy = RetentionPolicy::new().drop_outside_scope(Some("tenant-a"));

        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&inside)),
            RetentionDecision::Defer
        );
        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&outside)),
            RetentionDecision::Drop
        );
        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&unscoped)),
            RetentionDecision::Drop
        );
    }

    #[test]
    fn label_rules_are_string_backed() {
        let metadata = metadata(FrameKind::DemotedMessage, None);
        let policy = RetentionPolicy::new().keep_label("legal_hold");
        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&metadata).with_retention_label("legal_hold")),
            RetentionDecision::Keep
        );
        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&metadata).with_retention_label("ephemeral")),
            RetentionDecision::Defer
        );
    }
}
