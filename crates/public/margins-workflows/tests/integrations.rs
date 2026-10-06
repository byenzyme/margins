//! Integration tests for the connector control-plane store.

use anyhow::Result;
use chrono::{TimeZone, Utc};
use margins_workflows::integrations::{
    ConnectorCtx, HealthStatus, IntegrationsStore, ParticipantThread, RawItemDraft, ThreadEvidence,
};
use rusqlite::Connection;
use std::path::Path;

fn ctx(root: &Path) -> ConnectorCtx {
    ConnectorCtx {
        vault_root: root.to_path_buf(),
        connector_id: "email".to_string(),
        account: "owner@example.com".to_string(),
        command_path: None,
    }
}

#[test]
fn schema_contains_only_dedicated_authoritative_evidence_and_raw_payload_cache() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let store = IntegrationsStore::open(temp.path())?;
    let connection = Connection::open(store.db_path())?;
    let objects = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type IN ('table', 'view') ORDER BY name")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    assert!(!objects.iter().any(|name| name == "episodes"));
    assert!(!objects.iter().any(|name| name == "recall_items"));
    for expected in [
        "thread_evidence",
        "participant_threads",
        "calendar_event_evidence",
        "calendar_event_attendees",
        "external_document_evidence",
        "external_document_participants",
    ] {
        assert!(
            objects.iter().any(|name| name == expected),
            "missing {expected}"
        );
    }

    let raw_columns = connection
        .prepare("PRAGMA table_info(raw_items)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert_eq!(
        raw_columns,
        [
            "connector_id",
            "source_account",
            "source_id",
            "payload_json",
            "fetched_at",
        ]
    );
    Ok(())
}

#[test]
fn raw_cache_and_failed_run_receipt_preserve_last_coherent_evidence() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let store = IntegrationsStore::open(temp.path())?;
    let ctx = ctx(temp.path());
    let occurred_at = Utc.with_ymd_and_hms(2026, 8, 25, 10, 0, 0).unwrap();
    store.ingest_raw_connector_items(
        &ctx,
        vec![RawItemDraft {
            source_id: "thread-1".to_string(),
            payload: serde_json::json!({"transport": "cached"}),
        }],
    )?;
    store.replace_email_thread_snapshot(
        &ctx,
        vec![ThreadEvidence {
            thread_id: "thread-1".to_string(),
            occurred_from: occurred_at,
            occurred_to: occurred_at,
            body_text: "Last coherent plaintext thread".to_string(),
            href: None,
        }],
        vec![ParticipantThread {
            participant: "alice@example.com".to_string(),
            thread_id: "thread-1".to_string(),
            last_interaction: occurred_at,
            sampling_score: None,
        }],
    )?;
    store.update_health(&ctx, HealthStatus::Fresh, None)?;

    store.record_failed_reconcile(&ctx, "quota exhausted")?;

    assert_eq!(store.raw_items(&ctx)?.len(), 1);
    assert_eq!(store.thread_evidence(&ctx)?.len(), 1);
    assert_eq!(store.participant_threads(&ctx)?.len(), 1);
    assert_eq!(store.health_report(&ctx)?.status, HealthStatus::Error);
    let manifest = store
        .latest_run_manifest(&ctx.connector_id, &ctx.account)?
        .expect("failed pull run receipt");
    assert_eq!(manifest.records_written, 0);
    assert_eq!(manifest.errors, ["quota exhausted"]);
    Ok(())
}

#[test]
fn granola_retains_raw_connector_cache_without_creating_native_authority() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let store = IntegrationsStore::open(temp.path())?;
    let ctx = ConnectorCtx {
        vault_root: temp.path().to_path_buf(),
        connector_id: "granola".to_string(),
        account: "owner@example.com".to_string(),
        command_path: None,
    };

    store.ingest_raw_connector_items(
        &ctx,
        vec![RawItemDraft {
            source_id: "meeting-1".to_string(),
            payload: serde_json::json!({"upstream_shape": "meeting"}),
        }],
    )?;

    assert_eq!(store.raw_items(&ctx)?.len(), 1);
    assert!(store.external_document_evidence(&ctx)?.is_empty());
    Ok(())
}

mod retention_store {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};
    use margins_workflows::integrations::{
        apply_retention, preview_retention, CalendarEventAttendee, CalendarEventDelta,
        CalendarEventEvidence, ExternalDocumentDelta, ExternalDocumentEvidence,
        ExternalDocumentParticipant, ParticipantThread, RawItemDraft, RetentionScope,
        RetentionTarget, SurveyRange, ThreadEvidence, EMAIL_CONNECTOR_ID,
        GOOGLE_CALENDAR_CONNECTOR_ID, GOOGLE_MEET_CONNECTOR_ID,
    };
    use margins_workflows::workspace::{
        create_workspace, workspace_revision, RetentionPolicy, WorkspaceMutationError,
    };
    use rusqlite::Connection;

    fn email_ctx(state_dir: &Path) -> ConnectorCtx {
        ConnectorCtx {
            vault_root: state_dir.to_path_buf(),
            connector_id: EMAIL_CONNECTOR_ID.to_string(),
            account: "owner@example.com".to_string(),
            command_path: None,
        }
    }

    fn seed_email_fixture(store: &IntegrationsStore, ctx: &ConnectorCtx) -> anyhow::Result<()> {
        let occurred = Utc.with_ymd_and_hms(2026, 8, 20, 10, 0, 0).unwrap();
        store.replace_email_thread_snapshot(
            ctx,
            vec![
                ThreadEvidence {
                    thread_id: "active".into(),
                    occurred_from: occurred,
                    occurred_to: occurred,
                    body_text: "active thread".into(),
                    href: None,
                },
                ThreadEvidence {
                    thread_id: "tomb".into(),
                    occurred_from: occurred - Duration::days(400),
                    occurred_to: occurred - Duration::days(400),
                    body_text: "tomb thread".into(),
                    href: None,
                },
            ],
            vec![ParticipantThread {
                participant: "peer@example.com".into(),
                thread_id: "active".into(),
                last_interaction: occurred,
                sampling_score: Some(1),
            }],
        )?;
        store.ingest_raw_connector_items(
            ctx,
            vec![
                RawItemDraft {
                    source_id: "active".into(),
                    payload: serde_json::json!({"active": true}),
                },
                RawItemDraft {
                    source_id: "stale".into(),
                    payload: serde_json::json!({"stale": true}),
                },
            ],
        )?;
        store.record_failed_reconcile(ctx, "fixture failure")?;
        let connection = Connection::open(store.db_path())?;
        connection.execute(
            "UPDATE thread_evidence SET tombstoned_at = ?1 WHERE thread_id = 'tomb'",
            [(occurred - Duration::days(400)).to_rfc3339()],
        )?;
        connection.execute(
            "UPDATE raw_items SET fetched_at = ?1 WHERE source_id = 'stale'",
            [(occurred - Duration::days(400)).to_rfc3339()],
        )?;
        Ok(())
    }

    fn open_workspace(
        temp: &tempfile::TempDir,
    ) -> anyhow::Result<margins_workflows::workspace::ResolvedWorkspace> {
        let margins_home = temp.path().join("margins-home");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes)?;
        std::fs::write(notes.join("home.md"), "# Home\n")?;
        Ok(create_workspace(&margins_home, "retention", None, &notes)?)
    }

    fn target() -> RetentionTarget {
        RetentionTarget {
            connector_id: EMAIL_CONNECTOR_ID.to_string(),
            source_account: "owner@example.com".to_string(),
        }
    }

    #[test]
    fn raw_cache_scope_deletes_only_raw_items() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = open_workspace(&temp)?;
        let store = IntegrationsStore::open(&workspace.state_dir)?;
        let ctx = email_ctx(&workspace.state_dir);
        seed_email_fixture(&store, &ctx)?;
        let revision = workspace_revision(&workspace)?;
        let plan = preview_retention(&workspace, &target(), RetentionScope::RawCache)?;
        assert_eq!(plan.schema_version, "margins.retention.preview.v1");
        assert_eq!(plan.revision, revision);
        assert_eq!(plan.target.connector_id, EMAIL_CONNECTOR_ID);
        assert_eq!(plan.target.source_account, "owner@example.com");
        assert!(plan.destructive);
        assert!(!plan.index_refresh_required);
        assert_eq!(plan.counts.raw_items, 2);
        let receipt = apply_retention(&workspace, &plan, &revision, "raw-cache-1")?;
        assert_eq!(receipt.schema_version, "margins.retention.apply.v1");
        assert_eq!(store.raw_items(&ctx)?.len(), 0);
        assert_eq!(store.thread_evidence(&ctx)?.len(), 1);
        assert!(store
            .latest_run_manifest(EMAIL_CONNECTOR_ID, &ctx.account)?
            .is_some());
        assert_eq!(
            ledger_count(
                store.db_path(),
                "SELECT COUNT(*) FROM curation_observations"
            ),
            0
        );
        Ok(())
    }

    #[test]
    fn tombstones_scope_deletes_only_tombstoned_evidence() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = open_workspace(&temp)?;
        let store = IntegrationsStore::open(&workspace.state_dir)?;
        let ctx = email_ctx(&workspace.state_dir);
        seed_email_fixture(&store, &ctx)?;
        let revision = workspace_revision(&workspace)?;
        let plan = preview_retention(&workspace, &target(), RetentionScope::Tombstones)?;
        assert_eq!(plan.counts.tombstoned_evidence, 1);
        let receipt = apply_retention(&workspace, &plan, &revision, "tombstones-1")?;
        assert_eq!(receipt.deleted.tombstoned_evidence, 1);
        let connection = Connection::open(store.db_path())?;
        assert_eq!(
            connection.query_row(
                "SELECT COUNT(*) FROM thread_evidence WHERE thread_id = 'tomb'",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            0
        );
        assert_eq!(store.thread_evidence(&ctx)?.len(), 1);
        assert_eq!(store.raw_items(&ctx)?.len(), 2);
        Ok(())
    }

    #[test]
    fn materialization_scope_preserves_raw_runs_and_receipts() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = open_workspace(&temp)?;
        let store = IntegrationsStore::open(&workspace.state_dir)?;
        let ctx = email_ctx(&workspace.state_dir);
        seed_email_fixture(&store, &ctx)?;
        let runs_before = ledger_count(store.db_path(), "SELECT COUNT(*) FROM runs");
        let revision = workspace_revision(&workspace)?;
        let plan = preview_retention(&workspace, &target(), RetentionScope::Materialization)?;
        assert!(plan.index_refresh_required);
        apply_retention(&workspace, &plan, &revision, "materialization-1")?;
        assert_eq!(store.thread_evidence(&ctx)?.len(), 0);
        assert_eq!(store.participant_threads(&ctx)?.len(), 0);
        assert_eq!(store.raw_items(&ctx)?.len(), 2);
        assert_eq!(
            ledger_count(store.db_path(), "SELECT COUNT(*) FROM runs"),
            runs_before
        );
        assert_eq!(
            ledger_count(store.db_path(), "SELECT COUNT(*) FROM reconcile_receipts"),
            0
        );
        assert_eq!(
            ledger_count(store.db_path(), "SELECT COUNT(*) FROM purge_receipts"),
            1
        );
        Ok(())
    }

    #[test]
    fn all_scope_deletes_materialization_and_raw_cache() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = open_workspace(&temp)?;
        let store = IntegrationsStore::open(&workspace.state_dir)?;
        let ctx = email_ctx(&workspace.state_dir);
        seed_email_fixture(&store, &ctx)?;
        let revision = workspace_revision(&workspace)?;
        let plan = preview_retention(&workspace, &target(), RetentionScope::All)?;
        apply_retention(&workspace, &plan, &revision, "all-scope-1")?;
        assert!(store.thread_evidence(&ctx)?.is_empty());
        assert!(store.raw_items(&ctx)?.is_empty());
        Ok(())
    }

    #[test]
    fn expired_scope_respects_retention_policy_without_touching_active_evidence(
    ) -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let mut workspace = open_workspace(&temp)?;
        let margins_home = workspace.state_dir.parent().unwrap().parent().unwrap().to_path_buf();
        margins_workflows::workspace::set_workspace_retention(
            &margins_home,
            &workspace.config.id,
            &RetentionPolicy {
                raw_cache_max_age_days: Some(30),
                tombstone_max_age_days: Some(30),
            },
        )?;
        workspace = margins_workflows::workspace::resolve_state_dir(&workspace.state_dir)?;
        let store = IntegrationsStore::open(&workspace.state_dir)?;
        let ctx = email_ctx(&workspace.state_dir);
        seed_email_fixture(&store, &ctx)?;
        let revision = workspace_revision(&workspace)?;
        let plan = preview_retention(&workspace, &target(), RetentionScope::Expired)?;
        assert!(plan.cutoffs.raw_cache_before.is_some() || plan.cutoffs.tombstone_before.is_some());
        apply_retention(&workspace, &plan, &revision, "expired-1")?;
        assert_eq!(store.thread_evidence(&ctx)?.len(), 1);
        assert_eq!(store.raw_items(&ctx)?.len(), 1);
        Ok(())
    }

    #[test]
    fn preview_is_deterministic_and_apply_replays_exactly() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = open_workspace(&temp)?;
        let store = IntegrationsStore::open(&workspace.state_dir)?;
        seed_email_fixture(&store, &email_ctx(&workspace.state_dir))?;
        let revision = workspace_revision(&workspace)?;
        let first = preview_retention(&workspace, &target(), RetentionScope::RawCache)?;
        let second = preview_retention(&workspace, &target(), RetentionScope::RawCache)?;
        assert_eq!(first.plan_id, second.plan_id);
        assert_eq!(first.ledger_fingerprint, second.ledger_fingerprint);
        let applied = apply_retention(&workspace, &first, &revision, "replay-raw")?;
        assert!(!applied.replayed);
        let replay = apply_retention(&workspace, &first, &revision, "replay-raw")?;
        assert!(replay.replayed);
        assert_eq!(applied.deleted, replay.deleted);
        Ok(())
    }

    #[test]
    fn apply_rejects_changed_request_hash_and_stale_plan_or_revision() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = open_workspace(&temp)?;
        let store = IntegrationsStore::open(&workspace.state_dir)?;
        seed_email_fixture(&store, &email_ctx(&workspace.state_dir))?;
        let revision = workspace_revision(&workspace)?;
        let plan = preview_retention(&workspace, &target(), RetentionScope::RawCache)?;
        apply_retention(&workspace, &plan, &revision, "conflict-id")?;
        let hash_conflict = apply_retention(
            &workspace,
            &preview_retention(&workspace, &target(), RetentionScope::All)?,
            &revision,
            "conflict-id",
        )
        .expect_err("same request id with a different hash must conflict");
        assert!(hash_conflict
            .downcast_ref::<WorkspaceMutationError>()
            .is_some_and(|error| matches!(
                error,
                WorkspaceMutationError::IdempotencyConflict { .. }
            )));
        let invalid_revision = apply_retention(
            &workspace,
            &plan,
            "0000000000000000000000000000000000000000",
            "invalid-revision",
        )
        .expect_err("--if-revision must match the preview revision");
        assert!(invalid_revision
            .downcast_ref::<margins_workflows::integrations::RetentionMutationError>()
            .is_some_and(|error| matches!(
                error,
                margins_workflows::integrations::RetentionMutationError::InvalidPlan(_)
            )));
        store.ingest_raw_connector_items(
            &email_ctx(&workspace.state_dir),
            vec![RawItemDraft {
                source_id: "fresh".into(),
                payload: serde_json::json!({"fresh": true}),
            }],
        )?;
        let stale_plan = apply_retention(&workspace, &plan, &revision, "stale-plan")
            .expect_err("ledger drift after preview must stale the plan");
        assert!(stale_plan
            .downcast_ref::<margins_workflows::integrations::RetentionMutationError>()
            .is_some_and(|error| matches!(
                error,
                margins_workflows::integrations::RetentionMutationError::PlanStale { .. }
            )));
        let current_plan = preview_retention(&workspace, &target(), RetentionScope::RawCache)?;
        let mut changed = workspace.clone();
        let mut policy = changed.config.policy.clone();
        policy.excluded_folders.push("revision-changed-after-preview".to_string());
        margins_workflows::workspace::update_policy(&mut changed, policy)?;
        let revision_conflict = apply_retention(
            &workspace,
            &current_plan,
            &current_plan.revision,
            "stale-revision",
        )
        .expect_err("changed workspace config must conflict");
        assert!(revision_conflict
            .downcast_ref::<WorkspaceMutationError>()
            .is_some_and(|error| matches!(error, WorkspaceMutationError::RevisionConflict { .. })));
        Ok(())
    }

    #[test]
    fn removed_binding_target_remains_purgeable() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let mut workspace = open_workspace(&temp)?;
        let store = IntegrationsStore::open(&workspace.state_dir)?;
        let ctx = ConnectorCtx {
            vault_root: workspace.state_dir.clone(),
            connector_id: GOOGLE_MEET_CONNECTOR_ID.to_string(),
            account: "owner@margins.test".to_string(),
            command_path: None,
        };
        let occurred = Utc.with_ymd_and_hms(2026, 8, 20, 10, 0, 0).unwrap();
        store.apply_external_document_delta(
            &ctx,
            ExternalDocumentDelta {
                documents: vec![ExternalDocumentEvidence {
                    source_id: "doc-alpha".into(),
                    occurred_at: occurred,
                    title: "Meet".into(),
                    body_text: "Meet body".into(),
                    href: None,
                    attributes: serde_json::json!({}),
                }],
                participants: vec![ExternalDocumentParticipant {
                    source_id: "doc-alpha".into(),
                    participant_key: "email:alice@example.com".into(),
                    position: 0,
                    display_name: "Alice".into(),
                    email: Some("alice@example.com".into()),
                    ambiguous: false,
                }],
                tombstone_source_ids: Vec::new(),
                raw_items: Vec::new(),
                snapshot_scope: None,
                complete_snapshot: false,
                materialization_fingerprint: "meet-fixture".into(),
                next_cursor: None,
            },
            None,
        )?;
        margins_workflows::workspace::add_source(
            &mut workspace,
            "meet",
            margins_workflows::workspace::WorkspaceBinding::GoogleMeet {
                account: "owner@margins.test".to_string(),
            },
        )?;
        margins_workflows::workspace::remove_source(&mut workspace, "meet")?;
        let target = RetentionTarget {
            connector_id: GOOGLE_MEET_CONNECTOR_ID.to_string(),
            source_account: "owner@margins.test".to_string(),
        };
        let revision = workspace_revision(&workspace)?;
        let plan = preview_retention(&workspace, &target, RetentionScope::Materialization)?;
        assert_eq!(plan.counts.active_evidence, 1);
        apply_retention(&workspace, &plan, &revision, "removed-binding")?;
        assert!(store.external_document_evidence(&ctx)?.is_empty());
        Ok(())
    }

    #[test]
    fn calendar_and_shared_external_layouts_delete_only_the_exact_target() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = open_workspace(&temp)?;
        let store = IntegrationsStore::open(&workspace.state_dir)?;
        let occurred = Utc.with_ymd_and_hms(2026, 8, 20, 10, 0, 0).unwrap();
        let calendar_ctx = ConnectorCtx {
            vault_root: workspace.state_dir.clone(),
            connector_id: GOOGLE_CALENDAR_CONNECTOR_ID.to_string(),
            account: "owner@example.com".to_string(),
            command_path: None,
        };
        store.apply_calendar_event_delta(
            &calendar_ctx,
            CalendarEventDelta {
                events: vec![CalendarEventEvidence {
                    source_id: "primary:event-1".into(),
                    calendar_id: "primary".into(),
                    occurred_from: occurred,
                    occurred_to: Some(occurred + Duration::hours(1)),
                    title: "Calendar authority".into(),
                    body_text: "Calendar body".into(),
                    href: None,
                }],
                attendees: vec![CalendarEventAttendee {
                    source_id: "primary:event-1".into(),
                    attendee_key: "peer@example.com".into(),
                    position: 0,
                    display_name: "Peer".into(),
                    email: Some("peer@example.com".into()),
                    response_status: None,
                    is_self: false,
                    organizer: false,
                }],
                tombstone_source_ids: Vec::new(),
                raw_items: vec![RawItemDraft {
                    source_id: "primary:event-1".into(),
                    payload: serde_json::json!({"calendar": true}),
                }],
                scope: SurveyRange {
                    occurred_from: occurred - Duration::days(1),
                    occurred_to: occurred + Duration::days(1),
                },
                complete_snapshot: false,
                materialization_fingerprint: "calendar-test-v1".into(),
                next_cursor: None,
            },
            None,
        )?;
        let calendar_target = RetentionTarget {
            connector_id: GOOGLE_CALENDAR_CONNECTOR_ID.to_string(),
            source_account: calendar_ctx.account.clone(),
        };
        let calendar_plan = preview_retention(&workspace, &calendar_target, RetentionScope::All)?;
        assert_eq!(calendar_plan.counts.active_evidence, 1);
        assert_eq!(calendar_plan.counts.associations, 1);
        assert_eq!(calendar_plan.counts.raw_items, 1);
        apply_retention(
            &workspace,
            &calendar_plan,
            &calendar_plan.revision,
            "calendar-all",
        )?;
        assert!(store.calendar_event_evidence(&calendar_ctx)?.is_empty());
        assert!(store.calendar_event_attendees(&calendar_ctx)?.is_empty());
        assert!(store.raw_items(&calendar_ctx)?.is_empty());

        let external_delta = |source_id: &str, title: &str| ExternalDocumentDelta {
            documents: vec![ExternalDocumentEvidence {
                source_id: source_id.into(),
                occurred_at: occurred,
                title: title.into(),
                body_text: format!("{title} body"),
                href: None,
                attributes: serde_json::json!({}),
            }],
            participants: vec![ExternalDocumentParticipant {
                source_id: source_id.into(),
                participant_key: "peer@example.com".into(),
                position: 0,
                display_name: "Peer".into(),
                email: Some("peer@example.com".into()),
                ambiguous: false,
            }],
            tombstone_source_ids: Vec::new(),
            raw_items: Vec::new(),
            snapshot_scope: None,
            complete_snapshot: false,
            materialization_fingerprint: "external-test-v1".into(),
            next_cursor: None,
        };
        let meet_ctx = ConnectorCtx {
            vault_root: workspace.state_dir.clone(),
            connector_id: GOOGLE_MEET_CONNECTOR_ID.to_string(),
            account: "owner@example.com".to_string(),
            command_path: None,
        };
        let granola_ctx = ConnectorCtx {
            vault_root: workspace.state_dir.clone(),
            connector_id: "granola".to_string(),
            account: "owner@example.com".to_string(),
            command_path: None,
        };
        store.apply_external_document_delta(&meet_ctx, external_delta("meet-doc", "Meet"), None)?;
        store.apply_external_document_delta(
            &granola_ctx,
            external_delta("granola-doc", "Granola"),
            None,
        )?;
        let granola_target = RetentionTarget {
            connector_id: "granola".to_string(),
            source_account: granola_ctx.account.clone(),
        };
        let granola_plan =
            preview_retention(&workspace, &granola_target, RetentionScope::Materialization)?;
        assert_eq!(granola_plan.counts.active_evidence, 1);
        assert_eq!(granola_plan.counts.associations, 1);
        apply_retention(
            &workspace,
            &granola_plan,
            &granola_plan.revision,
            "granola-materialization",
        )?;
        assert!(store.external_document_evidence(&granola_ctx)?.is_empty());
        assert!(store
            .external_document_participants(&granola_ctx)?
            .is_empty());
        assert_eq!(store.external_document_evidence(&meet_ctx)?.len(), 1);
        assert_eq!(store.external_document_participants(&meet_ctx)?.len(), 1);
        Ok(())
    }

    fn ledger_count(path: &Path, query: &str) -> i64 {
        Connection::open(path)
            .unwrap()
            .query_row(query, [], |row| row.get(0))
            .unwrap()
    }
}
