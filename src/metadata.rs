//! Typed metadata envelope for entries written by memory-lifecycle hooks.
//!
//! Backends stamp a [`FrameMetadata`] envelope into their per-entry metadata
//! map (e.g. `memvid_core::SearchHitMetadata::extra_metadata`, or any
//! equivalent key-value bag on another backend) so downstream tools — evals,
//! memory inspectors, RAG pipelines — can decode the origin of each retrieved
//! entry uniformly regardless of the storage engine.

use serde::{Deserialize, Serialize};

/// The type of entry produced by a memory lifecycle hook.
///
/// Backends pattern-match on this enum across crate boundaries (e.g. to
/// classify retrieved entries by origin), so the enum is **not**
/// `#[non_exhaustive]`. Any future variant addition is therefore a breaking
/// change and must bump the major version of this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    /// A single user/assistant interaction evicted from the working set.
    DemotedMessage,
    /// A rolled-up summary of multiple evicted interactions.
    CompactionSummary,
}

/// The typed schema written into a backend's per-entry metadata map for every
/// entry produced by a Rig memory lifecycle hook.
///
/// Downstream tools can decode the metadata map into this struct to reason
/// about the origin of the retrieved entry.
///
/// Backends construct this envelope directly (struct literal); to keep that
/// usage ergonomic across crate boundaries the struct is **not**
/// `#[non_exhaustive]`. Any future field addition is therefore a breaking
/// change and must bump the major version of this crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameMetadata {
    /// The schema version of this envelope. Currently `1`.
    pub schema_version: u8,
    /// What kind of memory lifecycle produced this entry.
    pub kind: FrameKind,
    /// The conversation ID from the Rig `CompactingMemory` context.
    pub conversation_id: String,
    /// The logical chat role (`system`, `user`, `assistant`).
    pub chat_role: String,
    /// The deterministic content-hash dedup key.
    pub dedup_key: String,
    /// The isolation scope under which this entry was written, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

impl FrameMetadata {
    /// Attempt to parse the metadata envelope from a string-to-string
    /// dictionary.
    pub fn try_from_map(
        map: &std::collections::BTreeMap<String, String>,
    ) -> Result<Self, serde_json::Error> {
        let mut obj = serde_json::Map::new();
        #[allow(clippy::collapsible_if)]
        for (k, v) in map {
            if let Ok(num) = v.parse::<u8>() {
                if k == "schema_version" {
                    obj.insert(k.clone(), serde_json::Value::Number(num.into()));
                    continue;
                }
            }
            obj.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        serde_json::from_value(serde_json::Value::Object(obj))
    }

    /// Convert the metadata envelope into a string-to-string dictionary.
    pub fn into_map(self) -> std::collections::BTreeMap<String, String> {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "schema_version".to_string(),
            self.schema_version.to_string(),
        );
        map.insert("kind".to_string(), self.kind.as_str().to_string());
        map.insert("conversation_id".to_string(), self.conversation_id);
        map.insert("chat_role".to_string(), self.chat_role);
        map.insert("dedup_key".to_string(), self.dedup_key);
        if let Some(scope) = self.scope {
            map.insert("scope".to_string(), scope);
        }
        map
    }
}

impl FrameKind {
    /// Provide the snake_case string representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DemotedMessage => "demoted_message",
            Self::CompactionSummary => "compaction_summary",
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_map() {
        let original = FrameMetadata {
            schema_version: 1,
            kind: FrameKind::DemotedMessage,
            conversation_id: "conv-1".to_string(),
            chat_role: "user".to_string(),
            dedup_key: "deadbeef".to_string(),
            scope: Some("scope-a".to_string()),
        };
        let map = original.clone().into_map();
        let parsed = FrameMetadata::try_from_map(&map).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn scope_round_trips_when_absent() {
        let original = FrameMetadata {
            schema_version: 1,
            kind: FrameKind::CompactionSummary,
            conversation_id: "conv-2".to_string(),
            chat_role: "assistant".to_string(),
            dedup_key: "cafebabe".to_string(),
            scope: None,
        };
        let map = original.clone().into_map();
        assert!(!map.contains_key("scope"));
        let parsed = FrameMetadata::try_from_map(&map).unwrap();
        assert_eq!(parsed, original);
    }
}
