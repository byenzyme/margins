use margins_desktop::pi_distill::{run_pi_distill_blocking, NoteConfig, PiDistillRequest};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let work_dir =
        std::env::temp_dir().join(format!("margins-pi-distill-smoke-{}", std::process::id()));
    let margins_dir = work_dir.join(".margins");
    let trace_dir = margins_dir.clone();
    std::fs::create_dir_all(&margins_dir)?;

    let memo_path = work_dir.join("smoke.md");
    let aligned_path = margins_dir.join("smoke_aligned.md");
    std::fs::write(&memo_path, "[00:01] desktop should use Pi Rust SDK and Enzyme\n[00:04 ~00:05] final note needs a feedback loop\n")?;
    std::fs::write(
        &aligned_path,
        "# Aligned Timeline\n\n[00:01] memo: desktop should use Pi Rust SDK and Enzyme\n[00:02] ch0: We should avoid a separate Python synthesis path and reuse Pi login, especially Codex OAuth.\n[00:04] memo: final note needs a feedback loop\n[00:05] ch1: The Distill tab can show Enzyme connections and then accept user feedback.\n",
    )?;

    let vault = if repo.join(".enzyme").exists() {
        Some(repo.clone())
    } else {
        None
    };
    let aligned_context = std::fs::read_to_string(&aligned_path)?;
    let outcome = run_pi_distill_blocking(
        PiDistillRequest {
            work_dir: work_dir.clone(),
            margins_dir,
            trace_dir,
            session_name: "smoke".to_string(),
            memo_path,
            capture_context: String::new(),
            aligned_context,
            vault_path: vault,
            note_config: NoteConfig::default(),
            ai_provider: None,
            ai_model: None,
            ai_api_key: None,
            ai_credential_generation: None,
            prep_ai_provider: None,
            prep_ai_model: None,
            prep_ai_api_key: None,
            prep_ai_credential_generation: None,
            skill_path: repo.join("skills/margins/hosts/desktop.md"),
            cancel: Arc::new(AtomicBool::new(false)),
            resume_session_path: None,
            refine_message: None,
            existing_note_path: None,
            save_generated_note: true,
        },
        |stage, msg, progress| eprintln!("[{stage}] {progress:?} {msg}"),
    )?;

    println!("note_path={}", outcome.note_path.unwrap_or_default());
    Ok(())
}
