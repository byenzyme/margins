pub use margins_capture::app;
pub use margins_capture::asr;
pub mod audio_info;
pub mod audio_pipeline;
#[cfg(feature = "recall-local-model")]
pub mod catalyst_model_setup;
pub mod cli;
pub use margins_capture::cli_log;
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
pub use margins_capture::coreml_asr;
pub use margins_capture::diarization;
pub mod google_oauth_client;
pub mod granola_import;
pub mod hosted_credentials;
pub mod included_lease;
mod note;
pub mod note_artifacts;
pub use margins_capture::offline_asr;
pub mod project;
#[cfg(feature = "recall")]
pub mod recall;
#[cfg(feature = "recall")]
pub mod scan;
#[cfg(feature = "recall")]
pub mod enzyme_cli;
#[cfg(feature = "recall")]
mod setup_compile;
#[cfg(feature = "recall")]
mod workspace_recall;
/// Portable public contracts re-exported by the root crate.
pub use margins_core as core;

/// Initialize the process-wide SQLite runtime before the linked engine (still
/// used by setup scan and compile) and Margins storage can open their
/// independently wrapped connections.
pub fn initialize_sqlite_runtime() -> anyhow::Result<()> {
    #[cfg(feature = "recall")]
    recall_engine::initialize_sqlite_runtime()?;
    Ok(())
}

// libsql configures SQLite's process-wide serialized mode on its first open.
// The unit-test harness can run store tests before a recall test, unlike the
// private CLI, which initializes SQLite before dispatch. Start the same runtime
// before any test thread can open a libsql-rusqlite connection: SQLite rejects
// sqlite3_config with SQLITE_MISUSE once another wrapper has initialized it.
#[cfg(all(test, feature = "recall"))]
#[ctor::ctor]
fn initialize_sqlite_before_unit_tests() {
    initialize_sqlite_runtime().expect("initialize SQLite before unit tests");
}

#[cfg(test)]
pub(crate) fn test_process_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
pub use margins_capture::recorder;
pub mod session;
pub mod session_index;
pub use margins_capture::text_helpers;
pub use margins_capture::tui;

#[cfg(test)]
mod media_compatibility_tests {
    #[test]
    fn root_recorder_path_reexports_portable_timeline_helpers() {
        assert_eq!(
            crate::recorder::target_frame(1_000_000_000, 48_000).unwrap(),
            48_000
        );

        let mut resampler = crate::recorder::RationalResampler::new(2, 1).unwrap();
        let mut output = resampler.process(&[0.0, 1.0]).unwrap();
        output.extend(resampler.finish().unwrap());
        assert_eq!(output, vec![1.0]);
    }

    #[test]
    fn root_media_facades_preserve_legacy_type_and_function_paths() {
        let words = vec![crate::asr::WordTiming {
            start_ms: 10,
            end_ms: 20,
            text: "hello".into(),
        }];
        let entries = crate::asr::words_to_transcript_entries(&words, 1, 5);
        assert_eq!(entries[0].start_ms, 15);

        let audio = crate::audio_pipeline::AudioBuffer {
            samples: vec![0.25, 0.75],
            sample_rate: 16_000,
            channels: 2,
        };
        assert_eq!(
            crate::audio_pipeline::downmix_to_mono(&audio).unwrap(),
            vec![0.5]
        );
    }
}
