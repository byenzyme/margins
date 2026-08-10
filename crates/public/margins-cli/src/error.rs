use margins_core::{CaptureError, CaptureErrorCode, DiarizationErrorCode, TranscriptErrorCode};
use std::fmt;

pub const EX_UNAVAILABLE: i32 = 69;

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
        Self::unavailable(
            "capture_unavailable",
            "capture is unavailable in this build",
        )
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
