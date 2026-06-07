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
    /// Bi-temporal *valid-time* start in Unix milliseconds: when the fact this
    /// candidate represents became true in the modelled world. Distinct from
    /// [`written_at_unix_ms`](Self::written_at_unix_ms), which is
    /// transaction-time (when the backend recorded it).
    pub valid_from_unix_ms: Option<u64>,
    /// Bi-temporal *valid-time* end in Unix milliseconds: when the fact stopped
    /// being true. `None` means still valid (open interval).
    pub valid_to_unix_ms: Option<u64>,
    /// Identifier of an entry this candidate supersedes, if any. Backends use
    /// this to invalidate the older fact rather than hard-deleting it.
    pub supersedes: Option<&'a str>,
    /// Host-supplied importance weight in `[0, 1]` used by salience scoring.
    /// Absent importance is treated as `1.0` (maximally important).
    pub importance: Option<f32>,
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
            valid_from_unix_ms: None,
            valid_to_unix_ms: None,
            supersedes: None,
            importance: None,
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

    /// Attach the bi-temporal valid-time start in Unix milliseconds.
    #[must_use]
    pub fn with_valid_from_unix_ms(mut self, valid_from_unix_ms: u64) -> Self {
        self.valid_from_unix_ms = Some(valid_from_unix_ms);
        self
    }

    /// Attach the bi-temporal valid-time end in Unix milliseconds. A candidate
    /// with a `valid_to` in the past is considered expired.
    #[must_use]
    pub fn with_valid_to_unix_ms(mut self, valid_to_unix_ms: u64) -> Self {
        self.valid_to_unix_ms = Some(valid_to_unix_ms);
        self
    }

    /// Mark this candidate as superseding a prior entry by identifier.
    #[must_use]
    pub fn with_supersedes(mut self, supersedes: &'a str) -> Self {
        self.supersedes = Some(supersedes);
        self
    }

    /// Attach a host-supplied importance weight, clamped to `[0, 1]`.
    #[must_use]
    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance = Some(importance.clamp(0.0, 1.0));
        self
    }

    /// Recency-weighted salience score at `as_of_unix_ms`.
    ///
    /// Combines importance with an exponential recency decay
    /// (`importance * 0.5^(age / half_life)`) using the candidate's
    /// valid-time start when present, otherwise its write timestamp. A
    /// candidate with no usable timestamp, or a zero/unset half-life, scores
    /// purely on importance. Absent importance defaults to `1.0`.
    #[must_use]
    pub fn salience(&self, as_of_unix_ms: u64, half_life_ms: u64) -> f64 {
        let importance = f64::from(self.importance.unwrap_or(1.0));
        let reference = self.valid_from_unix_ms.or(self.written_at_unix_ms);
        let Some(reference) = reference else {
            return importance;
        };
        if half_life_ms == 0 {
            return importance;
        }
        let age = as_of_unix_ms.saturating_sub(reference);
        let decay = 0.5_f64.powf(age as f64 / half_life_ms as f64);
        importance * decay
    }
}

/// Result of evaluating one candidate against a retention policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionDecision {
    /// The backend should retain the candidate.
    Keep,
    /// The backend may delete, compact away, or archive the candidate.
    Drop,
    /// The candidate is stale (expired or superseded) and should be marked
    /// invalid in place rather than hard-deleted. Append-only backends apply
    /// this by writing a tombstone or clearing the valid-time interval; they
    /// must not lose the historical record.
    Invalidate,
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
    /// Keep the N most recent candidates, determined by sorting remaining
    /// candidates descending by sequence (and then write timestamp).
    KeepMostRecent {
        /// Maximum number of candidates to keep in this rule.
        count: usize,
    },
    /// Invalidate candidates whose bi-temporal valid-time has ended at or
    /// before `as_of_unix_ms` (i.e. `valid_to <= as_of`). Candidates with an
    /// open interval (`valid_to == None`) are unaffected.
    InvalidateExpired {
        /// Evaluation instant in Unix milliseconds, supplied by the caller to
        /// keep the rule wall-clock-free and deterministic.
        as_of_unix_ms: u64,
    },
    /// Invalidate candidates that have been superseded by a newer entry, i.e.
    /// those carrying a [`RetentionCandidate::supersedes`] reference.
    InvalidateSuperseded,
    /// Keep the N highest-salience candidates, ranking by recency-weighted
    /// importance (see [`RetentionCandidate::salience`]).
    KeepTopBySalience {
        /// Maximum number of candidates to keep in this rule.
        count: usize,
        /// Recency half-life in Unix milliseconds used by the decay term.
        half_life_ms: u64,
        /// Evaluation instant in Unix milliseconds.
        as_of_unix_ms: u64,
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
            Self::KeepMostRecent { .. } => RetentionDecision::Defer,
            Self::KeepTopBySalience { .. } => RetentionDecision::Defer,
            Self::InvalidateExpired { as_of_unix_ms } => candidate
                .valid_to_unix_ms
                .map(|valid_to| {
                    if valid_to <= *as_of_unix_ms {
                        RetentionDecision::Invalidate
                    } else {
                        RetentionDecision::Defer
                    }
                })
                .unwrap_or(RetentionDecision::Defer),
            Self::InvalidateSuperseded if candidate.supersedes.is_some() => {
                RetentionDecision::Invalidate
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

/// The partitioned result of evaluating a batch of candidates.
#[derive(Debug, Clone, Default)]
pub struct RetentionReport<'a> {
    /// Candidates the backend should retain.
    pub keep: Vec<RetentionCandidate<'a>>,
    /// Candidates the backend may delete, compact, or archive.
    pub drop: Vec<RetentionCandidate<'a>>,
    /// Candidates the backend should mark invalid in place (expired or
    /// superseded) without losing the historical record.
    pub invalidate: Vec<RetentionCandidate<'a>>,
    /// Candidates where the policy had no opinion.
    pub defer: Vec<RetentionCandidate<'a>>,
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

    /// Keep the `count` most recent candidates.
    ///
    /// This rule requires batch evaluation via [`RetentionPolicy::evaluate_batch`].
    /// If evaluated individually via [`RetentionPolicy::evaluate`], it defers.
    #[must_use]
    pub fn keep_most_recent(self, count: usize) -> Self {
        self.rule(RetentionRule::KeepMostRecent { count })
    }

    /// Keep the `count` highest-salience candidates, decaying recency by
    /// `half_life_ms` and evaluated at `as_of_unix_ms`.
    ///
    /// This rule requires batch evaluation via [`RetentionPolicy::evaluate_batch`].
    /// If evaluated individually via [`RetentionPolicy::evaluate`], it defers.
    #[must_use]
    pub fn keep_top_by_salience(self, count: usize, half_life_ms: u64, as_of_unix_ms: u64) -> Self {
        self.rule(RetentionRule::KeepTopBySalience {
            count,
            half_life_ms,
            as_of_unix_ms,
        })
    }

    /// Invalidate candidates whose valid-time ended at or before
    /// `as_of_unix_ms`.
    #[must_use]
    pub fn invalidate_expired(self, as_of_unix_ms: u64) -> Self {
        self.rule(RetentionRule::InvalidateExpired { as_of_unix_ms })
    }

    /// Invalidate candidates that supersede an older entry.
    #[must_use]
    pub fn invalidate_superseded(self) -> Self {
        self.rule(RetentionRule::InvalidateSuperseded)
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

    /// Evaluate a batch of candidates against this policy.
    ///
    /// Candidates are processed through the policy rules iteratively.
    /// Stateful rules like `KeepMostRecent` sort and filter the remaining
    /// candidate pool efficiently.
    #[must_use]
    pub fn evaluate_batch<'a>(
        &self,
        candidates: impl IntoIterator<Item = RetentionCandidate<'a>>,
    ) -> RetentionReport<'a> {
        let mut report = RetentionReport::default();
        let mut current_batch: Vec<RetentionCandidate<'a>> = candidates.into_iter().collect();

        for rule in &self.rules {
            if current_batch.is_empty() {
                break;
            }

            match rule {
                RetentionRule::KeepMostRecent { count } => {
                    current_batch.sort_by(|a, b| {
                        b.sequence
                            .cmp(&a.sequence)
                            .then_with(|| b.written_at_unix_ms.cmp(&a.written_at_unix_ms))
                    });

                    let mut next_batch = Vec::with_capacity(current_batch.len());
                    let mut kept = 0;
                    for candidate in current_batch {
                        if kept < *count {
                            report.keep.push(candidate);
                            kept += 1;
                        } else {
                            next_batch.push(candidate);
                        }
                    }
                    current_batch = next_batch;
                }
                RetentionRule::KeepTopBySalience {
                    count,
                    half_life_ms,
                    as_of_unix_ms,
                } => {
                    current_batch.sort_by(|a, b| {
                        b.salience(*as_of_unix_ms, *half_life_ms)
                            .total_cmp(&a.salience(*as_of_unix_ms, *half_life_ms))
                    });

                    let mut next_batch = Vec::with_capacity(current_batch.len());
                    let mut kept = 0;
                    for candidate in current_batch {
                        if kept < *count {
                            report.keep.push(candidate);
                            kept += 1;
                        } else {
                            next_batch.push(candidate);
                        }
                    }
                    current_batch = next_batch;
                }
                _ => {
                    let mut next_batch = Vec::with_capacity(current_batch.len());
                    for candidate in current_batch {
                        match rule.evaluate(candidate) {
                            RetentionDecision::Keep => report.keep.push(candidate),
                            RetentionDecision::Drop => report.drop.push(candidate),
                            RetentionDecision::Invalidate => report.invalidate.push(candidate),
                            RetentionDecision::Defer => next_batch.push(candidate),
                        }
                    }
                    current_batch = next_batch;
                }
            }
        }

        for candidate in current_batch {
            match self.default_decision {
                RetentionDecision::Keep => report.keep.push(candidate),
                RetentionDecision::Drop => report.drop.push(candidate),
                RetentionDecision::Invalidate => report.invalidate.push(candidate),
                RetentionDecision::Defer => report.defer.push(candidate),
            }
        }

        report
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

    #[test]
    fn evaluate_batch_processes_keep_most_recent() {
        let m1 = metadata(FrameKind::DemotedMessage, None);
        let m2 = metadata(FrameKind::DemotedMessage, None);
        let m3 = metadata(FrameKind::DemotedMessage, None);

        let c1 = RetentionCandidate::new(&m1).with_sequence(1);
        let c2 = RetentionCandidate::new(&m2).with_sequence(3);
        let c3 = RetentionCandidate::new(&m3).with_sequence(2);

        let policy = RetentionPolicy::new()
            .keep_most_recent(2)
            .default_decision(RetentionDecision::Drop);

        let report = policy.evaluate_batch(vec![c1, c2, c3]);
        assert_eq!(report.keep.len(), 2);
        assert_eq!(report.drop.len(), 1);
        assert_eq!(report.defer.len(), 0);

        let kept_seqs: std::collections::HashSet<_> =
            report.keep.iter().map(|c| c.sequence.unwrap()).collect();
        assert!(kept_seqs.contains(&3));
        assert!(kept_seqs.contains(&2));
    }

    #[test]
    fn invalidate_expired_marks_candidates_whose_valid_time_ended() {
        let metadata = metadata(FrameKind::DemotedMessage, None);
        let policy = RetentionPolicy::new().invalidate_expired(1_000);

        // valid_to in the past -> Invalidate.
        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&metadata).with_valid_to_unix_ms(999)),
            RetentionDecision::Invalidate
        );
        // valid_to exactly at as_of -> Invalidate (inclusive).
        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&metadata).with_valid_to_unix_ms(1_000)),
            RetentionDecision::Invalidate
        );
        // valid_to in the future -> Defer.
        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&metadata).with_valid_to_unix_ms(1_001)),
            RetentionDecision::Defer
        );
        // open interval (no valid_to) -> Defer.
        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&metadata)),
            RetentionDecision::Defer
        );
    }

    #[test]
    fn invalidate_superseded_marks_candidates_with_a_supersedes_ref() {
        let metadata = metadata(FrameKind::CompactionSummary, None);
        let policy = RetentionPolicy::new().invalidate_superseded();

        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&metadata).with_supersedes("ep-older")),
            RetentionDecision::Invalidate
        );
        assert_eq!(
            policy.evaluate(RetentionCandidate::new(&metadata)),
            RetentionDecision::Defer
        );
    }

    #[test]
    fn evaluate_batch_routes_invalidations_into_their_own_bucket() {
        let fresh = metadata(FrameKind::DemotedMessage, None);
        let expired = metadata(FrameKind::DemotedMessage, None);
        let policy = RetentionPolicy::new()
            .invalidate_expired(1_000)
            .default_decision(RetentionDecision::Keep);

        let report = policy.evaluate_batch(vec![
            RetentionCandidate::new(&fresh).with_valid_to_unix_ms(2_000),
            RetentionCandidate::new(&expired).with_valid_to_unix_ms(500),
        ]);
        assert_eq!(report.keep.len(), 1);
        assert_eq!(report.invalidate.len(), 1);
        assert_eq!(report.drop.len(), 0);
        assert_eq!(report.defer.len(), 0);
    }

    #[test]
    fn salience_decays_with_age_and_scales_with_importance() {
        let metadata = metadata(FrameKind::DemotedMessage, None);
        let as_of = 10_000;
        let half_life = 1_000;

        // Same importance, older candidate scores lower.
        let recent = RetentionCandidate::new(&metadata)
            .with_written_at_unix_ms(9_000)
            .with_importance(1.0);
        let old = RetentionCandidate::new(&metadata)
            .with_written_at_unix_ms(5_000)
            .with_importance(1.0);
        assert!(recent.salience(as_of, half_life) > old.salience(as_of, half_life));

        // One half-life of age halves the score.
        let one_half_life = RetentionCandidate::new(&metadata)
            .with_written_at_unix_ms(9_000)
            .with_importance(1.0);
        assert!((one_half_life.salience(as_of, half_life) - 0.5).abs() < 1e-9);

        // Zero half-life falls back to pure importance.
        let weighted = RetentionCandidate::new(&metadata)
            .with_written_at_unix_ms(0)
            .with_importance(0.25);
        assert!((weighted.salience(as_of, 0) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn keep_top_by_salience_prefers_recent_important_candidates() {
        let m_old_important = metadata(FrameKind::DemotedMessage, None);
        let m_recent_low = metadata(FrameKind::DemotedMessage, None);
        let m_recent_high = metadata(FrameKind::DemotedMessage, None);
        let as_of = 100_000;
        let half_life = 10_000;

        // Old + important: importance 1.0 but 50_000ms old (5 half-lives).
        let old_important = RetentionCandidate::new(&m_old_important)
            .with_written_at_unix_ms(50_000)
            .with_importance(1.0);
        // Recent + low importance.
        let recent_low = RetentionCandidate::new(&m_recent_low)
            .with_written_at_unix_ms(99_000)
            .with_importance(0.2);
        // Recent + high importance: should always win.
        let recent_high = RetentionCandidate::new(&m_recent_high)
            .with_written_at_unix_ms(99_000)
            .with_importance(0.9);

        let policy = RetentionPolicy::new()
            .keep_top_by_salience(2, half_life, as_of)
            .default_decision(RetentionDecision::Drop);

        let report = policy.evaluate_batch(vec![old_important, recent_low, recent_high]);
        assert_eq!(report.keep.len(), 2);
        assert_eq!(report.drop.len(), 1);

        // recent_high must be kept; the single dropped item is old_important
        // (5 half-lives of decay sinks it below the recent pair).
        let kept_importances: Vec<f32> = report.keep.iter().filter_map(|c| c.importance).collect();
        assert!(kept_importances.contains(&0.9));
        assert!(kept_importances.contains(&0.2));
        assert_eq!(report.drop.first().and_then(|c| c.importance), Some(1.0));
    }
}
