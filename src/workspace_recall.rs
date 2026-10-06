//! Margins' side of Workspace recall: map engine document refs back to
//! Margins provenance (Sources, native paths, external records).
//!
//! The engine reads the Workspace program itself (`ENZYME_HOME=$MARGINS_HOME`)
//! and expands host sources through `margins-sources.enzyme`; nothing here
//! builds engine input.

use anyhow::{Context, Result};
use margins_workflows::integrations::EvidenceHandle;
use margins_workflows::workspace::{ResolvedWorkspace, SourceKind, WorkspaceBinding};
use rusqlite::Connection;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) struct CatalogEntry {
    pub source: String,
    pub kind: SourceKind,
    pub evidence: EvidenceHandle,
}

/// Create the Workspace ledger if it does not exist. Every program declares a
/// captures source and may declare ledger sources, whose kinds read
/// `ledger.db`; the engine refuses to index a declared database that is
/// missing.
pub(crate) fn ensure_ledger(workspace: &ResolvedWorkspace) -> Result<()> {
    if !workspace.ledger_path().is_file() {
        margins_workflows::integrations::IntegrationsStore::open(&workspace.state_dir)
            .context("creating the Workspace ledger")?;
    }
    Ok(())
}

/// Markdown identity under the Workspace language: with one Markdown source
/// refs are root-relative; with several they are `<source name>/<relative>`.
/// Returns the declaring source name and the absolute path of a native ref.
pub(crate) fn markdown_document(
    workspace: &ResolvedWorkspace,
    document_ref: &str,
) -> Option<(String, PathBuf)> {
    if document_ref.starts_with("sqlite:") {
        return None;
    }
    let markdown = workspace
        .config
        .bindings
        .iter()
        .filter_map(|(name, binding)| match binding {
            WorkspaceBinding::NativeMarkdown { path, .. } => Some((name, path)),
            _ => None,
        })
        .collect::<Vec<_>>();
    match markdown.as_slice() {
        [] => None,
        [(name, path)] => Some(((*name).clone(), path.join(document_ref))),
        many => {
            let (root, relative) = document_ref.split_once('/')?;
            many.iter()
                .find(|(name, _)| name.as_str() == root)
                .map(|(name, path)| ((*name).clone(), path.join(relative)))
        }
    }
}

/// The ledger record ids the source `name` indexes, as its kind's query
/// selects them (tombstoned records excluded).
pub(crate) fn ledger_source_ids(workspace: &ResolvedWorkspace, name: &str) -> Result<Vec<String>> {
    if !workspace.ledger_path().is_file() {
        return Ok(Vec::new());
    }
    let (sql, connector, account) = match workspace.config.bindings.get(name) {
        Some(WorkspaceBinding::Gmail { account, .. }) => (
            "SELECT thread_id FROM thread_evidence
             WHERE connector_id = ?1 AND source_account = ?2 AND tombstoned_at IS NULL",
            "email",
            account,
        ),
        Some(WorkspaceBinding::GoogleCalendar { account, .. }) => (
            "SELECT source_id FROM calendar_event_evidence
             WHERE connector_id = ?1 AND source_account = ?2 AND tombstoned_at IS NULL",
            "gcal",
            account,
        ),
        Some(WorkspaceBinding::GoogleMeet { account }) => (
            "SELECT source_id FROM external_document_evidence
             WHERE connector_id = ?1 AND source_account = ?2 AND tombstoned_at IS NULL",
            "google_meet",
            account,
        ),
        Some(WorkspaceBinding::Granola { account, .. }) => (
            "SELECT source_id FROM external_document_evidence
             WHERE connector_id = ?1 AND source_account = ?2 AND tombstoned_at IS NULL",
            "granola",
            account,
        ),
        _ => return Ok(Vec::new()),
    };
    let connection = Connection::open_with_flags(
        workspace.ledger_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut statement = connection.prepare(sql)?;
    let ids = statement
        .query_map([connector, account.as_str()], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use margins_workflows::workspace::SourceRole;

    #[test]
    fn markdown_document_identity_follows_the_workspace_language() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins");
        let notes = temp.path().join("notes");
        let reference = temp.path().join("reference");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::create_dir_all(&reference).unwrap();
        let mut workspace =
            margins_workflows::workspace::create_workspace(&margins_home, "practice", None, &notes)
                .unwrap();
        let home_name = workspace
            .config
            .bindings
            .iter()
            .find(|(_, binding)| matches!(binding, WorkspaceBinding::NativeMarkdown { .. }))
            .map(|(name, _)| name.clone())
            .unwrap();

        // One Markdown source: root-relative refs.
        assert_eq!(
            markdown_document(&workspace, "home.md"),
            Some((home_name.clone(), notes.join("home.md")))
        );

        // Several Markdown sources: refs are qualified by the source name.
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
        assert_eq!(
            markdown_document(&workspace, "research/reference.md"),
            Some(("research".to_string(), reference.join("reference.md")))
        );
        assert_eq!(
            markdown_document(&workspace, &format!("{home_name}/home.md")),
            Some((home_name, notes.join("home.md")))
        );
        assert_eq!(markdown_document(&workspace, "sqlite:mail/00"), None);
    }
}
