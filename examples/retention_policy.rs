use rig_memory_policy::dedup::{DedupSet, compute_key, hex_encode_key};
use rig_memory_policy::{
    FrameKind, FrameMetadata, RetentionCandidate, RetentionDecision, RetentionPolicy, Scope,
};
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("retention example failed: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool, rig_memory_policy::PolicyError> {
    let scope = Scope::new("tenant-a/support");
    let text = "support summary for tenant-a";
    let dedup_key = compute_key(
        FrameKind::CompactionSummary.as_str(),
        "conv-1",
        "assistant",
        Some(scope.as_str()),
        text,
    );

    let seen = DedupSet::new();
    if seen.contains(&dedup_key)? {
        return Ok(true);
    }
    seen.insert(dedup_key)?;

    let metadata = FrameMetadata {
        schema_version: 1,
        kind: FrameKind::CompactionSummary,
        conversation_id: "conv-1".into(),
        chat_role: "assistant".into(),
        dedup_key: hex_encode_key(&dedup_key),
        scope: Some(scope.as_str().into()),
    };

    let policy = RetentionPolicy::new()
        .drop_outside_scope(Some("tenant-a/support"))
        .keep_summaries()
        .default_decision(RetentionDecision::Drop);

    let decision = policy.evaluate(RetentionCandidate::new(&metadata).with_sequence(7));
    Ok(decision == RetentionDecision::Keep)
}
