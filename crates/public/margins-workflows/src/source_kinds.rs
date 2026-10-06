//! The source kinds Margins defines for Enzyme.
//!
//! A Workspace program declares `google-mail`, `google-calendar`,
//! `google-meet`, `granola`, and `margins-captures` sources. Their meaning is
//! written once, in the Workspace language, as `source kind` definitions in
//! `$MARGINS_HOME/configs/margins-sources.enzyme`. Enzyme reads that file from
//! its config directory (`ENZYME_HOME=$MARGINS_HOME`) and expands each
//! declaration to a SQLite source over the Workspace's `ledger.db`; Margins
//! validates programs with the same definitions through
//! [`enzyme_spec::Environment`], so both read identical programs.
//!
//! `margins-captures` names where Margins keeps recordings. Its kind indexes
//! nothing (the query returns no rows), so a capture store is never part of
//! the recall corpus and never changes Markdown document identity.

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::OnceLock;

use crate::workspace::{atomic_write, CONFIGS_DIR};

/// File name of the managed kinds program under `configs/`.
pub const SOURCES_PROGRAM: &str = "margins-sources.enzyme";

/// The shipped text of [`SOURCES_PROGRAM`].
pub const SOURCES_TEXT: &str = include_str!("../resources/margins-sources.enzyme");

/// The parsed shipped kinds.
pub fn sources_program() -> &'static enzyme_spec::Program {
    static PROGRAM: OnceLock<enzyme_spec::Program> = OnceLock::new();
    PROGRAM.get_or_init(|| {
        enzyme_spec::parse(SOURCES_TEXT).expect("shipped margins-sources.enzyme must parse")
    })
}

/// What resolution knows in a Margins home: the user's home for `~`, the
/// Margins home as the Enzyme home for `{home}`, and Margins' source kinds.
pub fn environment(margins_home: &Path) -> Result<enzyme_spec::Environment> {
    let user_home = dirs::home_dir().unwrap_or_else(|| "/".into());
    let mut environment =
        enzyme_spec::Environment::new(user_home).with_enzyme_home(margins_home);
    for kind in sources_program().source_kinds.values() {
        environment.register_kind(kind.clone())?;
    }
    Ok(environment)
}

/// Write the shipped kinds to `configs/margins-sources.enzyme` unless the file
/// already holds exactly that text.
pub fn ensure_sources_program(margins_home: &Path) -> Result<()> {
    let path = margins_home.join(CONFIGS_DIR).join(SOURCES_PROGRAM);
    match std::fs::read_to_string(&path) {
        Ok(current) if current == SOURCES_TEXT => return Ok(()),
        Ok(_) => log::info!("restoring managed {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    }
    atomic_write(&path, SOURCES_TEXT.as_bytes())
}

/// Whether a declaration of `kind` reads the Workspace ledger.
pub fn is_ledger_kind(kind: &str) -> bool {
    matches!(
        kind,
        crate::workspace_program::SOURCE_GOOGLE_MAIL
            | crate::workspace_program::SOURCE_GOOGLE_CALENDAR
            | crate::workspace_program::SOURCE_GOOGLE_MEET
            | crate::workspace_program::SOURCE_GRANOLA
    )
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

pub fn sqlite_document_ref_prefix(source_name: &str) -> String {
    format!("sqlite:{}/", sqlite_source_namespace(source_name))
}

/// The document ref of ledger record `id` in the source declared as
/// `source_name`: `sqlite:<name>/<lowercase hex of id>`, which the kinds in
/// `margins-sources.enzyme` emit through `document ref` (the in-process
/// Margins used the same refs, so existing indexes keep them).
pub fn ledger_document_ref(source_name: &str, id: &str) -> String {
    let hex: String = id.bytes().map(|byte| format!("{byte:02x}")).collect();
    format!("{}{hex}", sqlite_document_ref_prefix(source_name))
}

/// The ledger record id behind a [`ledger_document_ref`], if `document_ref`
/// belongs to `source_name`.
pub fn ledger_record_id(source_name: &str, document_ref: &str) -> Option<String> {
    let hex = document_ref.strip_prefix(&sqlite_document_ref_prefix(source_name))?;
    if hex.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_kinds_parse_validate_and_expand() {
        let program = sources_program();
        let names: Vec<_> = program.source_kinds.keys().cloned().collect();
        assert_eq!(
            names,
            [
                "google-calendar",
                "google-mail",
                "google-meet",
                "granola",
                "margins-captures"
            ]
        );
        let home = tempfile::tempdir().unwrap();
        let environment = environment(home.path()).unwrap();
        let workspace = enzyme_spec::parse(
            r#"
workspace "practice" {
  source markdown "notes" { path "/notes" }
  source margins-captures "captures" { path "/captures" }
  source google-mail "mail" { account "it's@example.com" backfill days 30 }
  source google-calendar "calendar" { account "me@example.com" lookback days 7 }
  source google-meet "meet" { account "me@example.com" }
  source granola "granola" { account "me@example.com" time range "last_30_days" }
  learn questions from source "mail"
}
"#,
        )
        .unwrap();
        let resolved = enzyme_spec::resolve_in(vec![workspace], &environment).unwrap();
        let text = enzyme_spec::render_program(&resolved);
        let ledger = home.path().join("workspaces/practice/ledger.db");
        assert!(text.contains(ledger.to_str().unwrap()), "{text}");
        assert!(text.contains("'it''s@example.com'"), "{text}");
    }

    /// The mail account itself is never one of a thread's people, so it
    /// cannot become a link entity; Gmail's dotted and `+tag` spellings of the
    /// account are the same person.
    #[test]
    fn mail_people_leave_out_the_account_owner() {
        use crate::integrations::{
            ConnectorCtx, IntegrationsStore, ParticipantThread, ThreadEvidence,
            EMAIL_CONNECTOR_ID,
        };
        let home = tempfile::tempdir().unwrap();
        let state = home.path().join("workspaces/practice");
        let account = "Jo.Owner+work@Gmail.com";
        let store = IntegrationsStore::open(&state).unwrap();
        let mail = ConnectorCtx {
            vault_root: state.clone(),
            connector_id: EMAIL_CONNECTOR_ID.to_string(),
            account: account.to_string(),
            command_path: None,
        };
        let at = chrono::Utc::now();
        let participants = ["joowner@gmail.com", "jo.owner+work@gmail.com", "ada@client.test"];
        store
            .replace_email_thread_snapshot_with_materialization_fingerprint(
                &mail,
                vec![ThreadEvidence {
                    thread_id: "t1".into(),
                    occurred_from: at,
                    occurred_to: at,
                    body_text: "Vendor shortlist".into(),
                    href: None,
                }],
                participants
                    .iter()
                    .map(|participant| ParticipantThread {
                        participant: participant.to_string(),
                        thread_id: "t1".into(),
                        last_interaction: at,
                        sampling_score: Some(1),
                    })
                    .collect(),
                &crate::workspace::GmailCollectionSelector::default_declaration()
                    .materialization_fingerprint()
                    .unwrap(),
            )
            .unwrap();
        drop(store);

        let program = enzyme_spec::parse(&format!(
            "workspace \"practice\" {{\n  source google-mail \"mail\" {{ account {account:?} }}\n}}\n"
        ))
        .unwrap();
        let resolved =
            enzyme_spec::resolve_in(vec![program], &environment(home.path()).unwrap()).unwrap();
        let enzyme_spec::Source::Sqlite(source) = &resolved.workspaces[0].sources[0] else {
            panic!("google-mail must expand to a SQLite source");
        };
        let ledger = rusqlite::Connection::open(&source.db).unwrap();
        let people: String = ledger
            .query_row(
                &format!("SELECT participants FROM ({})", source.query),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(people, r#"["ada@client.test"]"#);
    }

    #[test]
    fn unknown_fields_are_still_errors() {
        let home = tempfile::tempdir().unwrap();
        let workspace = enzyme_spec::parse(
            r#"
workspace "practice" {
  source markdown "notes" { path "/notes" }
  source google-mail "mail" { account "me@example.com" backfil days 30 }
}
"#,
        )
        .unwrap();
        let error = enzyme_spec::resolve_in(vec![workspace], &environment(home.path()).unwrap())
            .unwrap_err();
        assert!(format!("{error:#}").contains("backfil days"), "{error:#}");
    }

    #[test]
    fn managed_file_is_written_and_restored() {
        let home = tempfile::tempdir().unwrap();
        ensure_sources_program(home.path()).unwrap();
        let path = home.path().join("configs").join(SOURCES_PROGRAM);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SOURCES_TEXT);
        std::fs::write(&path, "// edited\n").unwrap();
        ensure_sources_program(home.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SOURCES_TEXT);
    }

    #[test]
    fn ledger_document_refs_are_readable_and_round_trip() {
        assert_eq!(sqlite_source_namespace("my mail"), "my%20mail");
        assert_eq!(ledger_document_ref("mail", "t1"), "sqlite:mail/7431");
        for id in ["relay-thread-0", "événement/42", ""] {
            let reference = ledger_document_ref("mail", id);
            assert_eq!(ledger_record_id("mail", &reference).as_deref(), Some(id));
            assert_eq!(ledger_record_id("mail2", &reference), None);
        }
        assert_eq!(ledger_record_id("mail", "sqlite:mail/zz"), None);
        assert_eq!(ledger_record_id("mail", "sqlite:mail/7"), None);
    }
}
