//! Fixture-backed acceptance coverage for the Gmail connector.

use anyhow::{anyhow, Result};
use margins_workflows::integrations::{
    automatic_correspondent_entity_selection, parse_gmail_thread_from_value, Connector,
    ConnectorCtx, GmailSearchPage, GmailThreadTransport, GoogleEmailConnector, HealthStatus,
    IntegrationsStore, ParticipantThread, ThreadEvidence, EMAIL_CONNECTOR_ID,
};
use margins_workflows::workspace::GmailCollectionSelector;
use rusqlite::Connection;
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/email")
        .join(name)
}

#[derive(Debug, Clone, Default)]
struct FixtureGmailTransport {
    fail: bool,
}

impl GmailThreadTransport for FixtureGmailTransport {
    fn search_page(
        &self,
        _ctx: &ConnectorCtx,
        query: &str,
        _max_results: u64,
        page_token: Option<&str>,
    ) -> Result<GmailSearchPage> {
        if self.fail {
            return Err(anyhow!("fixture Gmail refresh failed"));
        }
        let filename = if query.starts_with("from:me ")
            && query.contains("newer_than:365d -in:spam -in:trash")
        {
            "gmail-search-sent.json"
        } else if query.contains("newer_than:365d -in:spam -in:trash")
            && page_token == Some("survey-page-2")
        {
            "gmail-search-page-2.json"
        } else if query.contains("newer_than:365d -in:spam -in:trash") {
            "gmail-search-page-1.json"
        } else {
            "gmail-search.json"
        };
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(fixture_path(filename))?)?;
        let threads = value
            .get("threads")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|summary| summary.get("id")?.as_str().map(str::to_string))
            .collect();
        let next_page_token = value
            .get("nextPageToken")
            .and_then(serde_json::Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_string);
        Ok(GmailSearchPage {
            thread_ids: threads,
            next_page_token,
        })
    }

    fn fetch_thread(
        &self,
        _ctx: &ConnectorCtx,
        thread_id: &str,
    ) -> Result<margins_workflows::integrations::FetchedThread> {
        if self.fail {
            return Err(anyhow!("fixture Gmail refresh failed"));
        }
        let filename = match thread_id {
            "t-client" => "gmail-thread-client.json",
            "t-unrelated" => "gmail-thread-unrelated.json",
            "t-ambiguous" => "gmail-thread-ambiguous.json",
            "t-shared" => "gmail-thread-shared.json",
            "t-new-noisy" => "gmail-thread-new-noisy.json",
            other => return Err(anyhow!("unexpected Gmail thread fixture {other}")),
        };
        let value = serde_json::from_str(&std::fs::read_to_string(fixture_path(filename))?)?;
        parse_gmail_thread_from_value(value, thread_id)
    }
}

fn fixture_connector() -> GoogleEmailConnector<FixtureGmailTransport> {
    GoogleEmailConnector::new(
        GmailCollectionSelector::default_declaration(),
        FixtureGmailTransport::default(),
    )
}

#[test]
fn reconcile_uses_workspace_boundary_raw_first_and_idempotently() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let ctx = ConnectorCtx {
        vault_root: temp.path().to_path_buf(),
        connector_id: EMAIL_CONNECTOR_ID.to_string(),
        account: "owner@example.com".to_string(),
        command_path: None,
    };
    let connector = GoogleEmailConnector::new(
        GmailCollectionSelector {
            query: String::new(),
            backfill_days: 30,
        },
        FixtureGmailTransport::default(),
    );
    let first = connector.reconcile(&ctx, None)?;
    let store = IntegrationsStore::open(temp.path())?;
    let report = store
        .latest_curation_observation_for_account(EMAIL_CONNECTOR_ID, &ctx.account)?
        .expect("curation observation after reconcile");
    assert_eq!(report.item_counts.counts.get("email"), Some(&4));
    assert!(report
        .decisions_required
        .iter()
        .any(|decision| decision.contains("kevin")));
    assert!(report
        .inferred_people
        .iter()
        .filter(|person| person.label.starts_with("Kevin <"))
        .all(|person| person.ambiguous));
    assert!(!report
        .proposed_include
        .iter()
        .any(|entry| entry.contains("kevin.one") || entry.contains("kevin.two")));
    assert_eq!(
        report.proposal_flags.get("person:newsletter@noise.test"),
        Some(&vec!["likely-automated".to_string()])
    );
    assert_eq!(
        report.proposal_flags.get("domain:noise.test"),
        Some(&vec!["likely-automated".to_string()])
    );
    assert_eq!(
        report.proposal_evidence["person:newsletter@noise.test"],
        serde_json::json!({
            "threads": 1,
            "thread_count": 1,
            "user_replied": 0,
            "user_sent": 0,
            "last_interaction": "2026-08-20T11:00:00Z",
            "automated_signals": ["automated-address-pattern", "automated-subject", "list-unsubscribe"],
            "samples": ["recent", "sent"]
        })
    );
    let client_rank = report
        .proposed_include
        .iter()
        .position(|value| value == "person:alice@acme.test")
        .unwrap();
    let automated_rank = report
        .proposed_include
        .iter()
        .position(|value| value == "person:newsletter@noise.test")
        .unwrap();
    assert!(client_rank < automated_rank);
    assert!(
        report.proposal_evidence["person:alice@acme.test"]["user_sent"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(
        report.proposal_evidence["person:alice@acme.test"]["user_replied"]
            .as_u64()
            .unwrap()
            > 0
    );

    assert_eq!(first.records_written, 4);

    let connection = Connection::open(store.db_path())?;
    let thread_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM thread_evidence WHERE connector_id = 'email'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        thread_count, 4,
        "every thread matching the Workspace boundary is materialized"
    );
    let (body, href): (String, Option<String>) = connection.query_row(
        "SELECT body_text, href FROM thread_evidence WHERE thread_id = 't-client'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert!(body.contains("pilot results"));
    assert!(!body.contains("From:"));
    assert!(!body.contains("alice@acme.test"));
    assert_eq!(
        href.as_deref(),
        Some("https://mail.google.com/mail/?authuser=owner%40example.com#all/t-client")
    );
    let raw_thread_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM raw_items WHERE connector_id = 'email'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        raw_thread_count, 4,
        "connector cache retains fetched threads"
    );
    let (body, href): (String, Option<String>) = connection.query_row(
        "SELECT body_text, href FROM thread_evidence WHERE thread_id = 't-ambiguous'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert!(body.contains("South Kevin replying separately."));
    assert!(!body.contains("kevin.one@north.test"));
    assert!(!body.contains("Subject:"));
    assert!(href.unwrap_or_default().contains("#all/t-ambiguous"));

    let second = connector.reconcile(&ctx, None)?;
    assert_eq!(second.records_written, 0);
    assert_eq!(second.records_updated, 0);
    assert_eq!(second.records_unchanged, 4);
    Ok(())
}

#[test]
fn default_reconcile_combines_recent_list_traffic_with_sent_correspondence() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let ctx = ConnectorCtx {
        vault_root: temp.path().to_path_buf(),
        connector_id: EMAIL_CONNECTOR_ID.to_string(),
        account: "owner@example.com".to_string(),
        command_path: None,
    };

    fixture_connector().reconcile(&ctx, None)?;
    let store = IntegrationsStore::open(temp.path())?;
    let report = store
        .latest_curation_observation_for_account(EMAIL_CONNECTOR_ID, &ctx.account)?
        .expect("curation observation");
    assert_eq!(report.item_counts.counts.get("email"), Some(&4));
    assert!(
        report.proposal_evidence["person:alice@acme.test"]["user_replied"]
            .as_u64()
            .unwrap()
            > 0
    );
    let alice_rank = report
        .proposed_include
        .iter()
        .position(|proposal| proposal == "person:alice@acme.test")
        .unwrap();
    let newsletter_rank = report
        .proposed_include
        .iter()
        .position(|proposal| proposal == "person:newsletter@noise.test")
        .unwrap();
    assert!(alice_rank < newsletter_rank);
    assert_eq!(
        report.proposal_evidence["person:alice@acme.test"]["samples"],
        serde_json::json!(["sent"])
    );
    assert_eq!(report.observation_call_count, Some(7));
    let raw_count_before = store.raw_items(&ctx)?.len();
    fixture_connector().reconcile(&ctx, None)?;
    let repeated = store
        .latest_curation_observation_for_account(EMAIL_CONNECTOR_ID, &ctx.account)?
        .expect("curation observation after repeat reconcile");
    assert_eq!(
        repeated.observation_call_count,
        Some(7),
        "replacement is always based on a complete bounded snapshot"
    );
    assert_eq!(store.raw_items(&ctx)?.len(), raw_count_before);
    let healthy = store.health_report(&ctx)?;
    assert_eq!(healthy.status, HealthStatus::Fresh);
    assert!(healthy.last_successful_sync.is_some());
    GoogleEmailConnector::new(
        GmailCollectionSelector::default_declaration(),
        FixtureGmailTransport { fail: true },
    )
    .reconcile(&ctx, None)
    .expect_err("failed Gmail refresh is recorded without deleting evidence");
    let failed = store.health_report(&ctx)?;
    assert_eq!(failed.status, HealthStatus::Error);
    assert_eq!(failed.last_successful_sync, healthy.last_successful_sync);
    let connection = Connection::open(store.db_path())?;
    assert_eq!(
        connection.query_row(
            "SELECT COUNT(*) FROM thread_evidence WHERE connector_id = 'email'",
            [],
            |row| row.get::<_, i64>(0),
        )?,
        4
    );
    let requested = report
        .observation_window
        .expect("requested observation window");
    assert_eq!(
        requested.occurred_to - requested.occurred_from,
        chrono::TimeDelta::days(365)
    );
    let observed = report.observed_range.expect("observed thread range");
    assert_eq!(
        observed.occurred_from.to_rfc3339(),
        "2026-02-20T10:05:00+00:00"
    );
    assert_eq!(
        observed.occurred_to.to_rfc3339(),
        "2026-08-21T10:15:00+00:00"
    );
    assert_eq!(
        report.sample_observed_ranges["sent"]
            .occurred_from
            .to_rfc3339(),
        "2026-02-20T10:05:00+00:00"
    );
    assert_eq!(
        report.sample_observed_ranges["recent"]
            .occurred_from
            .to_rfc3339(),
        "2026-08-20T11:00:00+00:00"
    );
    Ok(())
}

#[test]
fn participant_associations_are_role_blind_and_owner_excluded() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let ctx = ConnectorCtx {
        vault_root: temp.path().to_path_buf(),
        connector_id: EMAIL_CONNECTOR_ID.to_string(),
        account: "owner@example.com".to_string(),
        command_path: None,
    };
    fixture_connector().reconcile(&ctx, None)?;
    let store = IntegrationsStore::open(temp.path())?;
    let associations = store.participant_threads(&ctx)?;
    let thread_participants = associations
        .iter()
        .filter(|edge| edge.thread_id == "t-client")
        .map(|edge| edge.participant.as_str())
        .collect::<Vec<_>>();
    assert!(thread_participants.contains(&"alice@acme.test"));
    assert!(!thread_participants.contains(&"owner@example.com"));
    Ok(())
}

#[test]
fn ranked_participants_order_by_aggregate_score_then_recency() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let ctx = ConnectorCtx {
        vault_root: temp.path().to_path_buf(),
        connector_id: EMAIL_CONNECTOR_ID.to_string(),
        account: "owner@example.com".to_string(),
        command_path: None,
    };
    let store = IntegrationsStore::open(temp.path())?;
    let older =
        chrono::DateTime::parse_from_rfc3339("2026-08-20T10:00:00Z")?.with_timezone(&chrono::Utc);
    let newer =
        chrono::DateTime::parse_from_rfc3339("2026-08-21T10:00:00Z")?.with_timezone(&chrono::Utc);
    store.replace_email_thread_snapshot(
        &ctx,
        vec![
            ThreadEvidence {
                thread_id: "older".into(),
                occurred_from: older,
                occurred_to: older,
                body_text: "older".into(),
                href: None,
            },
            ThreadEvidence {
                thread_id: "newer".into(),
                occurred_from: newer,
                occurred_to: newer,
                body_text: "newer".into(),
                href: None,
            },
        ],
        vec![
            ParticipantThread {
                participant: "high@example.test".into(),
                thread_id: "older".into(),
                last_interaction: older,
                sampling_score: Some(1_001),
            },
            ParticipantThread {
                participant: "alpha@example.test".into(),
                thread_id: "newer".into(),
                last_interaction: newer,
                sampling_score: Some(1),
            },
            ParticipantThread {
                participant: "beta@example.test".into(),
                thread_id: "newer".into(),
                last_interaction: newer,
                sampling_score: Some(1),
            },
            ParticipantThread {
                participant: "older@example.test".into(),
                thread_id: "older".into(),
                last_interaction: older,
                sampling_score: Some(1),
            },
        ],
    )?;
    let ranked = store.ranked_participants(&ctx)?;
    assert_eq!(
        ranked
            .iter()
            .map(|row| row.participant.as_str())
            .collect::<Vec<_>>(),
        vec![
            "high@example.test",
            "alpha@example.test",
            "beta@example.test",
            "older@example.test",
        ]
    );
    let selection = automatic_correspondent_entity_selection(temp.path(), &ctx.account)?;
    assert!(selection
        .entities
        .iter()
        .all(|entity| entity.kind == "person"));
    assert_eq!(selection.entities.len(), 4);
    Ok(())
}

#[test]
fn ranked_participants_keep_reply_priority_for_large_collections() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let ctx = ConnectorCtx {
        vault_root: temp.path().to_path_buf(),
        connector_id: EMAIL_CONNECTOR_ID.to_string(),
        account: "owner@example.com".to_string(),
        command_path: None,
    };
    let store = IntegrationsStore::open(temp.path())?;
    let occurred =
        chrono::DateTime::parse_from_rfc3339("2026-08-21T10:00:00Z")?.with_timezone(&chrono::Utc);
    let mut threads = Vec::new();
    let mut associations = Vec::new();
    for index in 0..1_001 {
        let thread_id = format!("sent-{index:04}");
        threads.push(ThreadEvidence {
            thread_id: thread_id.clone(),
            occurred_from: occurred,
            occurred_to: occurred,
            body_text: "owner sent without a reply".into(),
            href: None,
        });
        associations.push(ParticipantThread {
            participant: "many-sent@example.test".into(),
            thread_id,
            last_interaction: occurred,
            sampling_score: Some(1_001),
        });
    }
    threads.push(ThreadEvidence {
        thread_id: "one-reply".into(),
        occurred_from: occurred,
        occurred_to: occurred,
        body_text: "one real reply".into(),
        href: None,
    });
    associations.push(ParticipantThread {
        participant: "one-reply@example.test".into(),
        thread_id: "one-reply".into(),
        last_interaction: occurred,
        sampling_score: Some(1_000_001),
    });
    store.replace_email_thread_snapshot(&ctx, threads, associations)?;

    let ranked = store.ranked_participants(&ctx)?;
    assert_eq!(ranked[0].participant, "one-reply@example.test");
    assert_eq!(ranked[1].participant, "many-sent@example.test");
    assert!(ranked[0].aggregate_sampling_score > ranked[1].aggregate_sampling_score);
    Ok(())
}

#[test]
fn absent_snapshot_threads_are_tombstoned_and_preserve_role_blind_associations() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let store = IntegrationsStore::open(temp.path())?;
    let ctx = ConnectorCtx {
        vault_root: temp.path().to_path_buf(),
        connector_id: EMAIL_CONNECTOR_ID.to_string(),
        account: "owner@example.com".to_string(),
        command_path: None,
    };
    let occurred = chrono::DateTime::parse_from_rfc3339("2026-08-20T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let first = store.replace_email_thread_snapshot(
        &ctx,
        vec![
            ThreadEvidence {
                thread_id: "keep".into(),
                occurred_from: occurred,
                occurred_to: occurred,
                body_text: "keep".into(),
                href: None,
            },
            ThreadEvidence {
                thread_id: "drop".into(),
                occurred_from: occurred,
                occurred_to: occurred,
                body_text: "role-blind thread body without message ids".into(),
                href: None,
            },
        ],
        vec![
            ParticipantThread {
                participant: "peer@example.com".into(),
                thread_id: "keep".into(),
                last_interaction: occurred,
                sampling_score: Some(1),
            },
            ParticipantThread {
                participant: "gone@example.com".into(),
                thread_id: "drop".into(),
                last_interaction: occurred,
                sampling_score: Some(1),
            },
        ],
    )?;
    assert_eq!(first.threads_written, 2);
    store.ingest_raw_connector_items(
        &ctx,
        vec![margins_workflows::integrations::RawItemDraft {
            source_id: "drop".into(),
            payload: serde_json::json!({"stale": "connector cache only"}),
        }],
    )?;
    let second = store.replace_email_thread_snapshot(
        &ctx,
        vec![ThreadEvidence {
            thread_id: "keep".into(),
            occurred_from: occurred,
            occurred_to: occurred,
            body_text: "keep changed".into(),
            href: None,
        }],
        vec![ParticipantThread {
            participant: "peer@example.com".into(),
            thread_id: "keep".into(),
            last_interaction: occurred,
            sampling_score: Some(1),
        }],
    )?;
    assert_eq!(second.threads_updated, 1);
    assert_eq!(
        second.threads_deleted, 1,
        "absent members count as tombstones"
    );
    let third = store.replace_email_thread_snapshot(
        &ctx,
        vec![ThreadEvidence {
            thread_id: "keep".into(),
            occurred_from: occurred,
            occurred_to: occurred,
            body_text: "keep changed".into(),
            href: None,
        }],
        vec![ParticipantThread {
            participant: "peer@example.com".into(),
            thread_id: "keep".into(),
            last_interaction: occurred,
            sampling_score: Some(1),
        }],
    )?;
    assert_eq!(third.threads_unchanged, 1);
    let connection = Connection::open(store.db_path())?;
    let (body, tombstoned_at): (String, Option<String>) = connection.query_row(
        "SELECT body_text, tombstoned_at FROM thread_evidence WHERE thread_id = 'drop'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert!(
        tombstoned_at.is_some(),
        "absent snapshot members are tombstoned, not hard-deleted"
    );
    assert_eq!(body, "role-blind thread body without message ids");
    assert!(!body.contains("Message "));
    assert_eq!(
        connection.query_row(
            "SELECT COUNT(*) FROM raw_items WHERE source_id = 'drop'",
            [],
            |row| row.get::<_, i64>(0),
        )?,
        1,
        "stale raw cache cannot resurrect tombstoned evidence"
    );
    assert_eq!(
        connection.query_row(
            "SELECT COUNT(*) FROM participant_threads WHERE thread_id = 'drop'",
            [],
            |row| row.get::<_, i64>(0),
        )?,
        1,
        "role-blind participant associations survive Gmail tombstones"
    );
    Ok(())
}

#[test]
fn tombstoned_threads_resurrect_when_the_next_snapshot_reincludes_them() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let store = IntegrationsStore::open(temp.path())?;
    let ctx = ConnectorCtx {
        vault_root: temp.path().to_path_buf(),
        connector_id: EMAIL_CONNECTOR_ID.to_string(),
        account: "owner@example.com".to_string(),
        command_path: None,
    };
    let occurred = chrono::DateTime::parse_from_rfc3339("2026-08-20T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    store.replace_email_thread_snapshot(
        &ctx,
        vec![
            ThreadEvidence {
                thread_id: "keep".into(),
                occurred_from: occurred,
                occurred_to: occurred,
                body_text: "keep".into(),
                href: None,
            },
            ThreadEvidence {
                thread_id: "returning".into(),
                occurred_from: occurred,
                occurred_to: occurred,
                body_text: "first body".into(),
                href: None,
            },
        ],
        vec![ParticipantThread {
            participant: "peer@example.com".into(),
            thread_id: "returning".into(),
            last_interaction: occurred,
            sampling_score: Some(1),
        }],
    )?;
    store.replace_email_thread_snapshot(
        &ctx,
        vec![ThreadEvidence {
            thread_id: "keep".into(),
            occurred_from: occurred,
            occurred_to: occurred,
            body_text: "keep".into(),
            href: None,
        }],
        Vec::new(),
    )?;
    let resurrected = store.replace_email_thread_snapshot(
        &ctx,
        vec![
            ThreadEvidence {
                thread_id: "keep".into(),
                occurred_from: occurred,
                occurred_to: occurred,
                body_text: "keep".into(),
                href: None,
            },
            ThreadEvidence {
                thread_id: "returning".into(),
                occurred_from: occurred,
                occurred_to: occurred,
                body_text: "restored body".into(),
                href: None,
            },
        ],
        vec![ParticipantThread {
            participant: "peer@example.com".into(),
            thread_id: "returning".into(),
            last_interaction: occurred,
            sampling_score: Some(1),
        }],
    )?;
    assert_eq!(resurrected.threads_updated, 1);
    let connection = Connection::open(store.db_path())?;
    let (body, tombstoned_at): (String, Option<String>) = connection.query_row(
        "SELECT body_text, tombstoned_at FROM thread_evidence WHERE thread_id = 'returning'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert!(tombstoned_at.is_none());
    assert_eq!(body, "restored body");
    Ok(())
}

#[test]
fn snapshot_rejects_invalid_ranges_and_unrepresentable_scores() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let store = IntegrationsStore::open(temp.path())?;
    let ctx = ConnectorCtx {
        vault_root: temp.path().to_path_buf(),
        connector_id: EMAIL_CONNECTOR_ID.to_string(),
        account: "owner@example.com".to_string(),
        command_path: None,
    };
    let earlier =
        chrono::DateTime::parse_from_rfc3339("2026-08-20T10:00:00Z")?.with_timezone(&chrono::Utc);
    let later =
        chrono::DateTime::parse_from_rfc3339("2026-08-20T11:00:00Z")?.with_timezone(&chrono::Utc);
    let invalid_range = store.replace_email_thread_snapshot(
        &ctx,
        vec![ThreadEvidence {
            thread_id: "invalid".into(),
            occurred_from: later,
            occurred_to: earlier,
            body_text: "body".into(),
            href: None,
        }],
        Vec::new(),
    );
    assert!(invalid_range
        .unwrap_err()
        .to_string()
        .contains("occurred_from"));

    let score_overflow = store.replace_email_thread_snapshot(
        &ctx,
        vec![ThreadEvidence {
            thread_id: "valid".into(),
            occurred_from: earlier,
            occurred_to: later,
            body_text: "body".into(),
            href: None,
        }],
        vec![ParticipantThread {
            participant: "peer@example.test".into(),
            thread_id: "valid".into(),
            last_interaction: later,
            sampling_score: Some(u64::MAX),
        }],
    );
    assert!(score_overflow
        .unwrap_err()
        .to_string()
        .contains("SQLite integer range"));
    assert!(store.thread_evidence(&ctx)?.is_empty());
    Ok(())
}
