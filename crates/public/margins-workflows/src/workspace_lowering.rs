//! Host-source lowering: the one place Margins turns its own source kinds into
//! the native sources the recall engine indexes.
//!
//! A Workspace program declares `google-mail`, `google-calendar`,
//! `google-meet`, `granola`, and `margins-captures` sources. Before a program
//! reaches `enzyme_spec::resolve` (for validation or for indexing), Margins:
//!
//! - lowers each ledger-backed host source to a read-only `sqlite` source over
//!   the Workspace's `ledger.db`, keeping the source name so readings such as
//!   `learn questions from source "mail"` keep resolving;
//! - drops `margins-captures` sources. The capture registry is Margins policy
//!   (where recordings and transcripts live), not an indexed corpus; recall has
//!   never indexed it, and lowering it to a Markdown source would both change
//!   what recall can see and turn a one-Markdown Workspace into a multi-root one,
//!   changing every note's document identity;
//! - adds its hidden rules (the managed-projection tag exclusion).
//!
//! None of this is written to user files. Users never see ledger SQL.

use crate::integrations::GoogleCalendarScope;
use crate::workspace::WorkspaceBinding;
use crate::workspace_program::{host_binding, SOURCE_CAPTURES};
use anyhow::Result;
use chrono::{DateTime, Utc};
use enzyme_spec::{Program, Source, SqliteSource, SqliteWho};
use std::path::Path;

/// Tag on Markdown that Margins projects from its own records. Such notes are
/// never indexed: their content is already present as source evidence.
pub const MANAGED_PROJECTION_TAG: &str = "margins-managed-projection";

/// Lower a Workspace program for the engine. `ledger` is the Workspace's
/// `ledger.db`; it is only named in the lowered SQL, never opened here.
pub fn lower_for_engine(program: &Program, ledger: &Path, now: DateTime<Utc>) -> Result<Program> {
    let mut program = program.clone();
    for workspace in &mut program.workspaces {
        workspace
            .sources
            .retain(|source| !matches!(source, Source::Host(host) if host.kind == SOURCE_CAPTURES));
        if !workspace
            .excluded_tags
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case(MANAGED_PROJECTION_TAG))
        {
            workspace
                .excluded_tags
                .push(MANAGED_PROJECTION_TAG.to_string());
        }
    }
    program.lower_host_sources(|_, host| {
        let binding = host_binding(host)?;
        Ok(Some(Source::Sqlite(ledger_source(&host.name, &binding, ledger, now)?)))
    })?;
    Ok(program)
}

/// The engine's escaping of a SQLite source name inside `sqlite:<name>/…`
/// document refs and collection entities.
pub fn sqlite_source_namespace(source_name: &str) -> String {
    let mut escaped = String::new();
    for byte in source_name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            escaped.push(char::from(byte));
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    escaped
}

/// Exact document ref of one ledger record in a lowered source:
/// `sqlite:<source>/<hex(id)>`.
pub fn sqlite_document_ref(source_name: &str, id: &str) -> String {
    format!(
        "{}{}",
        sqlite_document_ref_prefix(source_name),
        hex(id.as_bytes())
    )
}

pub fn sqlite_document_ref_prefix(source_name: &str) -> String {
    format!("sqlite:{}/", sqlite_source_namespace(source_name))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn ledger_source(
    name: &str,
    binding: &WorkspaceBinding,
    ledger: &Path,
    now: DateTime<Utc>,
) -> Result<SqliteSource> {
    let prefix = sqlite_document_ref_prefix(name);
    let (query, what) = match binding {
        WorkspaceBinding::Gmail { account, .. } => {
            (mail_thread_query(&prefix, "email", account), vec!["body"])
        }
        WorkspaceBinding::GoogleCalendar { account, calendar } => (
            calendar_event_query(
                &prefix,
                "gcal",
                account,
                GoogleCalendarScope::for_selector(calendar, now)?,
            ),
            vec!["title", "body"],
        ),
        WorkspaceBinding::GoogleMeet { account } => (
            external_document_query(&prefix, "google_meet", account),
            vec!["title", "body"],
        ),
        WorkspaceBinding::Granola { account, .. } => (
            external_document_query(&prefix, "granola", account),
            vec!["title", "body"],
        ),
        WorkspaceBinding::NativeMarkdown { .. } | WorkspaceBinding::Captures { .. } => {
            anyhow::bail!("source {name:?} is not a ledger source")
        }
    };
    Ok(SqliteSource {
        name: name.to_string(),
        db: ledger.to_string_lossy().into_owned(),
        query,
        id: vec!["id".to_string()],
        document_ref: Some("document_ref".to_string()),
        who: SqliteWho::JsonArray {
            column: "participants".to_string(),
        },
        when: "occurred_at_ms".to_string(),
        what: what.into_iter().map(str::to_string).collect(),
        where_columns: Vec::new(),
        weight: None,
        timestamp_unit: "ms".to_string(),
        timestamp_epoch: None,
        filter: None,
    })
}

/// One stable row per materialized email thread.
fn mail_thread_query(prefix: &str, connector: &str, account: &str) -> String {
    format!(
        "SELECT te.thread_id AS id, \
         {} || lower(hex(CAST(te.thread_id AS BLOB))) AS document_ref, \
         CAST(strftime('%s', te.occurred_to) AS INTEGER) * 1000 AS occurred_at_ms, \
         te.body_text AS body, \
         COALESCE((SELECT json_group_array(ordered.participant) FROM ( \
             SELECT DISTINCT pt.participant FROM participant_threads pt \
             WHERE pt.connector_id = te.connector_id \
               AND pt.source_account = te.source_account \
               AND pt.thread_id = te.thread_id \
             ORDER BY pt.participant \
         ) ordered), '[]') AS participants \
         FROM thread_evidence te \
         WHERE te.connector_id = {} AND te.source_account = {} \
           AND te.tombstoned_at IS NULL",
        sql_string(prefix),
        sql_string(connector),
        sql_string(account),
    )
}

/// One stable row per authoritative Calendar event inside the rolling scope.
/// The engine receives a generic document plus role-blind participants; it
/// never reads Calendar projections or raw transport payloads.
fn calendar_event_query(
    prefix: &str,
    connector: &str,
    account: &str,
    scope: GoogleCalendarScope,
) -> String {
    format!(
        "SELECT ce.source_id AS id, \
         {} || lower(hex(CAST(ce.source_id AS BLOB))) AS document_ref, \
         CAST(strftime('%s', ce.occurred_from) AS INTEGER) * 1000 AS occurred_at_ms, \
         ce.title, ce.body_text AS body, \
         COALESCE((SELECT json_group_array(participant) FROM ( \
             SELECT COALESCE(NULLIF(ca.email, ''), ca.display_name) AS participant \
             FROM calendar_event_attendees ca \
             WHERE ca.connector_id = ce.connector_id \
               AND ca.source_account = ce.source_account \
               AND ca.source_id = ce.source_id \
             ORDER BY ca.position, ca.attendee_key \
         )), '[]') AS participants \
         FROM calendar_event_evidence ce \
         WHERE ce.connector_id = {} AND ce.source_account = {} \
           AND ce.tombstoned_at IS NULL \
           AND ce.occurred_from >= {} AND ce.occurred_from <= {}",
        sql_string(prefix),
        sql_string(connector),
        sql_string(account),
        sql_string(&scope.occurred_from.to_rfc3339()),
        sql_string(&scope.occurred_to.to_rfc3339()),
    )
}

/// One stable row per authoritative external evidence document (Google Meet,
/// Granola). Provider attributes stay on the Margins side of the boundary.
fn external_document_query(prefix: &str, connector: &str, account: &str) -> String {
    format!(
        "SELECT ed.source_id AS id, \
         {} || lower(hex(CAST(ed.source_id AS BLOB))) AS document_ref, \
         CAST(strftime('%s', ed.occurred_at) AS INTEGER) * 1000 AS occurred_at_ms, \
         ed.title, ed.body_text AS body, \
         COALESCE((SELECT json_group_array(participant) FROM ( \
             SELECT COALESCE(NULLIF(ep.email, ''), ep.display_name) AS participant \
             FROM external_document_participants ep \
             WHERE ep.connector_id = ed.connector_id \
               AND ep.source_account = ed.source_account \
               AND ep.source_id = ed.source_id \
               AND ep.ambiguous = 0 \
             ORDER BY ep.position, ep.participant_key \
         )), '[]') AS participants \
         FROM external_document_evidence ed \
         WHERE ed.connector_id = {} AND ed.source_account = {} \
           AND ed.tombstoned_at IS NULL",
        sql_string(prefix),
        sql_string(connector),
        sql_string(account),
    )
}

fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace_program::WorkspaceProgram;

    #[test]
    fn host_sources_lower_to_named_ledger_sources_and_captures_drop() {
        let program = WorkspaceProgram::parse(
            r#"
workspace "practice" {
  source markdown "notes" { path "/notes" }
  source margins-captures "captures" { path "/captures" }
  source google-mail "mail" { account "me@example.com" }
  source google-calendar "calendar" { account "me@example.com" }
  remember in folder "inbox" create note
  learn questions from source "mail"
}
"#,
        )
        .unwrap();
        let lowered =
            lower_for_engine(program.program(), Path::new("/state/ledger.db"), Utc::now()).unwrap();
        let workspace = &lowered.workspaces[0];
        let names: Vec<_> = workspace.sources.iter().map(|s| (s.kind(), s.name())).collect();
        assert_eq!(
            names,
            [("markdown", "notes"), ("sqlite", "mail"), ("sqlite", "calendar")]
        );
        let Source::Sqlite(mail) = &workspace.sources[1] else {
            unreachable!()
        };
        assert_eq!(mail.db, "/state/ledger.db");
        assert!(mail.query.contains("'sqlite:mail/'"), "{}", mail.query);
        assert!(workspace
            .excluded_tags
            .contains(&MANAGED_PROJECTION_TAG.to_string()));
        enzyme_spec::resolve(vec![lowered], Path::new("/")).unwrap();
    }

    #[test]
    fn sqlite_document_refs_escape_like_the_engine() {
        assert_eq!(sqlite_document_ref("mail", "t1"), "sqlite:mail/7431");
        assert_eq!(sqlite_source_namespace("my mail"), "my%20mail");
    }
}
