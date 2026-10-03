use margins_meeting_protocol::{
    WorkspaceArtifactV1, WorkspaceMemoV1, WorkspaceNoteAssociationV1, WorkspaceProcessingJobV1,
    WorkspaceSessionPageV1, WorkspaceSessionSummaryV1, WorkspaceTranscriptV1,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

fn assert_exact_wire_shape<T: DeserializeOwned + Serialize>(fixture: &Value, key: &str) {
    let value = fixture.get(key).unwrap_or_else(|| panic!("missing {key} fixture"));
    let parsed: T = serde_json::from_value(value.clone())
        .unwrap_or_else(|error| panic!("{key} no longer deserializes as the Rust protocol type: {error}"));
    assert_eq!(
        serde_json::to_value(parsed).expect("serialize protocol type"),
        *value,
        "{key} differs from the complete Rust wire shape"
    );
}

#[test]
fn plugin_workspace_response_fixture_matches_rust_protocol() {
    let fixture: Value = serde_json::from_str(include_str!("../contracts/workspace-responses.v1.json"))
        .expect("valid Workspace response fixture");
    assert_exact_wire_shape::<WorkspaceSessionSummaryV1>(&fixture, "session_summary");
    assert_exact_wire_shape::<WorkspaceSessionPageV1>(&fixture, "session_page");
    assert_exact_wire_shape::<WorkspaceTranscriptV1>(&fixture, "transcript");
    assert_exact_wire_shape::<Vec<WorkspaceArtifactV1>>(&fixture, "artifacts");
    assert_exact_wire_shape::<WorkspaceMemoV1>(&fixture, "memo");
    assert_exact_wire_shape::<WorkspaceNoteAssociationV1>(&fixture, "note_association");
    assert_exact_wire_shape::<Option<WorkspaceNoteAssociationV1>>(&fixture, "note_association_missing");
    assert_exact_wire_shape::<WorkspaceProcessingJobV1>(&fixture, "processing_job");
    assert_eq!(fixture["session_page"]["sessions"][0], fixture["session_summary"]);
}
