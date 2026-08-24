use margins_core::{CaptureError, CaptureErrorCode, DiarizationErrorCode, TranscriptErrorCode};
use std::fmt;

pub const EX_UNAVAILABLE: i32 = 69;

pub const MACOS_SYSTEM_AUDIO_PERMISSION_DENIED_MESSAGE: &str =
    "Margins needs \"Screen & System Audio Recording\" permission. Grant it to your terminal in System Settings > Privacy & Security > Screen & System Audio Recording, then quit and reopen the terminal before running Margins again.";

pub const MACOS_SYSTEM_AUDIO_TAP_FAILURE_CONTEXT: &str =
    "could not create the macOS system-audio tap. The most likely cause is missing \"Screen & System Audio Recording\" permission. Grant it to your terminal in System Settings > Privacy & Security > Screen & System Audio Recording, then quit and reopen the terminal before running Margins again";

pub fn macos_system_audio_permission_likely_message(observation: &str) -> String {
    format!(
        "{observation}. This most likely means the permission is missing. {MACOS_SYSTEM_AUDIO_PERMISSION_DENIED_MESSAGE}"
    )
}

const MACOS_CAPTURE_UNAVAILABLE_MESSAGE: &str =
    "capture is unavailable. On macOS, the most likely cause is missing \"Screen & System Audio Recording\" permission. Grant it to your terminal in System Settings > Privacy & Security > Screen & System Audio Recording, then quit and reopen the terminal before running Margins again.";

fn capture_unavailable_message() -> &'static str {
    if cfg!(target_os = "macos") {
        MACOS_CAPTURE_UNAVAILABLE_MESSAGE
    } else {
        "capture is unavailable in this build"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    code: &'static str,
    message: String,
    exit_code: i32,
}

impl CliError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            exit_code: 1,
        }
    }

    pub fn unavailable(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            exit_code: EX_UNAVAILABLE,
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            code: "usage",
            message: message.into(),
            exit_code: 2,
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    pub fn capture(error: CaptureError) -> Self {
        match error.code {
            CaptureErrorCode::Unavailable => Self::capture_unavailable(),
            CaptureErrorCode::PermissionDenied => {
                Self::new("capture_permission_denied", error.message)
            }
            CaptureErrorCode::DeviceLost => Self::new("capture_device_lost", error.message),
            _ => Self::new("capture_open_failed", error.message),
        }
    }

    pub fn capture_unavailable() -> Self {
        Self::unavailable("capture_unavailable", capture_unavailable_message())
    }

    pub fn asr_unavailable() -> Self {
        Self::unavailable("asr_unavailable", "ASR is unavailable in this build")
    }

    pub fn diarization_unavailable() -> Self {
        Self::unavailable(
            "diarization_unavailable",
            "diarization is unavailable in this build",
        )
    }

    pub fn from_anyhow(error: anyhow::Error) -> Self {
        for cause in error.chain() {
            if let Some(error) = cause.downcast_ref::<margins_core::TranscriptError>() {
                return if error.code == TranscriptErrorCode::Unavailable {
                    Self::asr_unavailable()
                } else {
                    Self::new("asr_failed", error.message.clone())
                };
            }
            if let Some(error) = cause.downcast_ref::<margins_core::DiarizationError>() {
                return if error.code == DiarizationErrorCode::Unavailable {
                    Self::diarization_unavailable()
                } else {
                    Self::new("diarization_failed", error.message.clone())
                };
            }
        }
        Self::new("command_failed", error.to_string())
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_capture_messages_name_the_setting_and_required_restart() {
        for message in [
            MACOS_CAPTURE_UNAVAILABLE_MESSAGE,
            MACOS_SYSTEM_AUDIO_PERMISSION_DENIED_MESSAGE,
            MACOS_SYSTEM_AUDIO_TAP_FAILURE_CONTEXT,
        ] {
            assert!(message.contains("Screen & System Audio Recording"));
            assert!(message.contains("System Settings > Privacy & Security"));
            assert!(message.contains("terminal"));
            assert!(message.contains("quit and reopen"));
        }
        assert!(MACOS_CAPTURE_UNAVAILABLE_MESSAGE.contains("most likely cause"));
        assert!(MACOS_SYSTEM_AUDIO_TAP_FAILURE_CONTEXT.contains("most likely cause"));
        assert!(!MACOS_SYSTEM_AUDIO_PERMISSION_DENIED_MESSAGE.contains("most likely"));

        let likely = macos_system_audio_permission_likely_message(
            "macOS system-audio IO started but delivered only empty buffers",
        );
        assert!(likely.contains("most likely"));
        assert!(likely.contains(MACOS_SYSTEM_AUDIO_PERMISSION_DENIED_MESSAGE));
    }
}
