use chrono::Local;
use margins_cli::services::{CliServices, ProjectService};
use margins_core::{
    AsrBackend, AsrRequest, AsrResult, TranscriptError, TranscriptErrorCode, TranscriptWord,
};
use margins_store::legacy;
use margins_workflows::project::{ProjectSource, ResolvedProject};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct Project(PathBuf);

impl ProjectService for Project {
    fn list(&self) -> anyhow::Result<Vec<ResolvedProject>> {
        Ok(vec![self.resolve(None)?])
    }
    fn resolve(&self, _selector: Option<&str>) -> anyhow::Result<ResolvedProject> {
        Ok(ResolvedProject {
            project: ProjectSource {
                id: "test".into(),
                name: "Test".into(),
                path: self.0.to_string_lossy().into_owned(),
                inbox_folder: "meetings".into(),
                people_folder: "people".into(),
                readiness: "ready".into(),
            },
            root_dir: self.0.clone(),
            work_dir: self.0.clone(),
        })
    }
    fn set_active(&self, _selector: &str) -> anyhow::Result<ResolvedProject> {
        self.resolve(None)
    }
    fn add(
        &self,
        _path: &str,
        _name: Option<&str>,
        _inbox: Option<&str>,
    ) -> anyhow::Result<ResolvedProject> {
        self.resolve(None)
    }
}

#[derive(Default)]
struct FixtureAsr {
    offsets: Mutex<Vec<u64>>,
    fail_at: Option<u64>,
}

impl AsrBackend for FixtureAsr {
    fn backend_name(&self) -> &'static str {
        "fixture-provider"
    }

    fn transcribe(&self, request: AsrRequest) -> Result<AsrResult, TranscriptError> {
        self.offsets.lock().unwrap().push(request.session_offset_ms);
        if self.fail_at == Some(request.session_offset_ms) {
            return Err(TranscriptError {
                code: TranscriptErrorCode::InferenceFailed,
                message: "injected multipart failure".into(),
                retryable: true,
            });
        }
        let label = match request.session_offset_ms {
            0 => "opening",
            75_250 => "decision",
            185_500 => "follow-up",
            offset => panic!("unexpected segment offset {offset}"),
        };
        Ok(AsrResult {
            words: vec![
                TranscriptWord {
                    start_ms: request.session_offset_ms + 1_000,
                    end_ms: request.session_offset_ms + 1_600,
                    text: format!("{label}-a"),
                    speaker: None,
                    confidence_per_mille: Some(970),
                },
                TranscriptWord {
                    start_ms: request.session_offset_ms + 2_400,
                    end_ms: request.session_offset_ms + 3_100,
                    text: format!("{label}-b"),
                    speaker: None,
                    confidence_per_mille: Some(960),
                },
            ],
            detected_language: None,
        })
    }
}

fn seed_multipart_project(root: &std::path::Path) {
    let margins_dir = root.join(".margins");
    legacy::create_session(&margins_dir, "multi", &Local::now(), ".margins/multi.md").unwrap();
    std::fs::write(
        margins_dir.join("multi.md"),
        "[00:30] opening question\n[02:00] decision checkpoint\n[04:00] owners confirmed\n",
    )
    .unwrap();
    let offsets = [0, 75_250, 185_500];
    for ordinal in [2, 0, 1] {
        let relative = format!(".margins/multi_seg{ordinal}.wav");
        margins_media::audio::write_interleaved_wav(
            root.join(&relative),
            &vec![0.05 * (ordinal + 1) as f32; 8_000],
            16_000,
            1,
        )
        .unwrap();
        legacy::add_segment(
            &margins_dir,
            "multi",
            ordinal,
            &relative,
            offsets[ordinal as usize],
            Some(0.5),
        )
        .unwrap();
    }
    legacy::create_session(
        &margins_dir,
        "neighbor",
        &Local::now(),
        ".margins/neighbor.md",
    )
    .unwrap();
    std::fs::write(
        margins_dir.join("neighbor_transcript.json"),
        "neighbor-json",
    )
    .unwrap();
    std::fs::write(margins_dir.join("neighbor_aligned.md"), "neighbor-aligned").unwrap();
}

fn services(root: &std::path::Path, asr: Arc<dyn AsrBackend>) -> CliServices {
    let mut services = CliServices::default();
    services.projects = Arc::new(Project(root.to_path_buf()));
    services.asr = asr;
    services
}

fn invoke(
    services: &CliServices,
    invocation_dir: &std::path::Path,
    args: &[&str],
) -> (Result<(), margins_cli::CliError>, String, String) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = margins_cli::run(
        services,
        invocation_dir,
        args.iter().copied(),
        &mut stdout,
        &mut stderr,
    );
    (
        result,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

#[test]
fn realistic_multipart_processing_is_exact_once_offset_once_and_session_confined() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let invocation = temp.path().join("isolated-invocation");
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&invocation).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    seed_multipart_project(&project);
    let margins_dir = project.join(".margins");

    let outside_json = outside.join("transcript.json");
    let outside_aligned = outside.join("aligned.md");
    std::fs::write(&outside_json, "outside-json-sentinel").unwrap();
    std::fs::write(&outside_aligned, "outside-aligned-sentinel").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside_json, margins_dir.join("multi_transcript.json"))
            .unwrap();
        std::os::unix::fs::symlink(&outside_aligned, margins_dir.join("multi_aligned.md")).unwrap();
    }

    let asr = Arc::new(FixtureAsr::default());
    let provider_services = services(&project, asr.clone());
    let (result, stdout, stderr) = invoke(
        &provider_services,
        &invocation,
        &["margins", "process", "multi"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("<segments>3</segments>"));
    assert!(stdout.contains("<transcript_entries>6</transcript_entries>"));
    assert!(stdout.contains("<asr_backends>fixture-provider</asr_backends>"));
    assert_eq!(*asr.offsets.lock().unwrap(), vec![0, 75_250, 185_500]);

    let entries = margins_workflows::processing::read_transcript_entries(
        &margins_dir.join("multi_transcript.json"),
    )
    .unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| (entry.text.as_str(), entry.start_ms))
            .collect::<Vec<_>>(),
        vec![
            ("opening-a", 1_000),
            ("opening-b", 2_400),
            ("decision-a", 76_250),
            ("decision-b", 77_650),
            ("follow-up-a", 186_500),
            ("follow-up-b", 187_900),
        ]
    );
    let counts = entries.iter().fold(BTreeMap::new(), |mut counts, entry| {
        *counts.entry(entry.text.as_str()).or_insert(0) += 1;
        counts
    });
    assert!(counts.values().all(|count| *count == 1));

    let transcript_before_align = std::fs::read(margins_dir.join("multi_transcript.json")).unwrap();
    let aligned = std::fs::read_to_string(margins_dir.join("multi_aligned.md")).unwrap();
    for label in counts.keys() {
        assert_eq!(aligned.matches(label).count(), 1, "{label}");
    }
    assert_eq!(
        std::fs::read_to_string(&outside_json).unwrap(),
        "outside-json-sentinel"
    );
    assert_eq!(
        std::fs::read_to_string(&outside_aligned).unwrap(),
        "outside-aligned-sentinel"
    );
    #[cfg(unix)]
    {
        assert!(
            !std::fs::symlink_metadata(margins_dir.join("multi_transcript.json"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            !std::fs::symlink_metadata(margins_dir.join("multi_aligned.md"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
    assert_eq!(
        std::fs::read_to_string(margins_dir.join("neighbor_transcript.json")).unwrap(),
        "neighbor-json"
    );
    assert_eq!(
        std::fs::read_to_string(margins_dir.join("neighbor_aligned.md")).unwrap(),
        "neighbor-aligned"
    );

    let defaults = CliServices::default();
    let align_only_services = services(&project, defaults.asr);
    let (result, stdout, stderr) = invoke(
        &align_only_services,
        &invocation,
        &["margins", "process", "multi", "--align-only"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("<transcript_entries>6</transcript_entries>"));
    assert_eq!(*asr.offsets.lock().unwrap(), vec![0, 75_250, 185_500]);
    assert_eq!(
        std::fs::read(margins_dir.join("multi_transcript.json")).unwrap(),
        transcript_before_align
    );
    let artifacts = legacy::list_session_artifacts(&margins_dir, "multi").unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].session_name, "multi");
    assert_eq!(artifacts[0].path, ".margins/multi_aligned.md");
    assert!(legacy::list_session_artifacts(&margins_dir, "neighbor")
        .unwrap()
        .is_empty());
    assert!(!std::fs::read_dir(&margins_dir).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".margins-write-")));
}

#[test]
fn multipart_failures_preserve_existing_outputs_and_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    seed_multipart_project(&project);
    let margins_dir = project.join(".margins");
    let transcript = margins_dir.join("multi_transcript.json");
    let aligned = margins_dir.join("multi_aligned.md");
    std::fs::write(&transcript, "old-transcript").unwrap();
    std::fs::write(&aligned, "old-aligned").unwrap();
    legacy::upsert_session_artifact(
        &margins_dir,
        "multi",
        legacy::SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        ".margins/multi_aligned.md",
        "durable",
        None,
    )
    .unwrap();
    let artifact_before = legacy::list_session_artifacts(&margins_dir, "multi").unwrap();
    let assert_unchanged = || {
        assert_eq!(
            std::fs::read_to_string(&transcript).unwrap(),
            "old-transcript"
        );
        assert_eq!(std::fs::read_to_string(&aligned).unwrap(), "old-aligned");
        assert_eq!(
            legacy::list_session_artifacts(&margins_dir, "multi").unwrap(),
            artifact_before
        );
    };

    let defaults = CliServices::default();
    let align_only = services(&project, defaults.asr);
    let (result, stdout, _) = invoke(
        &align_only,
        temp.path(),
        &["margins", "process", "multi", "--align-only"],
    );
    assert_eq!(result.unwrap_err().code(), "command_failed");
    assert!(stdout.is_empty());
    assert_unchanged();

    let unavailable = CliServices::default();
    let unavailable = services(&project, unavailable.asr);
    let (result, stdout, _) = invoke(&unavailable, temp.path(), &["margins", "process", "multi"]);
    assert_eq!(result.unwrap_err().code(), "asr_unavailable");
    assert!(stdout.is_empty());
    assert_unchanged();

    let preflight_asr = Arc::new(FixtureAsr::default());
    let preflight_services = services(&project, preflight_asr.clone());
    let (result, stdout, _) = invoke(
        &preflight_services,
        temp.path(),
        &["margins", "process", "multi", "--speakers", "2"],
    );
    assert_eq!(result.unwrap_err().code(), "diarization_unavailable");
    assert!(stdout.is_empty());
    assert!(preflight_asr.offsets.lock().unwrap().is_empty());
    assert_unchanged();

    let (result, stdout, _) = invoke(
        &preflight_services,
        temp.path(),
        &["margins", "process", "../escaped"],
    );
    let error = result.unwrap_err();
    assert_eq!(error.code(), "command_failed");
    assert!(error
        .message()
        .contains("session name must be a single path component"));
    assert!(stdout.is_empty());
    assert!(preflight_asr.offsets.lock().unwrap().is_empty());
    assert!(!project.join("escaped_transcript.json").exists());
    assert!(!project.join("escaped_aligned.md").exists());
    assert_unchanged();

    let failing_asr = Arc::new(FixtureAsr {
        offsets: Mutex::new(Vec::new()),
        fail_at: Some(75_250),
    });
    let failing_services = services(&project, failing_asr.clone());
    let (result, stdout, _) = invoke(
        &failing_services,
        temp.path(),
        &["margins", "process", "multi"],
    );
    assert!(result.is_err());
    assert!(stdout.is_empty());
    assert_eq!(*failing_asr.offsets.lock().unwrap(), vec![0, 75_250]);
    assert_unchanged();
}
