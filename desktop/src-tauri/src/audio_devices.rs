#[cfg(feature = "tauri-app")]
use margins::recorder;
use serde::Serialize;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::process::Command;
use std::sync::Arc;
#[cfg(feature = "tauri-app")]
use std::time::Duration;

#[derive(Serialize)]
pub(crate) struct AudioTestResult {
    device_name: String,
    peak: f32,
    drop_count: u64,
    ok: bool,
}

#[derive(Serialize)]
pub(crate) struct SystemAudioTestResult {
    peak: f32,
    drop_count: u64,
    silent_secs: f64,
    frame_count: u64,
    status: String,
    message: String,
    restart_recommended: bool,
}

pub(crate) fn verify_microphone_authorization() -> Result<(), String> {
    #[cfg(feature = "tauri-app")]
    match recorder::microphone_authorization().map_err(|error| error.to_string())? {
        recorder::MicrophoneAuthorization::Denied => {
            return Err("Microphone access is denied. Allow Margins in System Settings > Privacy & Security > Microphone, then try again.".to_string());
        }
        recorder::MicrophoneAuthorization::Restricted => {
            return Err("Microphone access is restricted. Check System Settings > Privacy & Security > Microphone or device management restrictions.".to_string());
        }
        recorder::MicrophoneAuthorization::NotDetermined => {
            // Surface the TCC prompt explicitly and wait for the answer. If
            // the stream-open is left to trigger it implicitly, CoreAudio
            // delivers silence while the dialog is unanswered, so the short
            // setup probe reads peak 0 and misreports a working mic as
            // broken (and a first recording starts with a silent mic track).
            // Callers run on spawn_blocking threads, so blocking here is safe.
            let granted =
                recorder::request_microphone_access().map_err(|error| error.to_string())?;
            if !granted {
                return Err("Microphone access is denied. Allow Margins in System Settings > Privacy & Security > Microphone, then try again.".to_string());
            }
        }
        recorder::MicrophoneAuthorization::Authorized => {}
    }
    Ok(())
}

pub(crate) fn test_audio_input(
    registry: Arc<crate::device_registry::DeviceRegistry>,
    device_uid: Option<String>,
) -> Result<AudioTestResult, String> {
    #[cfg(not(feature = "tauri-app"))]
    {
        let _ = (registry, device_uid);
        return Err("microphone capture is not available in headless server mode".to_string());
    }
    #[cfg(feature = "tauri-app")]
    {
        verify_microphone_authorization()?;
        let selection =
            crate::device_registry::resolve_selection(&registry.snapshot(), device_uid.as_deref());
        let selected = registry.resolve_live_device(selection.device_uid.as_deref())?;

        let device_name = selection
            .device_name
            .unwrap_or_else(|| "System Default".to_string());

        let mut target = recorder::MicTarget::from_optional(selected.as_ref());
        if let Some(uid) = selection.device_uid.as_deref() {
            target = target.with_stable_uid(uid);
        }
        let preferred = match std::env::var("MARGINS_MIC_BACKEND") {
            Ok(value) if value.eq_ignore_ascii_case("coreaudio") => {
                recorder::MicBackendKind::CoreAudio
            }
            _ => recorder::MicBackendKind::Cpal,
        };
        let (peak, drop_count) =
            recorder::test_input_target_level(&target, Duration::from_millis(1200), preferred)
                .map_err(|e| format!("Could not test microphone: {e}"))?;

        Ok(AudioTestResult {
            device_name,
            peak,
            drop_count,
            ok: peak >= 0.001,
        })
    }
}

#[cfg(all(feature = "tauri-app", any(target_os = "macos", target_os = "windows")))]
pub(crate) fn test_system_audio_tap() -> Result<SystemAudioTestResult, String> {
    let probe = match recorder::test_system_audio_tap_level(Duration::from_millis(1800)) {
        Ok(result) => result,
        Err(e) => {
            let message = format!(
                "Could not complete the computer-audio check: {e}. Check system audio settings for Margins, then quit and reopen the app before testing again."
            );
            return Ok(SystemAudioTestResult {
                peak: 0.0,
                drop_count: 0,
                silent_secs: 0.0,
                frame_count: 0,
                status: "blocked".to_string(),
                message,
                restart_recommended: true,
            });
        }
    };

    let (status, message, restart_recommended) =
        classify_system_audio_tap(probe.peak, probe.frame_count, probe.test_tone_played);

    Ok(SystemAudioTestResult {
        peak: probe.peak,
        drop_count: probe.drop_count,
        silent_secs: probe.silent_secs,
        frame_count: probe.frame_count,
        status,
        message,
        restart_recommended,
    })
}

#[cfg(not(all(feature = "tauri-app", any(target_os = "macos", target_os = "windows"))))]
pub(crate) fn test_system_audio_tap() -> Result<SystemAudioTestResult, String> {
    Ok(SystemAudioTestResult {
        peak: 0.0,
        drop_count: 0,
        silent_secs: 0.0,
        frame_count: 0,
        status: "unsupported".to_string(),
        message: "Computer audio capture is not available in this build on this operating system."
            .to_string(),
        restart_recommended: false,
    })
}

fn classify_system_audio_tap(
    peak: f32,
    frame_count: u64,
    test_tone_played: bool,
) -> (String, String, bool) {
    if !test_tone_played {
        (
            "quiet".to_string(),
            "Couldn't hear the test sound — check output volume and try again.".to_string(),
            false,
        )
    } else if frame_count == 0 {
        (
            "blocked".to_string(),
            "Computer audio is not reaching Margins. Check system audio settings, then quit and reopen Margins.".to_string(),
            true,
        )
    } else if peak < 0.001 {
        (
            "quiet".to_string(),
            "Couldn't hear the test sound — check output volume and try again.".to_string(),
            false,
        )
    } else {
        (
            "ok".to_string(),
            "Computer-audio tap is receiving signal.".to_string(),
            false,
        )
    }
}

#[cfg(target_os = "macos")]
fn macos_privacy_pane_url(pane: &str, product_version: Option<&str>) -> &'static str {
    match pane {
        "microphone" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        // AudioHardwareCreateProcessTap is gated by kTCCServiceAudioCapture.
        // Sonoma grouped its recovery UI into the screen-recording pane; newer
        // System Settings exposes the AudioCapture section directly.
        "system-audio" | "audio-capture"
            if product_version
                .and_then(|version| version.split('.').next())
                .and_then(|major| major.parse::<u32>().ok())
                .is_some_and(|major| major < 15) =>
        {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        "system-audio" | "audio-capture" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_AudioCapture"
        }
        "screen-capture" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        _ => "x-apple.systempreferences:com.apple.preference.security",
    }
}

pub(crate) fn open_privacy_pane(pane: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let product_version = Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok());
        let url = macos_privacy_pane_url(&pane, product_version.as_deref());
        let status = Command::new("open")
            .arg(url)
            .status()
            .map_err(|e| format!("failed to open System Settings: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err("System Settings did not open successfully.".to_string())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        #[cfg(target_os = "windows")]
        {
            let uri = match pane.as_str() {
                "microphone" => "ms-settings:privacy-microphone",
                "system-audio" | "audio-capture" => "ms-settings:sound",
                _ => "ms-settings:privacy",
            };
            let status = Command::new("explorer.exe")
                .arg(uri)
                .status()
                .map_err(|e| format!("failed to open Windows Settings: {e}"))?;
            if status.success() {
                Ok(())
            } else {
                Err("Windows Settings did not open successfully.".to_string())
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = pane;
            Err("System privacy panes are only available on macOS and Windows.".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::classify_system_audio_tap;

    #[test]
    fn delivered_silent_frames_are_inconclusive_not_blocked() {
        let (status, message, restart_recommended) = classify_system_audio_tap(0.0, 86_400, true);
        assert_eq!(status, "quiet");
        assert_eq!(
            message,
            "Couldn't hear the test sound — check output volume and try again."
        );
        assert!(!restart_recommended);
    }

    #[test]
    fn live_noise_floor_without_audio_is_quiet_not_blocked() {
        let (status, _, restart_recommended) = classify_system_audio_tap(0.0002, 86_400, true);
        assert_eq!(status, "quiet");
        assert!(!restart_recommended);
    }

    #[test]
    fn audible_system_audio_is_ready() {
        let (status, _, restart_recommended) = classify_system_audio_tap(0.02, 86_400, true);
        assert_eq!(status, "ok");
        assert!(!restart_recommended);
    }

    #[test]
    fn tap_that_opens_but_yields_no_frames_is_blocked() {
        let (status, _, restart_recommended) = classify_system_audio_tap(0.0, 0, true);
        assert_eq!(status, "blocked");
        assert!(restart_recommended);
    }

    #[test]
    fn failed_test_tone_is_inconclusive_even_without_frames() {
        let (status, message, restart_recommended) = classify_system_audio_tap(0.0, 0, false);
        assert_eq!(status, "quiet");
        assert!(message.contains("test sound"));
        assert!(!restart_recommended);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn system_audio_routes_to_audio_capture_privacy_pane() {
        assert!(super::macos_privacy_pane_url("system-audio", Some("26.2"))
            .ends_with("Privacy_AudioCapture"));
        assert!(super::macos_privacy_pane_url("system-audio", Some("14.7"))
            .ends_with("Privacy_ScreenCapture"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn first_run_reset_uses_same_audio_capture_service_as_recovery() {
        let script = include_str!("../../scripts/reinstall-first-run-test.sh");
        assert!(script.contains("FIRST_RUN_TCC_SERVICES=(Microphone AudioCapture)"));
        assert!(!script.contains("FIRST_RUN_TCC_SERVICES=(Microphone ScreenCapture)"));
        assert!(super::macos_privacy_pane_url("system-audio", Some("26.2"))
            .ends_with("Privacy_AudioCapture"));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn non_macos_system_audio_test_reports_unsupported() {
        let result = super::test_system_audio_tap().expect("system audio test result");
        assert_eq!(result.status, "unsupported");
        assert!(!result.restart_recommended);
    }
}
