use chrono::Local;
use margins_store::canonical;
use std::path::Path;

#[path = "fixture_generator.rs"]
mod fixture_generator;

pub fn exercise_store(root: &Path, session_name: &str) {
    let margins_dir = root.join("margins-store");
    canonical::create_session(&margins_dir, session_name, &Local::now(), "session-note.md").unwrap();
    canonical::add_segment(&margins_dir, session_name, 0, "segment.wav", 0, Some(1.25)).unwrap();

    let stored = canonical::get_session_meta(&margins_dir, session_name).unwrap();
    assert_eq!(stored.name, session_name);
    assert_eq!(stored.segments.len(), 1);
    assert_eq!(stored.segments[0].wav_path, "segment.wav");
    let listed = canonical::list_sessions(&margins_dir).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, session_name);
    assert_eq!(listed[0].segment_count, 1);
}

pub fn exercise_recall(root: &Path) {
    let margins_home = root.join("margins-home");
    let vault = root.join("vault");
    std::fs::create_dir_all(&margins_home).unwrap();
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::create_dir_all(vault.join("projects")).unwrap();
    std::fs::write(
        vault.join("projects/source.md"),
        "# Source\nSQLite runtime product order keeps this exact recall source visible.",
    )
    .unwrap();
    for index in 0..4 {
        std::fs::write(
            vault.join(format!("evidence-{index}.md")),
            format!(
                "# Project evidence {index}\n\n[[Projects]] adjacent planning material covers \
                 discovery, preparation, implementation, measurement, review, ownership, \
                 coordination, decisions, constraints, risks, milestones, outcomes, priorities, \
                 tradeoffs, commitments, dependencies, timelines, experiments, feedback, \
                 documentation, validation, maintenance, communication, and delivery. This is \
                 independent substantive evidence record {index} for the selected project folder."
            ),
        )
        .unwrap();
    }
    let workspace =
        margins_workflows::workspace::create_workspace(&margins_home, "runtime-test", None, &vault)
            .unwrap();
    std::env::set_var("MARGINS_HOME", &margins_home);
    let _generator = fixture_generator::FixtureGenerator::start(&margins_home);

    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    let result = margins::recall::recall(
        &workspace,
        "SQLite runtime product order keeps this exact recall source visible",
        None,
    )
    .unwrap();
    assert_eq!(result.status, "ok");
    assert!(
        result
            .results
            .iter()
            .any(|hit| hit.document_ref.ends_with("/projects/source.md")),
        "literal retrieval must retain the exact project source alongside catalysts"
    );
}
