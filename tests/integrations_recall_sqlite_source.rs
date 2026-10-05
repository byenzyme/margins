#![cfg(feature = "recall")]

#[path = "support/fixture_generator.rs"]
mod fixture_generator;

use chrono::{TimeZone, Utc};
use margins_workflows::integrations::{
    CalendarEventAttendee, CalendarEventDelta, CalendarEventEvidence, ConnectorCtx, EvidenceHandle,
    GoogleCalendarScope, HealthStatus, IntegrationsStore, ParticipantThread, ThreadEvidence,
    EMAIL_CONNECTOR_ID, GOOGLE_CALENDAR_CONNECTOR_ID,
};
use margins_workflows::workspace::{
    calendar_collection_namespace, gmail_collection_namespace,
    native_markdown_collection_namespace, CalendarCollectionSelector, GmailCollectionSelector,
    SourceKind, SourceRole, WorkspaceBinding, WorkspaceEntity, WorkspaceEntityOptions,
};
use std::collections::BTreeSet;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn assert_stale_recall(
    workspace: &margins_workflows::workspace::ResolvedWorkspace,
    query: &str,
    binding: &str,
    reason: &str,
) {
    let error = match margins::recall::recall(workspace, query, None) {
        Err(error) => error,
        Ok(_) => panic!("stale materialization must block semantic retrieval"),
    };
    assert!(error.to_string().contains("recall_unavailable_stale_materialization"));
    let sources = margins::recall::workspace_source_refresh_staleness(workspace).unwrap();
    assert!(sources[binding].stale);
    assert_eq!(sources[binding].stale_reason.as_deref(), Some(reason));
}

#[test]
fn materialized_calendar_events_keep_stable_refs_and_multi_attendee_occurrences() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    recall_engine::initialize_sqlite_runtime().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path().join("vault");
    let margins_home = temp.path().join("margins-home");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::create_dir_all(&margins_home).unwrap();
    std::fs::write(vault.join("home.md"), "# Home\n\nCalendar fixture home.").unwrap();

    let mut workspace = margins_workflows::workspace::create_workspace(
        &margins_home,
        "calendar-test",
        None,
        &vault,
    )
    .unwrap();
    let selector = CalendarCollectionSelector::default_declaration();
    margins_workflows::workspace::add_source(
        &mut workspace,
        "calendar",
        WorkspaceBinding::GoogleCalendar {
            account: "owner@example.com".to_string(),
            calendar: selector.clone(),
        },
    )
    .unwrap();
    workspace.config.policy.entities = vec![
        WorkspaceEntity::simple("[[grace@example.com]]"),
        WorkspaceEntity::simple("[[alex@example.com]]"),
    ];

    let store = IntegrationsStore::open(&workspace.state_dir).unwrap();
    let ctx = ConnectorCtx {
        vault_root: workspace.state_dir.clone(),
        connector_id: GOOGLE_CALENDAR_CONNECTOR_ID.to_string(),
        account: "owner@example.com".to_string(),
        command_path: None,
    };
    let anchor = Utc::now();
    let scope = GoogleCalendarScope::for_selector(&selector, anchor)
        .unwrap()
        .as_range();
    let mut events = Vec::new();
    let mut attendees = Vec::new();
    for index in 0..4u32 {
        let source_id = format!("primary:checkpoint-{index}");
        let occurred_from = anchor + chrono::Duration::hours(i64::from(index + 1));
        events.push(CalendarEventEvidence {
            source_id: source_id.clone(),
            calendar_id: "primary".to_string(),
            occurred_from,
            occurred_to: Some(occurred_from + chrono::Duration::minutes(30)),
            title: format!("Calendar checkpoint {index}"),
            body_text: format!(
                "Calendar checkpoint {index} covers team plan goal risk time data scope cost owner test ship learn track align decide build review adapt share draft check facts note gaps ask peers solve bugs prove value guard launch."
            ),
            href: Some(format!("https://calendar.google.com/event?eid=checkpoint-{index}")),
        });
        for (position, email) in ["grace@example.com", "alex@example.com"]
            .into_iter()
            .enumerate()
        {
            attendees.push(CalendarEventAttendee {
                source_id: source_id.clone(),
                attendee_key: format!("email:{email}"),
                position: u32::try_from(position).unwrap(),
                display_name: email.to_string(),
                email: Some(email.to_string()),
                response_status: Some("accepted".to_string()),
                is_self: false,
                organizer: false,
            });
        }
    }
    store
        .apply_calendar_event_delta(
            &ctx,
            CalendarEventDelta {
                events,
                attendees,
                tombstone_source_ids: Vec::new(),
                raw_items: Vec::new(),
                scope,
                complete_snapshot: true,
                materialization_fingerprint: selector.materialization_fingerprint().unwrap(),
                next_cursor: None,
            },
            None,
        )
        .unwrap();

    std::env::set_var("MARGINS_HOME", &margins_home);
    std::env::remove_var("ENZYME_HOME");
    let generator = fixture_generator::FixtureGenerator::start(&margins_home);
    margins::recall::provision_workspace_for_init(&workspace).unwrap();

    let namespace = calendar_collection_namespace("owner@example.com").unwrap();
    let prefix = format!("sqlite:{namespace}/");
    let recall = margins::recall::recall(&workspace, "Calendar checkpoint", None).unwrap();
    let calendar_hit = recall
        .results
        .iter()
        .find(|result| result.source_kind == SourceKind::GoogleCalendar)
        .expect("Calendar recall returns authoritative external evidence");
    assert!(calendar_hit.document_ref.starts_with(&prefix));
    assert!(matches!(
        &calendar_hit.evidence,
        EvidenceHandle::ExternalRecord {
            connector_id,
            source_account,
            source_id,
            href: Some(href),
        } if connector_id == "gcal"
            && source_account == "owner@example.com"
            && source_id.starts_with("primary:checkpoint-")
            && href.starts_with("https://calendar.google.com/")
    ));

    store
        .record_failed_reconcile(&ctx, "simulated Calendar refresh failure")
        .unwrap();
    assert_stale_recall(&workspace, "Calendar checkpoint", "calendar", "refresh_failed");
    store
        .update_health(&ctx, HealthStatus::Fresh, None)
        .unwrap();

    let index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    let hashes_before = index
        .prepare("SELECT source_ref, content_hash FROM docs WHERE source_ref LIKE ?1 ORDER BY source_ref")
        .unwrap()
        .query_map([format!("{prefix}%")], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(hashes_before.len(), 4);
    let occurrences = index
        .prepare(
            "SELECT e.name, eo.file_path
             FROM entities e JOIN entity_occurrences eo ON eo.entity_id = e.id
             WHERE e.name IN ('grace@example.com', 'alex@example.com')
               AND eo.file_path LIKE ?1 ORDER BY e.name, eo.file_path",
        )
        .unwrap()
        .query_map([format!("{prefix}%")], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let grace = occurrences
        .iter()
        .filter(|(name, _)| name == "grace@example.com")
        .map(|(_, path)| path)
        .collect::<BTreeSet<_>>();
    let alex = occurrences
        .iter()
        .filter(|(name, _)| name == "alex@example.com")
        .map(|(_, path)| path)
        .collect::<BTreeSet<_>>();
    assert_eq!(grace, alex);
    assert_eq!(grace.len(), 4);
    drop(index);

    let binding = margins_workflows::workspace::remove_source(&mut workspace, "calendar").unwrap();
    margins_workflows::workspace::add_source(&mut workspace, "renamed-calendar", binding).unwrap();
    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    let renamed_index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    let hashes_after = renamed_index
        .prepare("SELECT source_ref, content_hash FROM docs WHERE source_ref LIKE ?1 ORDER BY source_ref")
        .unwrap()
        .query_map([format!("{prefix}%")], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(hashes_after, hashes_before);
    drop(renamed_index);
    assert_eq!(generator.request_count(), 2);

    margins_workflows::workspace::remove_source(&mut workspace, "renamed-calendar").unwrap();
    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    let removed_index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    let remaining: i64 = removed_index
        .query_row(
            "SELECT COUNT(*) FROM docs WHERE source_ref LIKE ?1",
            [format!("{prefix}%")],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0, "binding removal excludes retained evidence");
}

/// Mail is one complete document per thread. Every associated participant sees
/// that same document through Enzyme's ordinary shared-document context path.
#[test]
fn materialized_mail_threads_use_generic_shared_document_context() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    recall_engine::initialize_sqlite_runtime().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path().join("vault");
    let margins_home = temp.path().join("margins-home");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::create_dir_all(&margins_home).unwrap();
    std::fs::write(vault.join("home.md"), "# Home\n\nMailbox fixture home.").unwrap();

    let mut workspace =
        margins_workflows::workspace::create_workspace(&margins_home, "mail-test", None, &vault)
            .unwrap();
    let initial_selector = GmailCollectionSelector::default_declaration();
    margins_workflows::workspace::add_source(
        &mut workspace,
        "mail",
        WorkspaceBinding::Gmail {
            account: "owner@example.com".to_string(),
            gmail: initial_selector.clone(),
        },
    )
    .unwrap();
    let gmail_source = gmail_collection_namespace("owner@example.com").unwrap();
    let gmail_document_prefix = format!("sqlite:{gmail_source}/");

    let store = IntegrationsStore::open(&workspace.state_dir).unwrap();
    let ctx = ConnectorCtx {
        vault_root: workspace.state_dir.clone(),
        connector_id: EMAIL_CONNECTOR_ID.to_string(),
        account: "owner@example.com".to_string(),
        command_path: None,
    };
    let occurred_from = Utc.with_ymd_and_hms(2026, 8, 20, 10, 0, 0).unwrap();
    let occurred_to = Utc.with_ymd_and_hms(2026, 8, 20, 10, 20, 0).unwrap();
    let mut threads = Vec::new();
    let mut associations = Vec::new();
    for index in 0..4 {
        let thread_id = format!("shared-{index}");
        threads.push(ThreadEvidence {
            thread_id: thread_id.clone(),
            occurred_from,
            occurred_to,
            body_text: format!(
                "----- Message 1 · 2026-08-20 10:00:00 UTC -----\n\n\
                 Alice proposes checkpoint {index}: team plan goal risk time data scope cost owner \
                 test ship learn track align decide build review adapt.\n\n\
                 ----- Message 2 · 2026-08-20 10:20:00 UTC -----\n\n\
                 Bob responds to checkpoint {index}: share draft check facts note gaps ask peers \
                 solve bugs prove value guard launch serve users close tasks."
            ),
            href: Some(format!(
                "https://mail.google.com/mail/u/owner@example.com/#all/{thread_id}"
            )),
        });
        for participant in ["alice@acme.test", "bob@partner.test"] {
            associations.push(ParticipantThread {
                participant: participant.to_string(),
                thread_id: thread_id.clone(),
                last_interaction: occurred_to,
                sampling_score: Some(100 - index),
            });
        }
    }
    store
        .replace_email_thread_snapshot_with_materialization_fingerprint(
            &ctx,
            threads.clone(),
            associations.clone(),
            &initial_selector.materialization_fingerprint().unwrap(),
        )
        .unwrap();
    store
        .update_health(&ctx, HealthStatus::Fresh, None)
        .unwrap();

    std::env::set_var("MARGINS_HOME", &margins_home);
    std::env::remove_var("ENZYME_HOME");
    let generator = fixture_generator::FixtureGenerator::start(&margins_home);
    margins::recall::provision_workspace_for_init(&workspace).unwrap();

    let recall = margins::recall::recall(&workspace, "Alice proposes checkpoint", None).unwrap();
    assert_eq!(
        recall.freshness.status,
        margins_workflows::integrations::FreshnessStatus::Fresh
    );
    let mail_hit = recall
        .results
        .iter()
        .find(|result| result.source_kind == SourceKind::GoogleMail)
        .expect("mail recall returns an authoritative external handle");
    assert!(mail_hit.document_ref.starts_with(&gmail_document_prefix));
    assert!(matches!(
        &mail_hit.evidence,
        EvidenceHandle::ExternalRecord {
            connector_id,
            source_account,
            source_id,
            href: Some(href),
        } if connector_id == "email"
            && source_account == "owner@example.com"
            && source_id.starts_with("shared-")
            && href.starts_with("https://mail.google.com/")
    ));
    let json: serde_json::Value =
        serde_json::from_str(&margins::recall::render_recall_json(&recall).unwrap()).unwrap();
    assert!(json["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|result| result.get("file_path").is_none()));

    store
        .record_failed_reconcile(&ctx, "simulated Gmail refresh failure")
        .unwrap();
    assert_stale_recall(&workspace, "Alice proposes checkpoint", "mail", "refresh_failed");
    store
        .update_health(&ctx, HealthStatus::Fresh, None)
        .unwrap();

    let index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    let mail_docs = index
        .prepare(&format!(
            "SELECT source_ref, content FROM docs
             WHERE source_ref LIKE 'sqlite:{gmail_source}/%' ORDER BY source_ref"
        ))
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        mail_docs.len(),
        4,
        "one indexed document must represent each thread"
    );
    assert!(mail_docs.iter().all(|(_, body)| {
        body.contains("Alice proposes")
            && body.contains("Bob responds")
            && body.matches("----- Message ").count() == 2
    }));

    let occurrences = index
        .prepare(&format!(
            "SELECT e.name, eo.file_path
             FROM entities e JOIN entity_occurrences eo ON eo.entity_id = e.id
             WHERE e.name IN ('alice@acme.test', 'bob@partner.test')
               AND eo.file_path LIKE 'sqlite:{gmail_source}/%'
             ORDER BY e.name, eo.file_path"
        ))
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let alice_paths = occurrences
        .iter()
        .filter(|(name, _)| name == "alice@acme.test")
        .map(|(_, path)| path.clone())
        .collect::<BTreeSet<_>>();
    let bob_paths = occurrences
        .iter()
        .filter(|(name, _)| name == "bob@partner.test")
        .map(|(_, path)| path.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(alice_paths, bob_paths);
    assert_eq!(alice_paths.len(), 4);

    let catalysts = index
        .prepare(
            "SELECT entity, text FROM catalysts
             WHERE entity IN ('alice@acme.test', 'bob@partner.test') ORDER BY entity, id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let skips = index
        .prepare(
            "SELECT entity, reason_code, distinct_documents, substantive_tokens
             FROM catalyst_generation_skips ORDER BY entity, id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        !catalysts.is_empty(),
        "generic catalyst generation skipped shared evidence: requests={}, skips={skips:?}",
        generator.request_count()
    );
    assert_eq!(generator.request_count(), 2);
    for (entity, text) in catalysts {
        let parsed = recall_engine::models::parse_catalyst_text(&text).unwrap();
        assert!(parsed.hypothesis.to_ascii_lowercase().contains(&entity));
        assert!(!parsed.receipts.is_empty());
        assert!(parsed.receipts.iter().all(|receipt| {
            alice_paths.contains(&receipt.source_ref) && receipt.quote.contains("Alice proposes")
        }));
    }
    let hashes_before = index
        .prepare(&format!(
            "SELECT source_ref, content_hash FROM docs
             WHERE source_ref LIKE 'sqlite:{gmail_source}/%' ORDER BY source_ref"
        ))
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    drop(index);

    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    assert_eq!(
        generator.request_count(),
        2,
        "an unchanged snapshot reuses Enzyme document and prompt hashes"
    );

    let removed = margins_workflows::workspace::remove_source(&mut workspace, "mail").unwrap();
    let account = match removed {
        WorkspaceBinding::Gmail { account, .. } => account,
        _ => unreachable!("mail binding changed kind"),
    };
    margins_workflows::workspace::add_source(
        &mut workspace,
        "renamed-mail",
        WorkspaceBinding::Gmail {
            account,
            gmail: initial_selector.clone(),
        },
    )
    .unwrap();
    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    assert_eq!(
        generator.request_count(),
        2,
        "binding display-name changes must preserve Gmail document and prompt hashes"
    );
    let display_rename =
        margins::recall::recall(&workspace, "Alice proposes checkpoint", None).unwrap();
    assert_eq!(
        display_rename.freshness.status,
        margins_workflows::integrations::FreshnessStatus::Fresh,
        "display-name-only changes do not invalidate account-scoped materialization"
    );

    let removed =
        margins_workflows::workspace::remove_source(&mut workspace, "renamed-mail").unwrap();
    let account = match removed {
        WorkspaceBinding::Gmail { account, .. } => account,
        _ => unreachable!("mail binding changed kind"),
    };
    let narrowed_selector = GmailCollectionSelector {
        query: "label:important".to_string(),
        backfill_days: 30,
    };
    margins_workflows::workspace::add_source(
        &mut workspace,
        "important-mail",
        WorkspaceBinding::Gmail {
            account,
            gmail: narrowed_selector.clone(),
        },
    )
    .unwrap();
    assert_stale_recall(&workspace, "Alice proposes checkpoint", "important-mail", "refresh_required");
    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    assert_eq!(
        generator.request_count(),
        2,
        "selector changes retain stable Gmail document and prompt hashes"
    );
    assert_stale_recall(&workspace, "Alice proposes checkpoint", "important-mail", "refresh_required");

    store
        .replace_email_thread_snapshot_with_materialization_fingerprint(
            &ctx,
            threads.clone(),
            associations.clone(),
            &narrowed_selector.materialization_fingerprint().unwrap(),
        )
        .unwrap();
    store
        .update_health(&ctx, HealthStatus::Fresh, None)
        .unwrap();
    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    let refreshed = margins::recall::recall(&workspace, "Alice proposes checkpoint", None).unwrap();
    assert_eq!(
        refreshed.freshness.status,
        margins_workflows::integrations::FreshnessStatus::Fresh
    );
    let renamed_index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    let hashes_after_binding_rename = renamed_index
        .prepare(&format!(
            "SELECT source_ref, content_hash FROM docs
             WHERE source_ref LIKE 'sqlite:{gmail_source}/%' ORDER BY source_ref"
        ))
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(hashes_after_binding_rename, hashes_before);
    drop(renamed_index);

    threads[0]
        .body_text
        .push_str("\n\nThe shared plan now records a changed launch decision.");
    let changed = store
        .replace_email_thread_snapshot(&ctx, threads.clone(), associations.clone())
        .unwrap();
    assert_eq!(changed.threads_updated, 1);
    assert_eq!(changed.threads_unchanged, 3);
    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    assert_eq!(
        generator.request_count(),
        4,
        "one changed shared thread invalidates both associated participant prompts"
    );
    let changed_index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    let hashes_after = changed_index
        .prepare(&format!(
            "SELECT source_ref, content_hash FROM docs
             WHERE source_ref LIKE 'sqlite:{gmail_source}/%' ORDER BY source_ref"
        ))
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_ne!(hashes_after, hashes_before);
    drop(changed_index);

    let deleted_thread = threads.pop().unwrap().thread_id;
    associations.retain(|association| association.thread_id != deleted_thread);
    let deleted = store
        .replace_email_thread_snapshot(&ctx, threads, associations)
        .unwrap();
    assert_eq!(
        deleted.threads_deleted, 1,
        "absent members become tombstones"
    );
    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    let tombstoned_index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    let mail_doc_count: i64 = tombstoned_index
        .query_row(
            &format!("SELECT COUNT(*) FROM docs WHERE source_ref LIKE 'sqlite:{gmail_source}/%'"),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        mail_doc_count, 3,
        "tombstoned threads drop out of derived recall on refresh"
    );
    drop(tombstoned_index);
    let ledger = rusqlite::Connection::open(store.db_path()).unwrap();
    let tombstone_row: Option<String> = ledger
        .query_row(
            "SELECT tombstoned_at FROM thread_evidence WHERE thread_id = ?1",
            [&deleted_thread],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        tombstone_row.is_some(),
        "reconcile tombstones absent threads instead of deleting ledger rows"
    );

    use margins_workflows::integrations::{
        apply_retention, preview_retention, RetentionScope, RetentionTarget,
    };
    use margins_workflows::workspace::workspace_revision;
    let target = RetentionTarget {
        connector_id: EMAIL_CONNECTOR_ID.to_string(),
        source_account: "owner@example.com".to_string(),
    };
    let revision = workspace_revision(&workspace).unwrap();
    let all_plan = preview_retention(&workspace, &target, RetentionScope::All).unwrap();
    apply_retention(&workspace, &all_plan, &revision, "recall-all-purge").unwrap();
    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    let purged_index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    let remaining_mail_docs: i64 = purged_index
        .query_row(
            &format!("SELECT COUNT(*) FROM docs WHERE source_ref LIKE 'sqlite:{gmail_source}/%'"),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        remaining_mail_docs, 0,
        "retention apply all prunes derived Enzyme sqlite-source docs"
    );
    let remaining_catalysts: i64 = purged_index
        .query_row(
            "SELECT COUNT(*) FROM catalysts
             WHERE entity IN ('alice@acme.test', 'bob@partner.test')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        remaining_catalysts, 0,
        "purged sqlite-source docs remove stale catalysts"
    );
}

#[test]
fn native_markdown_hashes_survive_binding_rename_and_additional_root() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    recall_engine::initialize_sqlite_runtime().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let reference = temp.path().join("reference");
    let additional = temp.path().join("additional");
    let margins_home = temp.path().join("margins-home");
    for root in [&home, &reference, &additional, &margins_home] {
        std::fs::create_dir_all(root).unwrap();
    }
    std::fs::write(home.join("home.md"), "# Home\n\nNative fixture home.").unwrap();
    for index in 0..4 {
        std::fs::write(
            reference.join(format!("plan-{index}.md")),
            format!(
                "# Plan {index}\n\n[[Shared Project]] checkpoint {index} covers team plan goal risk time data scope cost owner test ship learn track align decide build review adapt share draft check facts note gaps ask peers solve bugs prove value guard launch."
            ),
        )
        .unwrap();
    }
    std::fs::write(
        additional.join("background.md"),
        "# Background\n\nA separate native root without shared catalyst entities.",
    )
    .unwrap();

    let mut workspace =
        margins_workflows::workspace::create_workspace(&margins_home, "native-test", None, &home)
            .unwrap();
    margins_workflows::workspace::add_source(
        &mut workspace,
        "research",
        WorkspaceBinding::NativeMarkdown {
            path: reference.clone(),
            role: SourceRole::Reference,
            note_folder: None,
        },
    )
    .unwrap();
    std::env::set_var("MARGINS_HOME", &margins_home);
    std::env::remove_var("ENZYME_HOME");
    let _generator = fixture_generator::FixtureGenerator::start(&margins_home);
    margins::recall::provision_workspace_for_init(&workspace).unwrap();

    let reference_path = match &workspace.config.bindings["research"] {
        WorkspaceBinding::NativeMarkdown { path, .. } => path,
        _ => unreachable!("research must be a native Markdown source"),
    };
    let namespace = native_markdown_collection_namespace(reference_path).unwrap();
    let index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    let document_hashes_before = native_document_hashes(&index, &namespace);
    assert_eq!(document_hashes_before.len(), 4);
    let catalyst_hashes_before = catalyst_context_hashes(&index);
    assert!(!catalyst_hashes_before.is_empty());
    drop(index);

    let binding = margins_workflows::workspace::remove_source(&mut workspace, "research").unwrap();
    margins_workflows::workspace::add_source(&mut workspace, "renamed-research", binding).unwrap();
    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    let index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    assert_eq!(
        native_document_hashes(&index, &namespace),
        document_hashes_before
    );
    assert_eq!(catalyst_context_hashes(&index), catalyst_hashes_before);
    drop(index);

    margins_workflows::workspace::add_source(
        &mut workspace,
        "background",
        WorkspaceBinding::NativeMarkdown {
            path: additional,
            role: SourceRole::Reference,
            note_folder: None,
        },
    )
    .unwrap();
    margins::recall::provision_workspace_for_init(&workspace).unwrap();

    let index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    assert_eq!(
        native_document_hashes(&index, &namespace),
        document_hashes_before
    );
    assert_eq!(catalyst_context_hashes(&index), catalyst_hashes_before);
}

#[test]
fn scan_style_native_folder_entity_materializes_with_source_qualified_identity() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    recall_engine::initialize_sqlite_runtime().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let margins_home = temp.path().join("margins-home");
    std::fs::create_dir_all(home.join("people")).unwrap();
    std::fs::create_dir_all(&margins_home).unwrap();
    for (index, name) in ["Ada Chen", "Ben Patel", "Cara Jones", "Diego Ruiz"]
        .into_iter()
        .enumerate()
    {
        std::fs::write(
            home.join("people").join(format!("{name}.md")),
            format!(
                "# {name}\n\n{name} relationship context {index} covers trust cadence decision memory planning followup customer signal roadmap ambiguity delivery ownership repair feedback commitments priorities constraints introductions support escalation notes strategy collaboration review learning questions history meeting tone risks outcomes alignment handoff next steps."
            ),
        )
        .unwrap();
    }

    let mut workspace =
        margins_workflows::workspace::create_workspace(&margins_home, "native-folder", None, &home)
            .unwrap();
    workspace.config.policy.entities = vec![WorkspaceEntity::with_options(
        "folder:people",
        WorkspaceEntityOptions {
            profile: Some("relational".to_string()),
            expandable: false,
            children: Vec::new(),
        },
    )];

    std::env::set_var("MARGINS_HOME", &margins_home);
    std::env::remove_var("ENZYME_HOME");
    let generator = fixture_generator::FixtureGenerator::start(&margins_home);
    let status = margins::recall::provision_workspace_for_init(&workspace).unwrap();
    assert_eq!(status.status, "ok", "{status:?}");
    assert_eq!(status.reason, "catalyst", "{status:?}");
    assert_eq!(status.readiness.items_pending, 0, "{status:?}");
    assert!(
        generator.request_count() > 0,
        "fixture generator should be asked to materialize the folder catalyst"
    );

    let namespace = native_markdown_collection_namespace(&workspace.home_dir).unwrap();
    let folder_entity = format!("{namespace}/people");
    let index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    let folder_occurrences: i64 = index
        .query_row(
            "SELECT COUNT(*)
             FROM entities e
             JOIN entity_occurrences eo ON eo.entity_id = e.id
             WHERE e.name = ?1 AND e.type = 'folder'",
            [&folder_entity],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(folder_occurrences, 4);
    let plain_folder_occurrences: i64 = index
        .query_row(
            "SELECT COUNT(*)
             FROM entities e
             JOIN entity_occurrences eo ON eo.entity_id = e.id
             WHERE e.name = 'people' AND e.type = 'folder'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        plain_folder_occurrences, 0,
        "native Markdown folder identity must remain source-qualified"
    );
    let catalysts: i64 = index
        .query_row(
            "SELECT COUNT(*) FROM catalysts
             WHERE entity = ?1 AND json_extract(metadata, '$.entity_type') = 'folder'",
            [&folder_entity],
            |row| row.get(0),
        )
        .unwrap();
    assert!(catalysts > 0, "expected catalyst for {folder_entity}");
    let plain_skips: i64 = index
        .query_row(
            "SELECT COUNT(*) FROM catalyst_generation_skips WHERE entity = 'people'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(plain_skips, 0);
}

#[test]
fn thin_source_qualified_folder_uses_expanded_link_catalysts() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    recall_engine::initialize_sqlite_runtime().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let margins_home = temp.path().join("margins-home");
    std::fs::create_dir_all(home.join("people")).unwrap();
    std::fs::create_dir_all(home.join("inbox")).unwrap();
    std::fs::create_dir_all(home.join("Readwise")).unwrap();
    std::fs::create_dir_all(&margins_home).unwrap();
    std::fs::write(home.join("people/Alice Smith.md"), "[[Alice Smith]]\n").unwrap();
    for index in 0..4 {
        std::fs::write(
            home.join("inbox").join(format!("alice-{index}.md")),
            format!(
                "# Conversation {index}\n\n[[Alice Smith]] relationship context trust cadence decision memory planning followup customer signal roadmap ambiguity delivery ownership repair feedback commitments priorities constraints introductions support escalation notes strategy collaboration review learning questions history meeting tone risks outcomes alignment handoff next steps. This substantive reflection records what changed, why it mattered, how the relationship developed, and what should remain available for future preparation."
            ),
        )
        .unwrap();
    }
    let exact_phrase = "sand gods coordinate through patient distributed attention";
    std::fs::write(
        home.join("Readwise/rare-reference.md"),
        format!(
            "# Rare reference\n\nThis imported reading records a durable idea about coordination, memory, collective judgment, institutional learning, shared context, careful observation, feedback, responsibility, agency, and long-running collaboration. The phrase is: {exact_phrase}. It remains reference material rather than a relationship note, but it must stay reachable inside the declared notes boundary after catalyst attention is curated elsewhere.\n"
        ),
    )
    .unwrap();

    let mut workspace =
        margins_workflows::workspace::create_workspace(&margins_home, "people-links", None, &home)
            .unwrap();
    workspace.config.policy.entities = vec![WorkspaceEntity::with_options(
        "folder:people",
        WorkspaceEntityOptions {
            profile: Some("relational".to_string()),
            expandable: false,
            children: Vec::new(),
        },
    )];

    std::env::set_var("MARGINS_HOME", &margins_home);
    std::env::remove_var("ENZYME_HOME");
    let generator = fixture_generator::FixtureGenerator::start(&margins_home);
    let status = margins::recall::provision_workspace_for_init(&workspace).unwrap();
    assert_eq!(status.status, "ok", "{status:?}");
    assert_eq!(status.reason, "catalyst", "{status:?}");
    assert_eq!(status.readiness.items_pending, 0, "{status:?}");
    assert!(generator.request_count() > 0);

    let namespace = native_markdown_collection_namespace(&workspace.home_dir).unwrap();
    let folder_entity = format!("{namespace}/people");
    let index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    let link_catalysts: i64 = index
        .query_row(
            "SELECT COUNT(*) FROM catalysts
             WHERE entity = 'alice smith'
               AND json_extract(metadata, '$.entity_type') = 'link'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        link_catalysts > 0,
        "expanded person link should be catalyzed"
    );
    let folder_thin_evidence: i64 = index
        .query_row(
            "SELECT COUNT(*) FROM catalyst_generation_skips
             WHERE entity = ?1 AND reason_code = 'thin_evidence'",
            [&folder_entity],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        folder_thin_evidence > 0,
        "the thin folder should remain auditable without blocking its child catalysts"
    );

    let indexed_exact_paths = index
        .prepare(
            "SELECT d.source_ref FROM chunks c JOIN docs d ON d.id = c.doc_id
             WHERE instr(lower(c.content), lower(?1)) > 0",
        )
        .unwrap()
        .query_map([exact_phrase], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(indexed_exact_paths.len(), 1, "{indexed_exact_paths:?}");
    index
        .execute(
            "DELETE FROM catalyst_similarities WHERE file_path = ?1",
            [&indexed_exact_paths[0]],
        )
        .unwrap();
    let recalled = margins::recall::recall(&workspace, exact_phrase, Some("home")).unwrap();
    assert_eq!(recalled.status, "ok");
    assert_eq!(recalled.search_strategy, "catalyze");
    let recalled_refs = recalled
        .results
        .iter()
        .map(|result| (&result.document_ref, &result.via_catalyst_id))
        .collect::<Vec<_>>();
    assert!(
        recalled
            .results
            .iter()
            .any(|result| {
                result.document_ref.ends_with("/Readwise/rare-reference.md")
                    && result.via_catalyst_id.is_none()
            }),
        "explicit catalyst curation must not hide exact phrases elsewhere in home: {recalled_refs:?}"
    );

    let post_init_phrase = "unindexed future phrase remains outside the snapshot";
    std::fs::write(
        home.join("Readwise/post-init.md"),
        format!("# Later\n\n{post_init_phrase}\n"),
    )
    .unwrap();
    let not_recalled = margins::recall::recall(&workspace, post_init_phrase, None).unwrap();
    assert!(
        not_recalled
            .results
            .iter()
            .all(|result| !result.document_ref.ends_with("/Readwise/post-init.md")),
        "literal lookup must remain bounded to the indexed snapshot"
    );
}

fn native_document_hashes(index: &rusqlite::Connection, namespace: &str) -> Vec<(String, String)> {
    index
        .prepare(
            "SELECT source_ref, content_hash FROM docs
             WHERE source_ref LIKE ?1 ORDER BY source_ref",
        )
        .unwrap()
        .query_map([format!("{namespace}/%")], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn catalyst_context_hashes(index: &rusqlite::Connection) -> Vec<(String, String, String)> {
    index
        .prepare(
            "SELECT entity, entity_type, context_hash
             FROM catalyst_entity_hashes ORDER BY entity, entity_type",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}
