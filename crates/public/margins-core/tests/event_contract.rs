use margins_core::{
    Event, EventEnvelope, EventErrorCode, EventSequence, SegmentId, SessionId, EVENT_SCHEMA,
    EVENT_VERSION_V1,
};

const GOLDEN: &str = include_str!("fixtures/event_v1.json");

#[test]
fn v1_event_golden_round_trips_byte_for_byte() {
    let decoded: EventEnvelope = serde_json::from_str(GOLDEN).unwrap();
    assert_eq!(decoded.schema, EVENT_SCHEMA);
    assert_eq!(decoded.version, EVENT_VERSION_V1);
    assert_eq!(decoded.kind.0, "capture.lane_state_changed");
    assert_eq!(decoded.payload["future_detail"]["x"], 1);
    assert_eq!(decoded.extensions["future_top"], "retained");
    decoded.validate_v1().unwrap();

    let encoded = serde_json::to_string(&decoded).unwrap();
    assert_eq!(encoded, GOLDEN.trim_end());
}

#[test]
fn unknown_kind_and_payload_fields_are_preserved() {
    let raw = r#"{"schema":"margins.event","version":1,"sequence":7,"emitted_at_ms":8,"session_id":"s","segment_id":null,"operation_id":null,"kind":"future.kind","payload":{"unknown":true}}"#;
    let decoded: EventEnvelope = serde_json::from_str(raw).unwrap();
    assert_eq!(decoded.kind.0, "future.kind");
    assert_eq!(decoded.payload["unknown"], true);
    assert_eq!(serde_json::to_string(&decoded).unwrap(), raw);
}

#[test]
fn required_envelope_fields_cannot_be_omitted() {
    let missing_payload = r#"{"schema":"margins.event","version":1,"sequence":1,"emitted_at_ms":2,"session_id":"s","segment_id":null,"operation_id":null,"kind":"capture.started"}"#;
    assert!(serde_json::from_str::<EventEnvelope>(missing_payload).is_err());
}

#[test]
fn typed_event_conversion_keeps_kind_out_of_payload() {
    let envelope = EventEnvelope::from_event_v1(
        EventSequence(1),
        2,
        SessionId::from("session"),
        Some(SegmentId::from("segment")),
        None,
        &Event::CaptureStarted,
    )
    .unwrap();
    assert_eq!(envelope.kind.0, "capture.started");
    assert_eq!(envelope.payload, serde_json::json!({}));
}

#[test]
fn v1_validation_uses_the_hardened_protocol_json_integer_limit() {
    let mut envelope = EventEnvelope::from_event_v1(
        EventSequence(margins_core::wire::MAX_SAFE_JSON_INTEGER),
        2,
        SessionId::from("session"),
        Some(SegmentId::from("segment")),
        None,
        &Event::CaptureStarted,
    )
    .unwrap();
    envelope.validate_v1().unwrap();

    envelope.sequence.0 += 1;
    let error = envelope.validate_v1().unwrap_err();
    assert_eq!(error.code, EventErrorCode::InvalidEnvelope);
}

#[test]
fn v1_validation_rejects_ambiguous_or_malformed_envelopes() {
    let mut envelope = EventEnvelope::v1(
        EventSequence(1),
        2,
        SessionId::from("session"),
        "capture.started",
        serde_json::json!({}),
    );
    assert_eq!(
        envelope.validate_v1().unwrap_err().code,
        EventErrorCode::InvalidEnvelope
    );

    envelope.segment_id = Some(SegmentId::from("segment"));
    envelope.payload = serde_json::json!("not-an-object");
    assert_eq!(
        envelope.validate_v1().unwrap_err().code,
        EventErrorCode::InvalidEnvelope
    );

    envelope.payload = serde_json::json!({});
    envelope
        .extensions
        .insert("schema".to_owned(), serde_json::json!("shadow"));
    assert_eq!(
        envelope.validate_v1().unwrap_err().code,
        EventErrorCode::InvalidEnvelope
    );
}
