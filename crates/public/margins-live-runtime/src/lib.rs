//! Small, transport-neutral control seam for a locally running Margins meeting.
//!
//! This crate does not own HTTP, process discovery, Tauri, audio capture, or
//! installation. A concrete recording process implements [`LiveRuntime`], and
//! an adapter chooses how local clients reach it.

#![forbid(unsafe_code)]

use margins_meeting_protocol::{
    DesktopLiveAppendMemoRequestV1, DesktopLiveErrorV1, DesktopLiveMutationResponseV1,
    DesktopLiveSessionRequestV1, DesktopLiveSnapshotV1, DesktopLiveStartRequestV1,
    DesktopLiveUpdateNotepadRequestV1, LiveOperationId, ValidationErrorV1,
};
use std::{future::Future, pin::Pin};

/// The wire types retain their V1 desktop names for compatibility with the
/// first adapter. Callers of this crate can speak in terms of the runtime.
pub type LiveSnapshotV1 = DesktopLiveSnapshotV1;
pub type LiveMutationResponseV1 = DesktopLiveMutationResponseV1;
pub type LiveErrorV1 = DesktopLiveErrorV1;
pub type LiveStartRequestV1 = DesktopLiveStartRequestV1;
pub type LiveSessionRequestV1 = DesktopLiveSessionRequestV1;
pub type LiveAppendMemoRequestV1 = DesktopLiveAppendMemoRequestV1;
pub type LiveUpdateNotepadRequestV1 = DesktopLiveUpdateNotepadRequestV1;

/// One state-changing request to the process that owns the active meeting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveRuntimeCommandV1 {
    Start(LiveStartRequestV1),
    Pause(LiveSessionRequestV1),
    Resume(LiveSessionRequestV1),
    Stop(LiveSessionRequestV1),
    AppendMemo(LiveAppendMemoRequestV1),
    UpdateNotepad(LiveUpdateNotepadRequestV1),
}

impl LiveRuntimeCommandV1 {
    pub fn operation_id(&self) -> &LiveOperationId {
        match self {
            Self::Start(request) => &request.operation_id,
            Self::Pause(request) | Self::Resume(request) | Self::Stop(request) => {
                &request.operation_id
            }
            Self::AppendMemo(request) => &request.operation_id,
            Self::UpdateNotepad(request) => &request.operation_id,
        }
    }

    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        match self {
            Self::Start(request) => request.validate(),
            Self::Pause(request) | Self::Resume(request) | Self::Stop(request) => {
                request.validate()
            }
            Self::AppendMemo(request) => request.validate(),
            Self::UpdateNotepad(request) => request.validate(),
        }
    }
}

/// Boxed so a runtime can be used behind an `Arc` without choosing an async
/// runtime or adding an async-trait dependency to this public seam.
pub type LiveRuntimeFuture<'a> =
    Pin<Box<dyn Future<Output = Result<LiveMutationResponseV1, LiveErrorV1>> + Send + 'a>>;

/// The recording process behind a local control surface.
pub trait LiveRuntime: Send + Sync {
    fn snapshot(&self, session_id: Option<&str>) -> Result<LiveSnapshotV1, LiveErrorV1>;

    fn execute(&self, command: LiveRuntimeCommandV1) -> LiveRuntimeFuture<'_>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use margins_meeting_protocol::{
        DesktopLiveErrorCodeV1, DesktopLiveHealthV1, ProtocolVersionV1, UnixMillis,
    };

    struct StubRuntime;

    impl LiveRuntime for StubRuntime {
        fn snapshot(
            &self,
            _session_id: Option<&str>,
        ) -> Result<DesktopLiveSnapshotV1, DesktopLiveErrorV1> {
            Ok(DesktopLiveSnapshotV1 {
                protocol_version: ProtocolVersionV1,
                server_unix_ms: UnixMillis(1),
                session: None,
                health: DesktopLiveHealthV1 {
                    capture_phase: "idle".into(),
                    tap_status: "not_expected".into(),
                    tap_warning: None,
                    system_audio_expected: false,
                    system_audio_observed: false,
                    transcript_freshness: None,
                },
                rolling_transcript: Vec::new(),
                memo_lines: Vec::new(),
                notepad_revision: "empty".into(),
            })
        }

        fn execute(&self, _command: LiveRuntimeCommandV1) -> LiveRuntimeFuture<'_> {
            Box::pin(async {
                Err(DesktopLiveErrorV1::new(
                    DesktopLiveErrorCodeV1::NotReady,
                    "stub",
                    false,
                ))
            })
        }
    }

    #[test]
    fn runtime_is_object_safe_and_shareable() {
        let runtime: std::sync::Arc<dyn LiveRuntime> = std::sync::Arc::new(StubRuntime);
        let snapshot = runtime.snapshot(None).unwrap();
        assert_eq!(snapshot.server_unix_ms, UnixMillis(1));
    }

    #[test]
    fn command_uses_the_requests_operation_id_and_validation() {
        let command = LiveRuntimeCommandV1::Start(DesktopLiveStartRequestV1 {
            operation_id: LiveOperationId("start-1".into()),
            name: "customer-call".into(),
            project_id: None,
        });
        assert_eq!(command.operation_id().as_ref(), "start-1");
        assert!(command.validate().is_ok());

        let invalid = LiveRuntimeCommandV1::Start(DesktopLiveStartRequestV1 {
            operation_id: LiveOperationId("start-2".into()),
            name: " ".into(),
            project_id: None,
        });
        assert!(invalid.validate().is_err());
    }
}
