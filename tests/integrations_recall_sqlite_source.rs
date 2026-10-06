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
    CalendarCollectionSelector, GmailCollectionSelector, SourceKind, SourceRole,
    WorkspaceBinding, WorkspaceEntity, WorkspaceEntityOptions,
};
use margins_workflows::source_kinds::sqlite_document_ref_prefix;
use std::collections::BTreeSet;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Readings live in the Workspace program; edit them through the program.
fn set_entities(
    workspace: &mut margins_workflows::workspace::ResolvedWorkspace,
    entities: Vec<WorkspaceEntity>,
) {
    let mut policy = workspace.config.policy.clone();
    policy.entities = entities;
    margins_workflows::workspace::update_policy(workspace, policy).unwrap();
}

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
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
    set_entities(
        &mut workspace,
        vec![
            WorkspaceEntity::simple("[[grace@example.com]]"),
            WorkspaceEntity::simple("[[alex@example.com]]"),
        ],
    );

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
    let _generator = fixture_generator::FixtureGenerator::start(&margins_home);
    margins::recall::provision_workspace_for_init(&workspace).unwrap();

    let prefix = sqlite_document_ref_prefix("calendar");
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
    // Document identity follows the source name: a rename moves the same
    // records (same content hashes) to the new name and leaves none behind.
    let renamed_prefix = sqlite_document_ref_prefix("renamed-calendar");
    let renamed_index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    let hashes_by_prefix = |prefix: &str| {
        renamed_index
            .prepare("SELECT source_ref, content_hash FROM docs WHERE source_ref LIKE ?1 ORDER BY source_ref")
            .unwrap()
            .query_map([format!("{prefix}%")], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert!(hashes_by_prefix(&prefix).is_empty());
    let hashes_after = hashes_by_prefix(&renamed_prefix);
    let content = |hashes: &[(String, String)]| {
        hashes
            .iter()
            .map(|(_, hash)| hash.clone())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(content(&hashes_after).len(), content(&hashes_before).len());
    // Refs are the engine's stable identity of (source name, ledger id),
    // which Margins computes to map hits back to ledger records.
    let ledger = rusqlite::Connection::open(workspace.ledger_path()).unwrap();
    let ids = ledger
        .prepare("SELECT source_id FROM calendar_event_evidence WHERE tombstoned_at IS NULL")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let expected = |name: &str| {
        ids.iter()
            .map(|id| margins_workflows::source_kinds::ledger_document_ref(name, id))
            .collect::<BTreeSet<_>>()
    };
    let refs = |hashes: &[(String, String)]| {
        hashes
            .iter()
            .map(|(source_ref, _)| source_ref.clone())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(refs(&hashes_before), expected("calendar"));
    assert_eq!(refs(&hashes_after), expected("renamed-calendar"));
    drop(renamed_index);

    margins_workflows::workspace::remove_source(&mut workspace, "renamed-calendar").unwrap();
    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    let removed_index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    let remaining: i64 = removed_index
        .query_row(
            "SELECT COUNT(*) FROM docs WHERE source_ref LIKE 'sqlite:%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0, "binding removal excludes retained evidence");
}

/// Mail is one complete document per thread. Every associated participant sees
/// that same document through Enzyme's ordinary shared-document context path.
#[test]
fn materialized_mail_threads_use_generic_shared_document_context() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
    let gmail_source = "mail";
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
    assert!(
        !catalysts.is_empty(),
        "generic catalyst generation skipped shared evidence: requests={}",
        generator.request_count()
    );
    // Both correspondents are generated; the engine's automatic selection
    // may add entities of its own (one request each).
    let requests_after_init = generator.request_count();
    assert!(requests_after_init >= 2, "{requests_after_init}");
    for entity in ["alice@acme.test", "bob@partner.test"] {
        assert!(catalysts.iter().any(|(name, _)| name == entity), "{entity}: {catalysts:?}");
    }
    for (entity, text) in catalysts {
        let (hypothesis, source_refs) = catalyst_parts(&text);
        assert!(hypothesis.to_ascii_lowercase().contains(&entity), "{text}");
        assert!(!source_refs.is_empty());
        assert!(source_refs
            .iter()
            .all(|source_ref| alice_paths.contains(source_ref)));
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

    // enzyme 0.11.1 regenerates SQLite `who` link entities once on the second
    // init over unchanged rows (reported upstream); after that, an unchanged
    // snapshot reuses Enzyme document and prompt hashes.
    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    let requests_after_init = generator.request_count();
    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    assert_eq!(
        generator.request_count(),
        requests_after_init,
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
    // Document identity follows the source name (`sqlite:<name>/…`), so a
    // rename re-identifies the same threads; generation may rerun once.
    let mut requests_after_rename = generator.request_count();
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
    assert_stale_recall(&workspace, "Alice proposes checkpoint", "important-mail", "refresh_required");
    // Settle the engine's one-time link regeneration (see above).
    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    requests_after_rename = requests_after_rename.max(generator.request_count());

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
    assert_eq!(
        generator.request_count(),
        requests_after_rename,
        "an unchanged refreshed snapshot reuses Enzyme document and prompt hashes"
    );
    let gmail_source = "important-mail";
    let renamed_index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    let old_refs: i64 = renamed_index
        .query_row(
            "SELECT COUNT(*) FROM docs WHERE source_ref LIKE 'sqlite:mail/%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_refs, 0, "the previous source name keeps no documents");
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
    // The same threads under the new source name: refs are the engine's
    // identity of (source name, thread id).
    assert_eq!(hashes_after_binding_rename.len(), hashes_before.len());
    assert_eq!(
        hashes_after_binding_rename
            .iter()
            .map(|(source_ref, _)| source_ref.clone())
            .collect::<BTreeSet<_>>(),
        threads
            .iter()
            .map(|thread| margins_workflows::source_kinds::ledger_document_ref(
                gmail_source,
                &thread.thread_id
            ))
            .collect::<BTreeSet<_>>()
    );
    let hashes_before = hashes_after_binding_rename;
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
    // Both participants' prompts are invalidated (and the source's collection
    // entity, when the engine selected it).
    assert!(
        generator.request_count() >= requests_after_rename + 2,
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
fn native_markdown_refs_are_qualified_by_source_name() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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

    // Several Markdown sources: refs are `<source name>/<relative path>`.
    let index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    let document_hashes_before = native_document_hashes(&index, "research");
    assert_eq!(document_hashes_before.len(), 4);
    let catalyst_hashes_before = catalyst_context_hashes(&index);
    assert!(!catalyst_hashes_before.is_empty());
    drop(index);

    let binding = margins_workflows::workspace::remove_source(&mut workspace, "research").unwrap();
    margins_workflows::workspace::add_source(&mut workspace, "renamed-research", binding).unwrap();
    margins::recall::provision_workspace_for_init(&workspace).unwrap();
    // A rename is a new source name: its documents move to the new identity
    // with unchanged content, and nothing remains under the old name.
    let index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    assert!(native_document_hashes(&index, "research").is_empty());
    let renamed = native_document_hashes(&index, "renamed-research");
    assert_eq!(
        renamed
            .iter()
            .map(|(source_ref, hash)| (
                source_ref.replacen("renamed-research/", "research/", 1),
                hash.clone()
            ))
            .collect::<Vec<_>>(),
        document_hashes_before
    );
    let catalyst_hashes_before = catalyst_context_hashes(&index);
    assert!(!catalyst_hashes_before.is_empty());
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

    // Adding a third root leaves existing source-name identities alone. The
    // engine's catalyst prompt includes the workspace's context, so catalysts
    // may be regenerated, but every entity keeps its catalysts.
    let index = rusqlite::Connection::open(workspace.recall_path()).unwrap();
    assert_eq!(native_document_hashes(&index, "renamed-research"), renamed);
    let entities = |hashes: Vec<(String, String, String)>| {
        hashes
            .into_iter()
            .map(|(entity, kind, _)| (entity, kind))
            .collect::<BTreeSet<_>>()
    };
    assert!(entities(catalyst_context_hashes(&index)).is_superset(&entities(catalyst_hashes_before)));
}

#[test]
fn scan_style_native_folder_entity_materializes_with_language_identity() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
    set_entities(
        &mut workspace,
        vec![WorkspaceEntity::with_options(
            "folder:people",
            WorkspaceEntityOptions {
                profile: Some("relational".to_string()),
                expandable: false,
                children: Vec::new(),
            },
        )],
    );

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

    // One Markdown source: folder identity is the root-relative path.
    let folder_entity = "people".to_string();
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
    let hashed_folder_occurrences: i64 = index
        .query_row(
            "SELECT COUNT(*) FROM entities WHERE name LIKE 'markdown\\_%' ESCAPE '\\'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        hashed_folder_occurrences, 0,
        "hashed native Markdown namespaces are retired"
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
}

#[test]
fn thin_source_qualified_folder_uses_expanded_link_catalysts() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
    set_entities(
        &mut workspace,
        vec![WorkspaceEntity::with_options(
            "folder:people",
            WorkspaceEntityOptions {
                profile: Some("relational".to_string()),
                expandable: false,
                children: Vec::new(),
            },
        )],
    );

    std::env::set_var("MARGINS_HOME", &margins_home);
    std::env::remove_var("ENZYME_HOME");
    let generator = fixture_generator::FixtureGenerator::start(&margins_home);
    let status = margins::recall::provision_workspace_for_init(&workspace).unwrap();
    assert_eq!(status.status, "ok", "{status:?}");
    assert_eq!(status.reason, "catalyst", "{status:?}");
    assert_eq!(status.readiness.items_pending, 0, "{status:?}");
    assert!(generator.request_count() > 0);

    let folder_entity = "people";
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
    // The engine (v0.9.2+) no longer audits skipped generations; a thin
    // folder simply has no catalyst while its expanded child links do.
    let folder_catalysts: i64 = index
        .query_row(
            "SELECT COUNT(*) FROM catalysts
             WHERE entity = ?1 AND json_extract(metadata, '$.entity_type') = 'folder'",
            [folder_entity],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(folder_catalysts, 0, "the thin folder itself is not catalyzed");

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
                result.document_ref == "Readwise/rare-reference.md"
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
            .all(|result| result.document_ref != "Readwise/post-init.md"),
        "literal lookup must remain bounded to the indexed snapshot"
    );
}

/// The hypothesis and context refs of a canonical catalyst chunk:
/// `<hypothesis>\n\n```enzyme-context\n[refs]\n```` (see
/// enzyme-rust `docs/catalyst-text-format.md`).
fn catalyst_parts(text: &str) -> (String, Vec<String>) {
    let (hypothesis, rest) = text
        .split_once("```enzyme-context")
        .unwrap_or_else(|| panic!("catalyst text has no context block: {text}"));
    let refs = rest.trim().trim_end_matches("```").trim();
    let refs = refs.lines().next().unwrap_or("[]");
    (
        hypothesis.trim().to_string(),
        serde_json::from_str(refs).unwrap_or_else(|error| panic!("{error}: {text}")),
    )
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
