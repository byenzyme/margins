use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use margins_core::{AsrBackend, AsrRequest, AsrResult, TranscriptError, TranscriptWord};
use margins_meeting_protocol::*;
use margins_server::{
    asr::{RemoteAsrJobs, SpeechSetup},
    http::build_router,
    ServerState,
};
use margins_workflows::{
    workspace::ensure_service_workspace,
    workspace_service::{ScopedCredentialStore, ServicePrincipal, WorkspaceService},
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use tower::ServiceExt;

const SESSION: &str = "asr-http";
const TOKEN: &str = "asr-http-test-token";
const BASE: u64 = 1_800_000_000_000;

struct FakeAsr;

impl AsrBackend for FakeAsr {
    fn transcribe(&self, request: AsrRequest) -> Result<AsrResult, TranscriptError> {
        assert_eq!(request.sample_rate_hz, 16_000);
        assert!(!request.samples.is_empty());
        Ok(AsrResult {
            words: vec![TranscriptWord {
                start_ms: request.session_offset_ms + 1_000,
                end_ms: request.session_offset_ms + 1_200,
                text: "spoken evidence".into(),
                speaker: None,
                confidence_per_mille: None,
            }],
            detected_language: None,
        })
    }
}

fn command(id: &str, body: ClientMessageBodyV1) -> ClientMessageV1 {
    ClientMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: id.into(),
        session_id: SESSION.into(),
        sent_at_unix_ms: UnixMillis(BASE),
        body,
    }
}

fn fixture(root: &Path) -> (Router, Arc<WorkspaceService>, ServicePrincipal) {
    let notes = root.join("notes");
    let captures = root.join("captures");
    std::fs::create_dir_all(&notes).unwrap();
    let workspace =
        ensure_service_workspace(&root.join("home"), "practice", None, &notes, &captures).unwrap();
    let service = Arc::new(
        WorkspaceService::open_with_capabilities("test-instance", workspace, true, false).unwrap(),
    );
    let principal = ServicePrincipal::full("test-principal", "practice");
    let credentials = ScopedCredentialStore::open(root.join("credentials.json")).unwrap();
    credentials
        .register(
            &principal.id,
            TOKEN,
            principal.workspace_ids.iter().cloned().collect(),
            principal.operations.iter().cloned().collect(),
            None,
        )
        .unwrap();
    let app = build_router(ServerState {
        workspace_service: service.clone(),
        credential_store: credentials,
        service_principal: principal.clone(),
        asr_setup: SpeechSetup::new(true, true),
        remote_asr_jobs: RemoteAsrJobs::with_backend(Arc::new(FakeAsr)),
    });
    (app, service, principal)
}

fn finalize_pcm(service: &WorkspaceService, principal: &ServicePrincipal) {
    let reservation = service
        .reserve_session(
            principal,
            command(
                "create",
                ClientMessageBodyV1::CreateSession(CreateSessionV1 {
                    idempotency_key: "asr-http-create".into(),
                    started_at_unix_ms: UnixMillis(BASE),
                    title: Some("ASR HTTP fixture".into()),
                    sources: vec![CaptureSourceV1 {
                        source_id: "mic".into(),
                        kind: CaptureSourceKindV1::Microphone,
                        label: None,
                        external_id: None,
                    }],
                    lanes: vec![CaptureLaneV1 {
                        lane_id: "mic".into(),
                        source_ids: vec!["mic".into()],
                        label: None,
                        format: AudioFormatV1 {
                            codec: AudioCodecV1::PcmS16Le,
                            container: AudioContainerV1::Raw,
                            sample_rate_hz: 16_000,
                            channel_count: 1,
                        },
                    }],
                    provenance: CaptureProvenanceV1 {
                        hops: vec![CaptureProvenanceHopV1 {
                            producer: "asr-http-test".into(),
                            producer_version: Some("1".into()),
                            mode: CaptureModeV1::Live,
                            observed_at_unix_ms: UnixMillis(BASE),
                            attributes: BTreeMap::new(),
                        }],
                    },
                }),
            ),
        )
        .unwrap();
    let token = &reservation.producer_token;
    let samples = vec![0_i16; 1_600];
    let payload = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    service
        .execute_capture(
            principal,
            token,
            command(
                "chunk",
                ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
                    segment_id: "segment-1".into(),
                    lane_id: "mic".into(),
                    sequence: 0,
                    starts_at_ms: SessionMillis(0),
                    duration_ms: DurationMillis(100),
                    payload_digest: ContentDigestV1 {
                        algorithm: DigestAlgorithmV1::Sha256,
                        hex: format!("{:x}", Sha256::digest(&payload)),
                    },
                    payload,
                }),
            ),
        )
        .unwrap();
    service
        .execute_capture(
            principal,
            token,
            command(
                "close",
                ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
                    segment_id: "segment-1".into(),
                    ended_at_ms: SessionMillis(100),
                    lane_boundaries: vec![LaneBoundaryV1 {
                        lane_id: "mic".into(),
                        next_sequence: 1,
                    }],
                    reason: SegmentCloseReasonV1::Stop,
                }),
            ),
        )
        .unwrap();
    service
        .execute_capture(
            principal,
            token,
            command(
                "finalize",
                ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
                    ended_at_ms: SessionMillis(100),
                    segment_closes: vec![SegmentCloseReferenceV1 {
                        segment_id: "segment-1".into(),
                        close_message_id: "close".into(),
                    }],
                    reason: SessionFinalizeReasonV1::Completed,
                }),
            ),
        )
        .unwrap();
}

async fn call(app: &Router, method: Method, path: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("x-margins-instance-id", "test-instance")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn typed_transcribe_route_runs_durable_job_against_runtime_audio() {
    let temp = tempfile::tempdir().unwrap();
    let (app, service, principal) = fixture(temp.path());
    finalize_pcm(&service, &principal);
    let job_path = format!("/v1/workspaces/practice/sessions/{SESSION}/jobs/transcribe");
    let (status, receipt) = call(&app, Method::POST, &job_path).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["result"]["job_id"], "transcribe:asr-http");
    for _ in 0..100 {
        let job = service
            .latest_job(&principal, &SessionId(SESSION.into()))
            .unwrap()
            .unwrap();
        match job.status.as_str() {
            "complete" => break,
            "failed" => panic!("ASR job failed: {:?}", job.failure),
            _ => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    let job = service
        .latest_job(&principal, &SessionId(SESSION.into()))
        .unwrap()
        .unwrap();
    assert_eq!(job.status, "complete");
    assert_eq!(
        job.result_ref.as_deref(),
        Some(".margins/artifacts/asr-http/transcript.md")
    );
    let (status, transcript) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{SESSION}/transcript"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{transcript}");
    assert_eq!(transcript["result"]["terminal"], true);
    assert!(transcript["result"]["body"]
        .as_str()
        .unwrap()
        .contains("[00:01] you (mic): spoken evidence"));
}
