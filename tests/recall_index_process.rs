#![cfg(feature = "recall")]

#[path = "support/fixture_generator.rs"]
mod fixture_generator;

/// libsql configures SQLite's process-global threading mode. Keep this real
/// index-opening assertion in its own integration-test process so unrelated
/// rusqlite-backed unit tests cannot initialize SQLite first and poison it.
#[test]
fn exact_phrase_recall_returns_actual_source_path() {
    let _guard = env_lock().lock().unwrap();
    margins::initialize_sqlite_runtime().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let margins_home = tmp.path().join("margins-home");
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(margins_home.as_path()).unwrap();
    std::fs::create_dir_all(vault.join("projects")).unwrap();
    std::fs::write(
        vault.join("projects/source.md"),
        "# Source\nExact literal recall rescue: keeps the real project source visible.",
    )
    .unwrap();
    for i in 0..4 {
        std::fs::write(
            vault.join(format!("evidence-{i}.md")),
            format!(
                "# Project evidence {i}\n\n[[Projects]] background material about the adjacent planning topic \
                 covers discovery, preparation, implementation, measurement, review, ownership, \
                 coordination, decisions, constraints, risks, milestones, outcomes, priorities, \
                 tradeoffs, commitments, dependencies, timelines, experiments, feedback, \
                 documentation, validation, maintenance, communication, and delivery. This is \
                 independent substantive evidence record {i} for the selected project folder."
            ),
        )
        .unwrap();
    }
    let workspace =
        margins_workflows::workspace::create_workspace(&margins_home, "recall-test", None, &vault)
            .unwrap();
    std::env::set_var("MARGINS_HOME", &margins_home);
    std::env::remove_var("ENZYME_HOME");
    let _generator = fixture_generator::FixtureGenerator::start(&margins_home);

    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    std::fs::create_dir_all(vault.join("unrelated")).unwrap();
    std::fs::write(
        vault.join("unrelated/created-after-index.md"),
        "# Later\nThis file proves recall lookup does not walk unrelated Markdown.",
    )
    .unwrap();
    let out = margins::recall::recall(
        &workspace,
        "Exact literal recall rescue keeps the real project source visible",
        None,
    )
    .unwrap();

    assert_eq!(out.status, "ok");
    let source_hit = out
        .results
        .iter()
        .find(|result| result.document_ref.ends_with("/projects/source.md"))
        .expect("literal retrieval must retain the exact project source alongside catalysts");
    assert_eq!(source_hit.source, "home");
    assert_eq!(
        source_hit.evidence,
        margins_workflows::integrations::EvidenceHandle::NativeMarkdown {
            path: workspace
                .home_dir
                .join("projects/source.md")
                .to_string_lossy()
                .into_owned(),
        }
    );
    assert_eq!(
        out.freshness.index.reason.as_deref(),
        Some("snapshot_freshness_unknown")
    );
    assert!(!out.freshness.stale);
}

#[test]
fn recall_without_selected_entities_still_searches_declared_notes() {
    let _guard = env_lock().lock().unwrap();
    margins::initialize_sqlite_runtime().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let margins_home = tmp.path().join("margins-home");
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::write(
        vault.join("interview.md"),
        "Devon needs concise handoff summaries with discoverable source calls.",
    )
    .unwrap();
    std::fs::write(
        vault.join("decision.md"),
        "Trailhead decided on one handoff summary per customer call.",
    )
    .unwrap();
    std::fs::write(
        vault.join("questions.md"),
        "Open question: should incomplete interviews show a confidence level?",
    )
    .unwrap();
    let workspace = margins_workflows::workspace::create_workspace(
        &margins_home,
        "recall-without-links",
        None,
        &vault,
    )
    .unwrap();
    std::env::set_var("MARGINS_HOME", &margins_home);
    std::env::remove_var("ENZYME_HOME");
    let _generator = fixture_generator::FixtureGenerator::start(&margins_home);

    let init = margins::recall::provision_workspace_for_init(&workspace).unwrap();
    assert_eq!(init.readiness.entities_curated, 0);
    let out = margins::recall::recall(
        &workspace,
        "What supports the handoff decision and what remains unresolved?",
        None,
    )
    .unwrap();
    assert_eq!(out.status, "ok");
    assert_eq!(out.reason, "no_entities");
    assert_eq!(out.search_strategy, "direct");
    assert!(out
        .results
        .iter()
        .any(|hit| hit.document_ref.ends_with("/decision.md")));
    assert!(out
        .results
        .iter()
        .any(|hit| hit.document_ref.ends_with("/questions.md")));

    let handle = margins::recall::open_workspace(&workspace).unwrap().unwrap();
    let in_process: serde_json::Value = serde_json::from_str(
        &handle
            .catalyze_json("What supports the handoff decision?", 5)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(in_process["search_strategy"], "direct");
    assert!(in_process["results"].as_array().unwrap().iter().any(|hit| {
        hit["document_ref"]
            .as_str()
            .is_some_and(|path| path.ends_with("/decision.md"))
    }));

    let exact = margins::recall::recall(
        &workspace,
        "Devon needs concise handoff summaries with discoverable source calls",
        Some("home"),
    )
    .unwrap();
    assert_eq!(exact.status, "ok");
    assert!(exact.results[0].document_ref.ends_with("/interview.md"));
}

#[test]
fn recall_missing_snapshot_does_not_create_or_migrate_state() {
    let _guard = env_lock().lock().unwrap();
    margins::initialize_sqlite_runtime().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let margins_home = tmp.path().join("margins-home");
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::write(vault.join("note.md"), "# Note").unwrap();
    let workspace =
        margins_workflows::workspace::create_workspace(&margins_home, "recall-empty", None, &vault)
            .unwrap();
    std::env::set_var("MARGINS_HOME", &margins_home);
    std::env::remove_var("ENZYME_HOME");

    let out = margins::recall::recall(&workspace, "anything", None).unwrap();

    assert_eq!(out.status, "unavailable");
    assert_eq!(out.reason, "not_established");
    assert!(!workspace.recall_path().exists());
}

fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
