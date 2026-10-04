//! Private composition for the distributable `margins` binary.
//!
//! Parsing and non-interactive workflows remain in the public CLI crate. The
//! native recorder and memo TUI intentionally stay here, on the private side of
//! the open-core boundary.

use crate::note::AGENT_SKILL_DIRS;
use anyhow::Context;
use anyhow::{bail, Result};
#[cfg(feature = "audio-capture")]
use chrono::Local;
use clap::Parser;
use include_dir::{include_dir, Dir};
use margins_cli::args::{Args, Command, SetupLocalModelPolicyArg, SetupSkipArg, SetupStepArg};
use std::ffi::OsString;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
#[cfg(any(
    feature = "audio-capture",
    all(feature = "coreml-asr", target_os = "macos")
))]
use std::path::PathBuf;
#[cfg(any(test, feature = "audio-capture"))]
use std::sync::atomic::AtomicBool;
#[cfg(feature = "audio-capture")]
use std::sync::atomic::AtomicU32;
#[cfg(any(
    test,
    feature = "audio-capture",
    all(feature = "coreml-asr", target_os = "macos")
))]
use std::sync::atomic::AtomicU64;
#[cfg(any(test, feature = "audio-capture"))]
use std::sync::atomic::AtomicU8;
#[cfg(any(
    test,
    feature = "audio-capture",
    all(feature = "coreml-asr", target_os = "macos")
))]
use std::sync::atomic::Ordering;
#[cfg(feature = "audio-capture")]
use std::sync::Mutex;
#[cfg(any(
    test,
    feature = "audio-capture",
    all(feature = "coreml-asr", target_os = "macos")
))]
use std::sync::{mpsc, Arc};

#[cfg(feature = "audio-capture")]
#[path = "cli/native_bridge.rs"]
mod native_bridge;

#[path = "cli/capture_local_runtime.rs"]
mod capture_local_runtime;

#[cfg(feature = "audio-capture")]
#[path = "cli/audio_preferences.rs"]
mod audio_preferences;

trait InteractiveSession {
    fn create(&self, work_dir: &Path, title: Option<&str>) -> Result<()>;
    fn attach(&self, work_dir: &Path, selected: Option<&str>) -> Result<()>;
}

struct NativeInteractiveSession;

#[cfg(any(test, feature = "audio-capture"))]
trait CapturePermissionSource {
    fn permission(&self, lane: margins_core::AudioLane) -> Result<margins_core::PermissionState>;
    fn request_permission(
        &self,
        lane: margins_core::AudioLane,
    ) -> Result<margins_core::PermissionState>;
}

#[cfg(feature = "audio-capture")]
struct NativeCapturePermissionSource;

#[cfg(feature = "audio-capture")]
impl CapturePermissionSource for NativeCapturePermissionSource {
    fn permission(&self, lane: margins_core::AudioLane) -> Result<margins_core::PermissionState> {
        use crate::recorder::MicrophoneAuthorization;
        use margins_core::{AudioLane, PermissionState};

        match lane {
            AudioLane::Microphone => Ok(match crate::recorder::microphone_authorization()? {
                MicrophoneAuthorization::NotDetermined => PermissionState::NotDetermined,
                MicrophoneAuthorization::Restricted => PermissionState::Restricted,
                MicrophoneAuthorization::Denied => PermissionState::Denied,
                MicrophoneAuthorization::Authorized => PermissionState::Granted,
            }),
            // macOS does not expose a dependable passive preflight for the
            // process tap. Starting from the user's explicit capture action and
            // evaluating delivered frames is the truthful probe.
            AudioLane::System => Ok(PermissionState::Unknown),
            _ => Ok(PermissionState::Unavailable),
        }
    }

    fn request_permission(
        &self,
        lane: margins_core::AudioLane,
    ) -> Result<margins_core::PermissionState> {
        use margins_core::{AudioLane, PermissionState};

        match lane {
            AudioLane::Microphone => Ok(if crate::recorder::request_microphone_access()? {
                PermissionState::Granted
            } else {
                PermissionState::Denied
            }),
            AudioLane::System => Ok(PermissionState::Unknown),
            _ => Ok(PermissionState::Unavailable),
        }
    }
}

#[cfg(any(test, feature = "audio-capture"))]
fn ensure_capture_permissions(source: &dyn CapturePermissionSource) -> Result<()> {
    use margins_core::{permission_action, AudioLane, PermissionAction};

    for lane in [AudioLane::Microphone, AudioLane::System] {
        let mut state = source.permission(lane)?;
        if permission_action(state) == PermissionAction::Request {
            state = source.request_permission(lane)?;
        }
        match permission_action(state) {
            PermissionAction::Proceed | PermissionAction::ProbeOnStart => {}
            PermissionAction::Blocked => match lane {
                AudioLane::Microphone => bail!("Margins needs Microphone permission. Grant access to the app or terminal running Margins in System Settings > Privacy & Security > Microphone, then restart it."),
                AudioLane::System => bail!("{}", margins_cli::error::MACOS_SYSTEM_AUDIO_PERMISSION_DENIED_MESSAGE),
                _ => bail!("Margins does not have the required capture permission."),
            },
            PermissionAction::Unavailable => {
                bail!("the requested audio capture lane is unavailable")
            }
            PermissionAction::Request => {
                bail!("capture permission was not granted after the permission request")
            }
            _ => bail!("capture permission state is unsupported by this CLI"),
        }
    }
    Ok(())
}

include!("cli/dispatch.rs");
include!("cli/recall_cmds.rs");
include!("cli/setup.rs");
include!("cli/capture_local.rs");
include!("cli/capture_remote.rs");
include!("cli/live_asr.rs");
#[cfg(test)]
include!("cli/tests.rs");
