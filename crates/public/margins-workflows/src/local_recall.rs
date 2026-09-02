//! Small, fully open local recall baseline.
//!
//! This implementation deliberately searches declared Markdown Sources at
//! query time. It needs no private engine, generated index, model, receipt, or
//! database schema. Official builds may replace it with richer retrieval, but
//! a public checkout can always complete a useful local setup and recall loop.

use crate::workspace::{ResolvedWorkspace, WorkspaceBinding};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

const MAX_DOCUMENT_BYTES: u64 = 2 * 1024 * 1024;
const DEFAULT_RESULT_LIMIT: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalRecallStatus {
    pub schema_version: &'static str,
    pub available: bool,
    pub mode: &'static str,
    pub documents: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalRecallEvidence {
    pub kind: &'static str,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalRecallResult {
    pub document_ref: String,
    pub source: String,
    pub score: usize,
    pub content: String,
    pub evidence: LocalRecallEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalRecallOutput {
    pub schema_version: &'static str,
    pub status: &'static str,
    pub reason: &'static str,
    pub query: String,
    pub search_strategy: &'static str,
    pub results: Vec<LocalRecallResult>,
    pub total_results: usize,
}

#[derive(Debug, Clone)]
struct LocalDocument {
    source: String,
    root: PathBuf,
    path: PathBuf,
    relative: PathBuf,
    body: String,
}

pub fn status(workspace: &ResolvedWorkspace) -> Result<LocalRecallStatus> {
    Ok(LocalRecallStatus {
        schema_version: "margins.local-recall.v1",
        available: true,
        mode: "live_lexical",
        documents: discover_documents(workspace, None)?.len(),
    })
}

pub fn search(
    workspace: &ResolvedWorkspace,
    query: &str,
    source_filter: Option<&str>,
) -> Result<LocalRecallOutput> {
    let query = query.trim();
    if query.is_empty() {
        bail!("recall query must not be empty");
    }
    let tokens = query_tokens(query);
    if tokens.is_empty() {
        bail!("recall query must contain a word or number");
    }

    let query_lower = query.to_lowercase();
    let mut results = discover_documents(workspace, source_filter)?
        .into_iter()
        .filter_map(|document| {
            let body_lower = document.body.to_lowercase();
            let title_lower = document
                .path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_lowercase();
            let score = lexical_score(&body_lower, &title_lower, &query_lower, &tokens);
            (score > 0).then(|| LocalRecallResult {
                document_ref: format!(
                    "native:{}/{}",
                    document.source,
                    document.relative.to_string_lossy().replace('\\', "/")
                ),
                source: document.source,
                score,
                content: matching_excerpt(&document.body, &query_lower, &tokens),
                evidence: LocalRecallEvidence {
                    kind: "native_markdown",
                    path: document.path,
                },
            })
        })
        .collect::<Vec<_>>();
    results.sort_by_key(|result| {
        (
            Reverse(result.score),
            result.source.clone(),
            result.document_ref.clone(),
        )
    });
    results.truncate(DEFAULT_RESULT_LIMIT);
    let total_results = results.len();
    Ok(LocalRecallOutput {
        schema_version: "margins.recall.v1",
        status: "ok",
        reason: "local_lexical",
        query: query.to_string(),
        search_strategy: "live_local_markdown",
        results,
        total_results,
    })
}

fn discover_documents(
    workspace: &ResolvedWorkspace,
    source_filter: Option<&str>,
) -> Result<Vec<LocalDocument>> {
    if let Some(source) = source_filter {
        if !workspace.config.bindings.contains_key(source) {
            bail!(
                "workspace '{}' has no declared source '{}'",
                workspace.config.id,
                source
            );
        }
    }

    let mut documents = Vec::new();
    for (source, binding) in &workspace.config.bindings {
        if source_filter.is_some_and(|filter| filter != source) {
            continue;
        }
        let WorkspaceBinding::NativeMarkdown { path: root, .. } = binding else {
            continue;
        };
        let exclusions = workspace
            .config
            .policy
            .excluded_folders
            .iter()
            .map(|folder| folder.trim().to_lowercase())
            .filter(|folder| !folder.is_empty())
            .chain(
                [".git", ".margins", ".enzyme", "node_modules"]
                    .into_iter()
                    .map(str::to_string),
            )
            .collect::<BTreeSet<_>>();
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| include_entry(entry, root, &exclusions))
            .filter_map(|entry| entry.ok())
        {
            if !entry.file_type().is_file() || !is_markdown(entry.path()) {
                continue;
            }
            let metadata = entry
                .metadata()
                .with_context(|| format!("reading Markdown metadata {}", entry.path().display()))?;
            if metadata.len() > MAX_DOCUMENT_BYTES {
                continue;
            }
            let body = match std::fs::read_to_string(entry.path()) {
                Ok(body) => body,
                Err(_) => continue,
            };
            if excluded_by_tag(&body, &workspace.config.policy.excluded_tags) {
                continue;
            }
            documents.push(LocalDocument {
                source: source.clone(),
                root: root.clone(),
                path: entry.path().to_path_buf(),
                relative: entry
                    .path()
                    .strip_prefix(root)
                    .unwrap_or(entry.path())
                    .to_path_buf(),
                body,
            });
        }
    }
    documents.sort_by(|left, right| {
        (&left.source, &left.root, &left.relative).cmp(&(
            &right.source,
            &right.root,
            &right.relative,
        ))
    });
    Ok(documents)
}

fn include_entry(entry: &DirEntry, root: &Path, exclusions: &BTreeSet<String>) -> bool {
    entry.path() == root
        || !entry
            .file_name()
            .to_str()
            .is_some_and(|name| exclusions.contains(&name.to_lowercase()))
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        })
}

fn excluded_by_tag(body: &str, excluded_tags: &[String]) -> bool {
    let lower = body.to_lowercase();
    excluded_tags.iter().any(|tag| {
        let tag = tag.trim().trim_start_matches('#').to_lowercase();
        !tag.is_empty() && lower.contains(&format!("#{tag}"))
    })
}

fn query_tokens(query: &str) -> Vec<String> {
    query
        .split(|character: char| !character.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|token| !token.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn lexical_score(body: &str, title: &str, query: &str, tokens: &[String]) -> usize {
    let phrase = body.matches(query).count();
    let title_phrase = title.matches(query).count();
    let token_score = tokens
        .iter()
        .map(|token| body.matches(token).count().min(20) + title.matches(token).count() * 8)
        .sum::<usize>();
    phrase * 100 + title_phrase * 200 + token_score
}

fn matching_excerpt(body: &str, query: &str, tokens: &[String]) -> String {
    let line = body
        .lines()
        .find(|line| line.to_lowercase().contains(query))
        .or_else(|| {
            body.lines().find(|line| {
                let lower = line.to_lowercase();
                tokens.iter().any(|token| lower.contains(token))
            })
        })
        .or_else(|| body.lines().find(|line| !line.trim().is_empty()))
        .unwrap_or_default()
        .trim();
    line.chars().take(320).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{create_workspace, update_policy, SourceRole, WorkspacePolicy};

    #[test]
    fn local_recall_searches_declared_sources_and_honors_exclusions() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(notes.join("projects")).unwrap();
        std::fs::create_dir_all(notes.join("templates")).unwrap();
        std::fs::write(
            notes.join("projects/atlas.md"),
            "# Atlas\nThe phosphorescent handoff preserves the decision boundary.\n",
        )
        .unwrap();
        std::fs::write(
            notes.join("templates/meeting.md"),
            "phosphorescent handoff should stay excluded",
        )
        .unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        let policy = WorkspacePolicy {
            excluded_folders: vec!["templates".to_string()],
            ..workspace.config.policy.clone()
        };
        update_policy(&mut workspace, policy).unwrap();

        let output = search(&workspace, "phosphorescent handoff", None).unwrap();
        assert_eq!(output.total_results, 1);
        assert_eq!(output.results[0].source, "home");
        assert!(output.results[0]
            .document_ref
            .ends_with("projects/atlas.md"));
        assert_eq!(output.results[0].evidence.kind, "native_markdown");
    }

    #[test]
    fn source_filter_is_explicit_and_read_only() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        let reference = temp.path().join("reference");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::create_dir_all(&reference).unwrap();
        std::fs::write(notes.join("home.md"), "shared phrase home").unwrap();
        std::fs::write(reference.join("reference.md"), "shared phrase reference").unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        crate::workspace::add_source(
            &mut workspace,
            "research",
            WorkspaceBinding::NativeMarkdown {
                path: reference,
                role: SourceRole::Reference,
            },
        )
        .unwrap();

        let output = search(&workspace, "shared phrase", Some("research")).unwrap();
        assert_eq!(output.total_results, 1);
        assert_eq!(output.results[0].source, "research");
        assert!(search(&workspace, "shared phrase", Some("missing")).is_err());
    }
}
