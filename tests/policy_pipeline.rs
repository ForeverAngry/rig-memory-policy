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

#[test]
fn sota_memory_pipeline_integration() {
    use rig_memory_policy::fusion::{RRF_DEFAULT_K, reciprocal_rank_fusion};
    use rig_memory_policy::reconcile::{DuplicateResolver, ExactHashResolver, ReconcileOp};

    // 1. Reconciliation (Mem0-style)
    // Check if new incoming facts are novel, or exact duplicates.
    let mut resolver = ExactHashResolver::new(["user prefers python", "uses rig library"]);
    assert_eq!(resolver.reconcile("user prefers python"), ReconcileOp::Noop);
    assert_eq!(resolver.reconcile("wants to learn rust"), ReconcileOp::Add);
    resolver.observe("wants to learn rust");

    // 2. Retention (Bi-temporal & Salience - Zep & Generative Agents)
    let meta_a = FrameMetadata {
        schema_version: 1,
        kind: FrameKind::DemotedMessage,
        conversation_id: "c1".into(),
        chat_role: "user".into(),
        dedup_key: "k1".into(),
        scope: None,
    };
    let meta_b = FrameMetadata {
        dedup_key: "k2".into(),
        ..meta_a.clone()
    };
    let meta_c = FrameMetadata {
        dedup_key: "k3".into(),
        ..meta_a.clone()
    };
    let meta_d = FrameMetadata {
        dedup_key: "k4".into(),
        ..meta_a.clone()
    };

    let as_of = 100_000;

    // Candidate A: Expired valid-time
    let cand_a = RetentionCandidate::new(&meta_a)
        .with_valid_to_unix_ms(90_000)
        .with_importance(0.9);

    // Candidate B: Superseded by another fact
    let cand_b = RetentionCandidate::new(&meta_b)
        .with_supersedes("some_older_id")
        .with_importance(0.8);

    // Candidate C: Active, high importance
    let cand_c = RetentionCandidate::new(&meta_c)
        .with_valid_from_unix_ms(80_000)
        .with_importance(1.0);

    // Candidate D: Active, but older and lower importance
    let cand_d = RetentionCandidate::new(&meta_d)
        .with_valid_from_unix_ms(10_000)
        .with_importance(0.5);

    let policy = RetentionPolicy::new()
        .invalidate_expired(as_of) // Drops Cand A into `invalidate`
        .invalidate_superseded() // Drops Cand B into `invalidate` due to `supersedes`
        .keep_top_by_salience(1, 10_000, as_of) // Keeps only Cand C, defers Cand D
        .default_decision(RetentionDecision::Drop);

    let report = policy.evaluate_batch(vec![cand_a, cand_b, cand_c, cand_d]);

    // Validate the batch routing
    assert_eq!(report.invalidate.len(), 2, "A and B should be invalidated");
    let inv_keys: Vec<_> = report
        .invalidate
        .iter()
        .map(|c| c.metadata.dedup_key.as_str())
        .collect();
    assert!(inv_keys.contains(&"k1"));
    assert!(inv_keys.contains(&"k2"));

    assert_eq!(report.keep.len(), 1, "C should be kept based on salience");
    assert_eq!(report.keep[0].metadata.dedup_key, "k3");

    assert_eq!(
        report.drop.len(),
        1,
        "D should fall through to default Drop"
    );
    assert_eq!(report.drop[0].metadata.dedup_key, "k4");

    // 3. Fusion (Hybrid Retrieval)
    let dense_hits = ["doc1", "doc2", "doc3"];
    let sparse_hits = ["doc3", "doc1", "doc4"];
    let fused = reciprocal_rank_fusion(
        [dense_hits.as_slice(), sparse_hits.as_slice()],
        RRF_DEFAULT_K,
    );

    // "doc1" is rank 0 in dense (1/60) and rank 1 in sparse (1/61)
    // "doc3" is rank 2 in dense (1/62) and rank 0 in sparse (1/60)
    // doc1 > doc3 > doc2 > doc4
    assert_eq!(fused[0].0, "doc1");
    assert_eq!(fused[1].0, "doc3");
    assert_eq!(fused.len(), 4);
}

#[cfg(feature = "near-dedup")]
#[test]
fn sota_simhash_pipeline() {
    use rig_memory_policy::dedup::SimHash;

    // Verifying text paraphrase detection natively
    let hash1 = SimHash::from_text("user wants to learn rust programming");
    let hash2 = SimHash::from_text("rust programming learn wants to user");

    assert!(
        hash1.is_near(&hash2, 8),
        "Paraphrases should collide under SimHash threshold"
    );

    let hash3 = SimHash::from_text("completely different topic about deployment");
    assert!(
        !hash1.is_near(&hash3, 8),
        "Different topics shouldn't collide"
    );
}
