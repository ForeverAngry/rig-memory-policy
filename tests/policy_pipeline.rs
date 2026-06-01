#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use rig_memory_policy::dedup::{DedupSet, compute_key, hex_encode_key};
use rig_memory_policy::{
    Episode, FrameKind, FrameMetadata, InMemoryStore, RetentionCandidate, RetentionDecision,
    RetentionPolicy, Scope,
};

#[derive(Clone, Debug)]
struct TestEpisode {
    summary: String,
}

impl Episode for TestEpisode {
    fn summary(&self) -> &str {
        &self.summary
    }
}

#[test]
fn dedup_metadata_scope_retention_pipeline_behaves_like_external_consumer() {
    let scope = Scope::new("/tenant-a/project-1/");
    let text = "assistant summarized the tenant-a outage timeline";
    let key = compute_key(
        FrameKind::CompactionSummary.as_str(),
        "conv-1",
        "assistant",
        Some(scope.as_str()),
        text,
    );

    let dedup = DedupSet::new();
    assert!(!dedup.contains(&key).unwrap());
    dedup.insert(key).unwrap();
    assert!(dedup.contains(&key).unwrap());

    let metadata = FrameMetadata {
        schema_version: 1,
        kind: FrameKind::CompactionSummary,
        conversation_id: "conv-1".into(),
        chat_role: "assistant".into(),
        dedup_key: hex_encode_key(&key),
        scope: Some(scope.as_str().into()),
    };

    let map = metadata.clone().into_map();
    let decoded = FrameMetadata::try_from_map(&map).unwrap();
    assert_eq!(decoded, metadata);

    let policy = RetentionPolicy::new()
        .drop_outside_scope(Some("tenant-a/project-1"))
        .keep_summaries()
        .default_decision(RetentionDecision::Drop);
    let decision = policy.evaluate(RetentionCandidate::new(&decoded).with_sequence(42));
    assert_eq!(decision, RetentionDecision::Keep);

    let store = InMemoryStore::<TestEpisode>::new();
    let stored_key = pollster::block_on(store.append(TestEpisode {
        summary: text.into(),
    }))
    .unwrap();
    let hits = pollster::block_on(store.retrieve_similar("outage timeline", 5)).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].key, stored_key);
    assert!(hits[0].score > 0.0);
}
