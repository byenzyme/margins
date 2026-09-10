//! Fixture-driven Google Meet connector acceptance tests. No live account is used.

use anyhow::{Context, Result};
use margins_workflows::integrations::{
    parse_meet_v2_bundle, Connector, ConnectorCtx, DriveTranscriptDocument, GoogleMeetConnector,
    GoogleMeetSnapshot, GoogleMeetTransport, HealthStatus, IntegrationsStore,
    GOOGLE_MEET_CONNECTOR_ID,
};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const DRIVE_DOCUMENTS: &str = include_str!("fixtures/google_meet/google_drive_documents.json");
const DOC_ALPHA: &str = include_str!("fixtures/google_meet/google_docs_doc_alpha.json");
const DOC_ORPHAN: &str = include_str!("fixtures/google_meet/google_docs_doc_orphan.json");
const MEET_CONFERENCES: &str = include_str!("fixtures/google_meet/meet_v2_conferences.json");

#[derive(Clone)]
struct SnapshotTransport {
    snapshot: GoogleMeetSnapshot,
}

#[derive(Clone)]
struct SequenceTransport {
    snapshots: Arc<Mutex<VecDeque<GoogleMeetSnapshot>>>,
}

impl GoogleMeetTransport for SequenceTransport {
    fn fetch(
        &self,
        _account: &str,
        cursor: Option<&serde_json::Value>,
    ) -> Result<GoogleMeetSnapshot> {
        assert!(cursor.is_none(), "Meet refresh must use complete snapshots");
        self.snapshots
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("unexpected Meet snapshot fetch"))
    }

    fn health(&self, _account: &str) -> Result<HealthStatus> {
        Ok(HealthStatus::Fresh)
    }
}

impl GoogleMeetTransport for SnapshotTransport {
    fn fetch(
        &self,
        _account: &str,
        _cursor: Option<&serde_json::Value>,
    ) -> Result<GoogleMeetSnapshot> {
        Ok(self.snapshot.clone())
    }

    fn health(&self, _account: &str) -> Result<HealthStatus> {
        Ok(HealthStatus::Fresh)
    }
}

fn context(vault_root: PathBuf) -> ConnectorCtx {
    ConnectorCtx {
        vault_root,
        connector_id: GOOGLE_MEET_CONNECTOR_ID.to_string(),
        account: "owner@margins.test".to_string(),
        command_path: None,
    }
}

fn recorded_snapshot() -> Result<GoogleMeetSnapshot> {
    let drive: serde_json::Value = serde_json::from_str(DRIVE_DOCUMENTS)?;
    let files = drive
        .get("files")
        .and_then(serde_json::Value::as_array)
        .context("Drive fixture has no files")?;
    let mut documents = Vec::new();
    for file in files {
        let id = file
            .get("id")
            .and_then(serde_json::Value::as_str)
            .context("Drive fixture file has no id")?;
        let body_json = match id {
            "doc-alpha" => DOC_ALPHA,
            "doc-orphan" => DOC_ORPHAN,
            other => anyhow::bail!("unexpected Drive fixture document {other}"),
        };
        let body_value: serde_json::Value = serde_json::from_str(body_json)?;
        documents.push(DriveTranscriptDocument {
            id: id.to_string(),
            name: file
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(id)
                .to_string(),
            created_time: file
                .get("createdTime")
                .and_then(serde_json::Value::as_str)
                .context("Drive fixture file has no createdTime")?
                .parse()?,
            modified_time: file
                .get("modifiedTime")
                .and_then(serde_json::Value::as_str)
                .map(str::parse)
                .transpose()?,
            body: body_value
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            web_view_link: file
                .get("webViewLink")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            raw: file.clone(),
        });
    }
    Ok(GoogleMeetSnapshot {
        meet_recordings_folder_id: "folder-meet-recordings".to_string(),
        documents,
        conferences: parse_meet_v2_bundle(MEET_CONFERENCES)?,
        next_cursor: Some(serde_json::json!({"modified_time": "2026-08-20T16:00:00+00:00"})),
    })
}

#[test]
fn fixture_snapshot_parses_recorded_outputs() -> Result<()> {
    let snapshot = recorded_snapshot()?;
    assert_eq!(snapshot.meet_recordings_folder_id, "folder-meet-recordings");
    assert_eq!(snapshot.documents.len(), 2);
    assert_eq!(snapshot.conferences.len(), 1);
    assert!(snapshot.documents[0].body.contains("smaller scope"));
    Ok(())
}

#[test]
fn complete_snapshot_tombstones_documents_missing_from_the_next_refresh() -> Result<()> {
    let full = recorded_snapshot()?;
    let mut reduced = full.clone();
    reduced
        .documents
        .retain(|document| document.id == "doc-alpha");
    let transport = SequenceTransport {
        snapshots: Arc::new(Mutex::new(VecDeque::from([full, reduced]))),
    };
    let temp = tempfile::tempdir()?;
    let ctx = context(temp.path().to_path_buf());
    let connector = GoogleMeetConnector::new(transport);
    let store = IntegrationsStore::open(temp.path())?;

    assert_eq!(connector.reconcile(&ctx, None)?.records_written, 2);
    let second = connector.reconcile(&ctx, None)?;
    assert_eq!(second.tombstones, 1);
    let evidence = store.external_document_evidence(&ctx)?;
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].source_id, "doc-alpha");
    assert!(store
        .external_document_participants(&ctx)?
        .iter()
        .all(|participant| participant.source_id == "doc-alpha"));
    Ok(())
}

#[test]
fn reconcile_is_idempotent_and_preserves_identity_boundaries() -> Result<()> {
    let snapshot = recorded_snapshot()?;
    let temp = tempfile::tempdir()?;
    let ctx = context(temp.path().to_path_buf());
    let connector = GoogleMeetConnector::new(SnapshotTransport { snapshot });
    let store = IntegrationsStore::open(temp.path())?;

    let first = connector.reconcile(&ctx, None)?;
    assert_eq!(first.records_written, 2);
    assert_eq!(first.records_updated, 0);

    let second = connector.reconcile(&ctx, None)?;
    assert_eq!(second.records_written, 0);
    assert_eq!(second.records_updated, 0);
    assert_eq!(second.records_unchanged, 2);
    let evidence = store.external_document_evidence(&ctx)?;
    assert_eq!(evidence.len(), 2);
    let alpha = evidence
        .iter()
        .find(|document| document.source_id == "doc-alpha")
        .unwrap();
    assert!(alpha.body_text.contains("## Transcript"));
    assert_eq!(
        alpha.href.as_deref(),
        Some("https://docs.google.com/document/d/doc-alpha/edit")
    );
    assert_eq!(alpha.attributes["organizations"][0], "acme.test");
    let participants = store.external_document_participants(&ctx)?;
    assert!(participants.iter().any(|participant| {
        participant.source_id == "doc-alpha"
            && participant.email.as_deref() == Some("alice@acme.test")
            && !participant.ambiguous
    }));
    assert!(participants.iter().any(|participant| {
        participant.source_id == "doc-alpha"
            && participant.display_name == "Kevin"
            && participant.ambiguous
            && participant.email.is_none()
    }));
    assert!(
        !temp.path().join("Margins/Transcripts/google_meet").exists(),
        "Meet must not require a Markdown projection"
    );
    Ok(())
}

#[test]
fn ambiguous_document_to_conference_correlation_keeps_every_person_evidence_only() -> Result<()> {
    let mut snapshot = recorded_snapshot()?;
    let mut conflicting = snapshot.conferences[0].clone();
    conflicting.name = "conferenceRecords/conf-conflicting".to_string();
    conflicting.transcripts[0].name =
        "conferenceRecords/conf-conflicting/transcripts/other".to_string();
    snapshot.conferences.push(conflicting);

    let temp = tempfile::tempdir()?;
    let ctx = context(temp.path().to_path_buf());
    let connector = GoogleMeetConnector::new(SnapshotTransport { snapshot });
    connector.reconcile(&ctx, None)?;

    let store = IntegrationsStore::open(temp.path())?;
    let participants = store.external_document_participants(&ctx)?;
    assert!(participants
        .iter()
        .filter(|participant| participant.source_id == "doc-alpha")
        .all(|participant| participant.ambiguous));
    let alpha = store
        .external_document_evidence(&ctx)?
        .into_iter()
        .find(|document| document.source_id == "doc-alpha")
        .unwrap();
    assert!(alpha.attributes["identity_flags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|flag| flag.as_str().is_some_and(|flag| flag.contains("matched 2"))));
    assert!(alpha.attributes["organizations"]
        .as_array()
        .unwrap()
        .is_empty());
    Ok(())
}

#[test]
fn tombstoned_meet_documents_remain_purgeable_after_binding_removal() -> Result<()> {
    let margins_home = tempfile::tempdir()?;
    let notes = margins_home.path().join("notes");
    std::fs::create_dir_all(&notes)?;
    std::fs::write(notes.join("home.md"), "# Home\n")?;
    let mut workspace = margins_workflows::workspace::create_workspace(
        margins_home.path(),
        "meet-retention",
        None,
        &notes,
    )?;
    margins_workflows::workspace::add_source(
        &mut workspace,
        "meet",
        margins_workflows::workspace::WorkspaceBinding::GoogleMeet {
            account: "owner@margins.test".to_string(),
        },
    )?;

    let full = recorded_snapshot()?;
    let mut reduced = full.clone();
    reduced
        .documents
        .retain(|document| document.id == "doc-alpha");
    let transport = SequenceTransport {
        snapshots: Arc::new(Mutex::new(VecDeque::from([full, reduced]))),
    };
    let ctx = context(workspace.state_dir.clone());
    let connector = GoogleMeetConnector::new(transport);
    connector.reconcile(&ctx, None)?;
    connector.reconcile(&ctx, None)?;
    margins_workflows::workspace::remove_source(&mut workspace, "meet")?;

    use margins_workflows::integrations::{preview_retention, RetentionScope, RetentionTarget};
    use margins_workflows::workspace::workspace_revision;
    let target = RetentionTarget {
        connector_id: GOOGLE_MEET_CONNECTOR_ID.to_string(),
        source_account: "owner@margins.test".to_string(),
    };
    let plan = preview_retention(&workspace, &target, RetentionScope::Tombstones)?;
    assert_eq!(plan.target.connector_id, GOOGLE_MEET_CONNECTOR_ID);
    assert_eq!(plan.counts.tombstoned_evidence, 1);
    assert_eq!(workspace_revision(&workspace.config)?, plan.revision);
    Ok(())
}
