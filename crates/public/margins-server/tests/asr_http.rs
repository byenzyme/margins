use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt as _;
use margins_core::{AsrBackend, AsrRequest, AsrResult, TranscriptError, TranscriptWord};
use margins_meeting_protocol::*;
use margins_server::{
    asr::{RemoteAsrJobs, SpeechSetup},
    http::build_router,
    ServerState,
};
use margins_store::canonical;
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

fn finalize_native_pcm(service: &WorkspaceService, principal: &ServicePrincipal) {
    let segment_id = format!("{SESSION}-seg-0");
    let reservation = service
        .reserve_session(
            principal,
            command(
                "native-create",
                ClientMessageBodyV1::CreateSession(CreateSessionV1 {
                    idempotency_key: "native-asr-http-create".into(),
                    started_at_unix_ms: UnixMillis(BASE),
                    title: Some("Native artifact fixture".into()),
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
                            producer: "margins-tui".into(),
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
    for sequence in 0..2 {
        let payload = vec![sequence as u8; 3_200];
        service
            .execute_capture(
                principal,
                &reservation.producer_token,
                command(
                    &format!("native-chunk-{sequence}"),
                    ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
                        segment_id: segment_id.clone().into(),
                        lane_id: "mic".into(),
                        sequence,
                        starts_at_ms: SessionMillis(sequence * 100),
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
    }
    service
        .execute_capture(
            principal,
            &reservation.producer_token,
            command(
                "native-close",
                ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
                    segment_id: segment_id.clone().into(),
                    ended_at_ms: SessionMillis(200),
                    lane_boundaries: vec![LaneBoundaryV1 {
                        lane_id: "mic".into(),
                        next_sequence: 2,
                    }],
                    reason: SegmentCloseReasonV1::Stop,
                }),
            ),
        )
        .unwrap();
    service
        .execute_capture(
            principal,
            &reservation.producer_token,
            command(
                "native-finalize",
                ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
                    ended_at_ms: SessionMillis(200),
                    segment_closes: vec![SegmentCloseReferenceV1 {
                        segment_id: segment_id.into(),
                        close_message_id: "native-close".into(),
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
    assert_eq!(transcript["result"]["live"], false);
    assert!(transcript["result"]["body"]
        .as_str()
        .unwrap()
        .contains("[00:01] you (mic): spoken evidence"));
    let meta = canonical::get_session_meta(service.margins_dir(), SESSION).unwrap();
    assert_eq!(meta.processing_state.as_deref(), Some("done"));
    assert_eq!(
        canonical::transcript_coverage_read_only(service.margins_dir(), SESSION).unwrap(),
        canonical::transcript_coverage(&meta.segments)
    );
    canonical::add_segment(
        service.margins_dir(),
        SESSION,
        1,
        ".margins/asr-http_seg1.wav",
        100,
        Some(0.1),
    )
    .unwrap();
    let stale = service.transcript(&principal, SESSION).unwrap();
    assert!(!stale.terminal);
    assert!(!stale.body.contains("spoken evidence"));
}

#[tokio::test]
async fn interrupted_native_audio_download_leaves_no_wav() {
    let temp = tempfile::tempdir().unwrap();
    let (app, service, principal) = fixture(temp.path());
    finalize_native_pcm(&service, &principal);
    let artifact = service
        .artifacts(&principal, SESSION)
        .unwrap()
        .into_iter()
        .find(|artifact| artifact.kind == "audio_mic_runtime")
        .expect("native mic artifact");
    assert_eq!(artifact.size_bytes, Some(6_444));
    let wav = service.margins_dir().join(format!("{SESSION}_seg0.wav"));
    assert!(!wav.exists());

    let request = Request::builder()
        .method(Method::GET)
        .uri(format!(
            "/v1/workspaces/practice/artifacts/{}/content",
            artifact.artifact_id.as_ref()
        ))
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("x-margins-instance-id", "test-instance")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(&first[..4], b"RIFF");
    assert_eq!(&first[8..12], b"WAVE");
    assert_eq!(u16::from_le_bytes(first[22..24].try_into().unwrap()), 1);
    assert_eq!(
        u32::from_le_bytes(first[24..28].try_into().unwrap()),
        16_000
    );
    assert!(first.len() < 6_444, "test must interrupt an active stream");
    assert!(!wav.exists(), "stream must use an anonymous temporary file");
    drop(body);
    assert!(
        !wav.exists(),
        "dropping a download must leave no WAV on disk"
    );
}

#[tokio::test]
async fn legacy_browser_webm_upload_transcribes_without_runtime_capture_state() {
    let temp = tempfile::tempdir().unwrap();
    let (app, service, principal) = fixture(temp.path());
    let margins_dir = service.margins_dir();
    canonical::create_session(
        margins_dir,
        SESSION,
        &chrono::Local::now(),
        ".margins/asr-http.md",
    )
    .unwrap();
    canonical::add_segment(
        margins_dir,
        SESSION,
        0,
        ".margins/recordings/asr-http_seg0.wav",
        0,
        None,
    )
    .unwrap();
    let recordings = margins_dir.join("recordings");
    std::fs::create_dir_all(&recordings).unwrap();
    std::fs::write(
        recordings.join("asr-http_upload.webm"),
        include_bytes!("fixtures/legacy-browser.webm"),
    )
    .unwrap();
    std::fs::write(
        recordings.join("old-recording.recovery.json"),
        serde_json::to_vec(&serde_json::json!({
            "version": 2,
            "recording_id": "old-recording",
            "session_name": SESSION,
            "webm_chunk_count": 1
        }))
        .unwrap(),
    )
    .unwrap();
    let (status, receipt) = call(
        &app,
        Method::POST,
        &format!("/v1/workspaces/practice/sessions/{SESSION}/jobs/transcribe"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    for _ in 0..100 {
        let job = service
            .latest_job(&principal, &SessionId(SESSION.into()))
            .unwrap()
            .unwrap();
        match job.status.as_str() {
            "complete" => break,
            "failed" => panic!("legacy ASR job failed: {:?}", job.failure),
            _ => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    let (status, transcript) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{SESSION}/transcript"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{transcript}");
    let body = transcript["result"]["body"].as_str().unwrap();
    assert!(body.contains("spoken evidence"), "{body}");
    assert!(body.contains("Incomplete recording"), "{body}");
    assert_eq!(transcript["result"]["terminal"], true);
    let summary = service
        .session(&principal, &SessionId(SESSION.into()))
        .unwrap();
    assert!(summary.capture_incomplete);
    assert_eq!(summary.capture_gaps[0].reason, "legacy_unverified_upload");
}

#[tokio::test]
async fn undecodable_webm_segment_is_recorded_as_gap_while_later_audio_transcribes() {
    let temp = tempfile::tempdir().unwrap();
    let (app, service, principal) = fixture(temp.path());
    let reservation = service
        .reserve_session(
            &principal,
            command(
                "webm-create",
                ClientMessageBodyV1::CreateSession(CreateSessionV1 {
                    idempotency_key: "webm-gap-create".into(),
                    started_at_unix_ms: UnixMillis(BASE),
                    title: Some("WebM gap fixture".into()),
                    sources: vec![CaptureSourceV1 {
                        source_id: "microphone".into(),
                        kind: CaptureSourceKindV1::Microphone,
                        label: None,
                        external_id: None,
                    }],
                    lanes: vec![CaptureLaneV1 {
                        lane_id: "browser-audio".into(),
                        source_ids: vec!["microphone".into()],
                        label: None,
                        format: AudioFormatV1 {
                            codec: AudioCodecV1::Opus,
                            container: AudioContainerV1::Webm,
                            sample_rate_hz: 48_000,
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
    for (segment, message, start, audio) in [
        ("bad", "bad-chunk", 0, b"invalid WebM".as_slice()),
        (
            "good",
            "good-chunk",
            1_000,
            include_bytes!("fixtures/legacy-browser.webm").as_slice(),
        ),
    ] {
        service
            .execute_capture(
                &principal,
                &reservation.producer_token,
                command(
                    message,
                    ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
                        segment_id: segment.into(),
                        lane_id: "browser-audio".into(),
                        sequence: 0,
                        starts_at_ms: SessionMillis(start),
                        duration_ms: DurationMillis(300),
                        payload_digest: ContentDigestV1 {
                            algorithm: DigestAlgorithmV1::Sha256,
                            hex: format!("{:x}", Sha256::digest(audio)),
                        },
                        payload: audio.to_vec(),
                    }),
                ),
            )
            .unwrap();
        service
            .execute_capture(
                &principal,
                &reservation.producer_token,
                command(
                    &format!("close-{segment}"),
                    ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
                        segment_id: segment.into(),
                        ended_at_ms: SessionMillis(start + 300),
                        lane_boundaries: vec![LaneBoundaryV1 {
                            lane_id: "browser-audio".into(),
                            next_sequence: 1,
                        }],
                        reason: if segment == "bad" {
                            SegmentCloseReasonV1::Pause
                        } else {
                            SegmentCloseReasonV1::Stop
                        },
                    }),
                ),
            )
            .unwrap();
    }
    service
        .execute_capture(
            &principal,
            &reservation.producer_token,
            command(
                "webm-finalize",
                ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
                    ended_at_ms: SessionMillis(1_300),
                    segment_closes: vec![
                        SegmentCloseReferenceV1 {
                            segment_id: "bad".into(),
                            close_message_id: "close-bad".into(),
                        },
                        SegmentCloseReferenceV1 {
                            segment_id: "good".into(),
                            close_message_id: "close-good".into(),
                        },
                    ],
                    reason: SessionFinalizeReasonV1::Completed,
                }),
            ),
        )
        .unwrap();
    let (status, receipt) = call(
        &app,
        Method::POST,
        &format!("/v1/workspaces/practice/sessions/{SESSION}/jobs/transcribe"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    for _ in 0..100 {
        let job = service
            .latest_job(&principal, &SessionId(SESSION.into()))
            .unwrap()
            .unwrap();
        match job.status.as_str() {
            "complete" => break,
            "failed" => panic!("WebM ASR job failed: {:?}", job.failure),
            _ => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    let summary = service
        .session(&principal, &SessionId(SESSION.into()))
        .unwrap();
    assert!(summary.capture_incomplete);
    assert!(summary.capture_gaps.iter().any(|gap| {
        gap.segment_id == "bad"
            && gap.start_sequence == 0
            && gap.end_exclusive == 1
            && gap.reason == "undecodable_webm"
    }));
    let (status, transcript) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{SESSION}/transcript"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{transcript}");
    let body = transcript["result"]["body"].as_str().unwrap();
    assert!(body.contains("spoken evidence"), "{body}");
    assert!(body.contains("Incomplete recording"), "{body}");
    assert_eq!(transcript["result"]["terminal"], true);
    let (_, restarted, restarted_principal) = fixture(temp.path());
    assert!(
        restarted
            .session(&restarted_principal, &SessionId(SESSION.into()))
            .unwrap()
            .capture_incomplete
    );
}

#[tokio::test]
async fn zero_audio_incomplete_session_gets_terminal_explanatory_transcript() {
    let temp = tempfile::tempdir().unwrap();
    let (app, service, principal) = fixture(temp.path());
    let reservation = service
        .reserve_session(
            &principal,
            command(
                "empty-create",
                ClientMessageBodyV1::CreateSession(CreateSessionV1 {
                    idempotency_key: "empty-create".into(),
                    started_at_unix_ms: UnixMillis(BASE),
                    title: Some("Interrupted before audio".into()),
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
    service
        .execute_capture(
            &principal,
            &reservation.producer_token,
            command(
                "empty-finalize",
                ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
                    ended_at_ms: SessionMillis(0),
                    segment_closes: vec![],
                    reason: SessionFinalizeReasonV1::Error,
                }),
            ),
        )
        .unwrap();
    let (status, receipt) = call(
        &app,
        Method::POST,
        &format!("/v1/workspaces/practice/sessions/{SESSION}/jobs/transcribe"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    for _ in 0..100 {
        let job = service
            .latest_job(&principal, &SessionId(SESSION.into()))
            .unwrap()
            .unwrap();
        match job.status.as_str() {
            "complete" => break,
            "failed" => panic!("zero-audio ASR job failed: {:?}", job.failure),
            _ => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    let (status, transcript) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{SESSION}/transcript"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{transcript}");
    let body = transcript["result"]["body"].as_str().unwrap();
    assert!(body.contains("Incomplete recording"), "{body}");
    assert!(body.contains("_No timestamped transcript"), "{body}");
    assert_eq!(transcript["result"]["terminal"], true);
    let coverage = canonical::transcript_coverage_read_only(service.margins_dir(), SESSION)
        .unwrap()
        .unwrap();
    assert_eq!(coverage.segment_count, 0);
}

#[tokio::test]
async fn finalized_legacy_browser_wav_transcribes_after_webm_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let (app, service, principal) = fixture(temp.path());
    let margins_dir = service.margins_dir();
    canonical::create_session(
        margins_dir,
        SESSION,
        &chrono::Local::now(),
        ".margins/asr-http.md",
    )
    .unwrap();
    canonical::add_segment(
        margins_dir,
        SESSION,
        0,
        ".margins/recordings/asr-http_seg0.wav",
        0,
        Some(0.1),
    )
    .unwrap();
    let recordings = margins_dir.join("recordings");
    std::fs::create_dir_all(&recordings).unwrap();
    let mut writer = hound::WavWriter::create(
        recordings.join("asr-http_seg0.wav"),
        hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for _ in 0..1_600 {
        writer.write_sample(0.0_f32).unwrap();
    }
    writer.finalize().unwrap();
    // A crash during old cleanup could retain an upload after the WAV was
    // finalized. The canonical WAV remains the safer source.
    std::fs::write(recordings.join("asr-http_upload.webm"), b"stale upload").unwrap();
    let (status, receipt) = call(
        &app,
        Method::POST,
        &format!("/v1/workspaces/practice/sessions/{SESSION}/jobs/transcribe"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    for _ in 0..100 {
        let job = service
            .latest_job(&principal, &SessionId(SESSION.into()))
            .unwrap()
            .unwrap();
        match job.status.as_str() {
            "complete" => break,
            "failed" => panic!("legacy WAV ASR job failed: {:?}", job.failure),
            _ => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    let transcript = std::fs::read_to_string(
        margins_dir
            .join("artifacts")
            .join(SESSION)
            .join("transcript.md"),
    )
    .unwrap();
    assert!(transcript.contains("spoken evidence"), "{transcript}");
    assert!(!transcript.contains("Incomplete recording"), "{transcript}");
}
