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
use sha2::{Digest, Sha256};
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

/// The document ref Enzyme gives the ledger record `id` of a source declared
/// as `source_name`: `sqlite:<name>/<sha256>`, hashing the name's length
/// (little-endian `u64`), the name, and the JSON id tuple
/// `[{"type":"text","value":id}]`. A kind template cannot name its
/// declaration, so it cannot supply a readable `document ref` column.
pub fn sqlite_document_ref(source_name: &str, id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update((source_name.len() as u64).to_le_bytes());
    hasher.update(source_name.as_bytes());
    hasher.update(
        serde_json::to_vec(&serde_json::json!([{ "type": "text", "value": id }]))
            .expect("serializing a JSON literal cannot fail"),
    );
    format!(
        "{}{:x}",
        sqlite_document_ref_prefix(source_name),
        hasher.finalize()
    )
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
    fn sqlite_document_refs_hash_like_the_engine() {
        assert_eq!(sqlite_source_namespace("my mail"), "my%20mail");
        let reference = sqlite_document_ref("mail", "t1");
        assert!(reference.starts_with("sqlite:mail/"), "{reference}");
        assert_eq!(reference.len(), "sqlite:mail/".len() + 64);
        assert_ne!(reference, sqlite_document_ref("mail2", "t1"));
    }
}
