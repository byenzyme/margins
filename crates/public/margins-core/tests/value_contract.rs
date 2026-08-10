use margins_core::{
    AudioLane, CaptureAction, CaptureCommand, CaptureErrorCode, CaptureOperationId,
    CaptureProvider, SegmentId, SessionLifecycle, TranscriptWord, UnavailableCaptureProvider,
};

#[test]
fn unavailable_provider_has_typed_stable_behavior() {
    let provider = UnavailableCaptureProvider::default();
    let capabilities = provider.capabilities();
    assert!(!capabilities.available);
    assert!(capabilities.supported_lanes.is_empty());

    let error = provider.permission(AudioLane::Microphone).unwrap_err();
    assert_eq!(error.code, CaptureErrorCode::Unavailable);
    assert_eq!(serde_json::to_value(error.code).unwrap(), "unavailable");
}

#[test]
fn session_lifecycle_rejects_resurrection_after_tombstone() {
    assert!(SessionLifecycle::Active.can_transition_to(SessionLifecycle::Paused));
    assert!(SessionLifecycle::Paused.can_transition_to(SessionLifecycle::Active));
    assert!(!SessionLifecycle::Tombstoned.can_transition_to(SessionLifecycle::Active));
}

#[test]
fn protocol_ids_are_the_same_types_as_core_ids() {
    let core_id: margins_core::SessionId = "session-1".into();
    let wire_id: margins_core::wire::SessionId = core_id;
    assert_eq!(wire_id.as_ref(), "session-1");
}

#[test]
fn transcript_confidence_uses_the_hardened_per_mille_name() {
    let word = TranscriptWord {
        start_ms: 1,
        end_ms: 2,
        text: "hello".to_owned(),
        speaker: None,
        confidence_per_mille: Some(975),
    };
    let json = serde_json::to_value(word).unwrap();
    assert_eq!(json["confidence_per_mille"], 975);
    assert!(json.get("confidence_millis").is_none());
}

#[test]
fn capture_commands_carry_a_stale_segment_guard() {
    let command = CaptureCommand {
        operation_id: CaptureOperationId::new("operation-2"),
        expected_segment_id: SegmentId::from("segment-1"),
        action: CaptureAction::Pause,
    };
    let json = serde_json::to_value(command).unwrap();
    assert_eq!(json["expected_segment_id"], "segment-1");
}
