//! Backend-neutral helpers for matching and projecting memory scopes.
//!
//! Scopes are deliberately represented as normalized strings. Backends may
//! store them in URI fields, metadata maps, tags, or sidecar indexes, but this
//! crate only needs the policy-level question: does a frame belong to a scope,
//! and what hierarchical path can a caller expose for provenance?

/// A normalized memory scope used for isolation and retention decisions.
///
/// The value is slash-delimited text. Empty path segments are removed and
/// leading/trailing slashes are ignored, so `"/tenant/a/"` and `"tenant/a"`
/// normalize to the same scope.
///
/// # Example
///
/// ```
/// use rig_memory_policy::Scope;
///
/// let scope = Scope::new("/tenant-a/project-1/");
/// assert_eq!(scope.as_str(), "tenant-a/project-1");
/// assert_eq!(scope.path(), vec!["tenant-a", "project-1"]);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Scope(String);

impl Scope {
    /// Normalize `value` into a scope.
    #[must_use]
    pub fn new(value: impl AsRef<str>) -> Self {
        Self(normalize_scope(value.as_ref()))
    }

    /// Return the normalized scope string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return `true` when this scope has no path segments.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Split the scope into normalized path segments.
    #[must_use]
    pub fn path(&self) -> Vec<&str> {
        scope_path(&self.0)
    }

    /// Return `true` when `candidate` is exactly this scope after
    /// normalization.
    #[must_use]
    pub fn matches(&self, candidate: Option<&str>) -> bool {
        candidate
            .map(|candidate| normalize_scope(candidate) == self.0)
            .unwrap_or(false)
    }

    /// Return `true` when `candidate` is this scope or a descendant scope.
    ///
    /// For example, `tenant-a` contains `tenant-a/project-1`, but does not
    /// contain `tenant-ab`.
    #[must_use]
    pub fn contains(&self, candidate: Option<&str>) -> bool {
        let Some(candidate) = candidate else {
            return false;
        };
        let candidate = normalize_scope(candidate);
        if self.0.is_empty() {
            return candidate.is_empty();
        }
        candidate == self.0
            || candidate
                .strip_prefix(self.0.as_str())
                .map(|suffix| suffix.starts_with('/'))
                .unwrap_or(false)
    }
}

impl From<&str> for Scope {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for Scope {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Normalize a slash-delimited scope string.
#[must_use]
pub fn normalize_scope(scope: &str) -> String {
    scope
        .split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// Split a slash-delimited scope string into normalized path segments.
#[must_use]
pub fn scope_path(scope: &str) -> Vec<&str> {
    scope
        .split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// Match an optional candidate scope against an optional required scope.
///
/// `None` requires an unscoped candidate. `Some(scope)` requires exact scope
/// equality after normalization.
#[must_use]
pub fn scope_matches(required: Option<&str>, candidate: Option<&str>) -> bool {
    match required {
        Some(required) => Scope::new(required).matches(candidate),
        None => candidate
            .map(|candidate| normalize_scope(candidate).is_empty())
            .unwrap_or(true),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_scope_segments() {
        let scope = Scope::new(" /tenant-a//project-1/ ");
        assert_eq!(scope.as_str(), "tenant-a/project-1");
        assert_eq!(scope.path(), vec!["tenant-a", "project-1"]);
    }

    #[test]
    fn exact_match_uses_normalized_values() {
        let scope = Scope::new("tenant-a/project-1");
        assert!(scope.matches(Some("/tenant-a/project-1/")));
        assert!(!scope.matches(Some("tenant-a/project-2")));
        assert!(!scope.matches(None));
    }

    #[test]
    fn contains_accepts_descendants_only_on_segment_boundaries() {
        let scope = Scope::new("tenant-a");
        assert!(scope.contains(Some("tenant-a")));
        assert!(scope.contains(Some("tenant-a/project-1")));
        assert!(!scope.contains(Some("tenant-ab/project-1")));
    }

    #[test]
    fn optional_scope_matching_treats_none_as_unscoped() {
        assert!(scope_matches(None, None));
        assert!(scope_matches(None, Some("/")));
        assert!(!scope_matches(None, Some("tenant-a")));
        assert!(scope_matches(Some("tenant-a"), Some("/tenant-a/")));
    }
}
