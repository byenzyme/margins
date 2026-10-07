//! What Margins learns about, remembered between index refreshes.
//!
//! After every successful index build or refresh ([`crate::recall`]'s single
//! provisioning path, used by init, sync, reconcile, imports, and retention),
//! the engine's selection is saved as `attention.json` in the Workspace state
//! directory (never the notes folder). The next refresh compares against it,
//! so `margins sync` and `margins init` can say what changed: entities newly
//! learned about, no longer learned about, or changed state (for example
//! pending → ready, or ready → skipped and why). The first refresh records a
//! baseline instead of reporting every entity as new.

use crate::enzyme_cli::StatusEnvelope;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const ATTENTION_FILE: &str = "attention.json";
pub const ATTENTION_SCHEMA: &str = "margins.attention.v1";

/// Changes listed per category before "+N more".
pub const CHANGES_SHOWN: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: String,
    /// Workspace revision (program hash) the selection was made under.
    pub revision: String,
    pub captured_at_ms: i64,
    pub engine_version: String,
    pub documents: usize,
    pub catalysts: usize,
    pub entities: Vec<Entity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    #[serde(rename = "type")]
    pub entity_type: String,
    pub name: String,
    /// `reading` or `automatic`.
    pub origin: String,
    /// `ready`, `pending`, `unchecked`, or `skipped`.
    pub state: String,
    #[serde(default)]
    pub skip_kind: Option<String>,
    #[serde(default)]
    pub catalysts: usize,
}

impl Entity {
    fn key(&self) -> (String, String) {
        (self.entity_type.clone(), self.name.to_lowercase())
    }

    fn label(&self) -> String {
        match self.entity_type.as_str() {
            "folder" => format!("{} (folder)", self.name),
            "tag" => format!("#{}", self.name.trim_start_matches('#')),
            _ => self.name.clone(),
        }
    }

    fn state_label(&self) -> String {
        match (self.state.as_str(), self.skip_kind.as_deref()) {
            ("skipped", Some(kind)) => format!("skipped: {}", kind.replace('_', " ")),
            ("pending" | "unchecked", _) => "building".to_string(),
            (state, _) => state.to_string(),
        }
    }
}

/// What changed in attention between two refreshes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diff {
    /// No earlier snapshot: this refresh recorded the baseline.
    pub baseline: bool,
    pub from_revision: Option<String>,
    pub to_revision: String,
    /// `program_changed` (the Workspace program changed), `notes_changed`, or
    /// `none` when nothing changed.
    pub cause: &'static str,
    pub added: Vec<Entity>,
    pub removed: Vec<Entity>,
    pub changed: Vec<Change>,
    /// Selected entities still without catalysts: being built, or waiting
    /// for a catalyst generator to be set up (`generating` is false).
    pub building: usize,
    pub generating: bool,
    /// One readable line, e.g. "+ Alice · − Projects (folder) · 2 building".
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    #[serde(flatten)]
    pub entity: Entity,
    pub from_state: String,
    pub from_skip_kind: Option<String>,
    /// `reading` or `automatic` before; a reading removed from the program
    /// can leave the engine picking the same entity automatically.
    pub from_origin: String,
}

pub fn snapshot(status: &StatusEnvelope, revision: &str, engine_version: &str) -> Snapshot {
    let entities = status
        .selection
        .as_ref()
        .map(|selection| {
            selection
                .entities
                .iter()
                .map(|entity| Entity {
                    entity_type: entity.entity_type.clone(),
                    name: entity.name.clone(),
                    origin: entity.origin.clone().unwrap_or_else(|| "reading".to_string()),
                    state: entity.state.clone(),
                    skip_kind: entity
                        .skip_reason
                        .as_ref()
                        .and_then(|reason| reason.get("kind"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    catalysts: entity.catalysts,
                })
                .collect()
        })
        .unwrap_or_default();
    Snapshot {
        schema_version: ATTENTION_SCHEMA.to_string(),
        revision: revision.to_string(),
        captured_at_ms: chrono::Utc::now().timestamp_millis(),
        engine_version: engine_version.to_string(),
        documents: status.documents,
        catalysts: status.catalysts,
        entities,
    }
}

/// The saved snapshot, or `None` when there is none or it cannot be read
/// (an unreadable snapshot only costs one baseline).
pub fn load(state_dir: &Path) -> Option<Snapshot> {
    let text = std::fs::read_to_string(state_dir.join(ATTENTION_FILE)).ok()?;
    serde_json::from_str::<Snapshot>(&text)
        .ok()
        .filter(|snapshot| snapshot.schema_version == ATTENTION_SCHEMA)
}

pub fn save(state_dir: &Path, snapshot: &Snapshot) -> Result<()> {
    let path = state_dir.join(ATTENTION_FILE);
    let temp = state_dir.join(format!(".{ATTENTION_FILE}.tmp"));
    std::fs::write(&temp, serde_json::to_vec_pretty(snapshot)?)
        .with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, &path).with_context(|| format!("writing {}", path.display()))
}

/// `generating`: whether a catalyst generator is set up, so entities without
/// catalysts are being built rather than waiting for one.
pub fn diff(previous: Option<&Snapshot>, next: &Snapshot, generating: bool) -> Diff {
    let building = next
        .entities
        .iter()
        .filter(|entity| matches!(entity.state.as_str(), "pending" | "unchecked"))
        .count();
    let Some(previous) = previous else {
        let mut diff = Diff {
            baseline: true,
            from_revision: None,
            to_revision: next.revision.clone(),
            cause: "none",
            added: Vec::new(),
            removed: Vec::new(),
            changed: Vec::new(),
            building,
            generating,
            summary: String::new(),
        };
        diff.summary = summarize(&diff, next);
        return diff;
    };
    let before = previous
        .entities
        .iter()
        .map(|entity| (entity.key(), entity))
        .collect::<BTreeMap<_, _>>();
    let after = next
        .entities
        .iter()
        .map(|entity| (entity.key(), entity))
        .collect::<BTreeMap<_, _>>();
    let added = after
        .iter()
        .filter(|(key, _)| !before.contains_key(*key))
        .map(|(_, entity)| (*entity).clone())
        .collect::<Vec<_>>();
    let removed = before
        .iter()
        .filter(|(key, _)| !after.contains_key(*key))
        .map(|(_, entity)| (*entity).clone())
        .collect::<Vec<_>>();
    let changed = after
        .iter()
        .filter_map(|(key, entity)| {
            let old = before.get(key)?;
            (old.state != entity.state
                || old.skip_kind != entity.skip_kind
                || old.origin != entity.origin)
                .then(|| Change {
                    entity: (*entity).clone(),
                    from_state: old.state.clone(),
                    from_skip_kind: old.skip_kind.clone(),
                    from_origin: old.origin.clone(),
                })
        })
        .collect::<Vec<_>>();
    let nothing = added.is_empty() && removed.is_empty() && changed.is_empty();
    let cause = if previous.revision != next.revision {
        "program_changed"
    } else if nothing {
        "none"
    } else {
        "notes_changed"
    };
    let mut diff = Diff {
        baseline: false,
        from_revision: Some(previous.revision.clone()),
        to_revision: next.revision.clone(),
        cause,
        added,
        removed,
        changed,
        building,
        generating,
        summary: String::new(),
    };
    diff.summary = summarize(&diff, next);
    diff
}

fn listed<T>(items: &[T], show: impl Fn(&T) -> String) -> String {
    let mut parts = items.iter().take(CHANGES_SHOWN).map(show).collect::<Vec<_>>();
    if items.len() > CHANGES_SHOWN {
        parts.push(format!("+{} more", items.len() - CHANGES_SHOWN));
    }
    parts.join(", ")
}

fn describe_change(change: &Change) -> String {
    let before = Entity {
        state: change.from_state.clone(),
        skip_kind: change.from_skip_kind.clone(),
        origin: change.from_origin.clone(),
        ..change.entity.clone()
    };
    let mut moves = Vec::new();
    if before.origin != change.entity.origin {
        moves.push(format!(
            "{} → {}",
            origin_label(&before.origin),
            origin_label(&change.entity.origin)
        ));
    }
    if before.state_label() != change.entity.state_label() {
        moves.push(format!("{} → {}", before.state_label(), change.entity.state_label()));
    }
    format!("{} ({})", change.entity.label(), moves.join("; "))
}

fn origin_label(origin: &str) -> &str {
    match origin {
        "automatic" => "picked automatically",
        "reading" => "from a reading",
        other => other,
    }
}

fn summarize(diff: &Diff, next: &Snapshot) -> String {
    let mut parts = Vec::new();
    if diff.baseline {
        parts.push(format!(
            "baseline recorded: learning about {} {}",
            next.entities.len(),
            if next.entities.len() == 1 { "thing" } else { "things" }
        ));
    } else {
        if !diff.added.is_empty() {
            parts.push(format!("+ {}", listed(&diff.added, Entity::label)));
        }
        if !diff.removed.is_empty() {
            parts.push(format!("− {}", listed(&diff.removed, Entity::label)));
        }
        if !diff.changed.is_empty() {
            parts.push(format!("~ {}", listed(&diff.changed, describe_change)));
        }
        if parts.is_empty() {
            parts.push("no change in what Margins learns about".to_string());
        } else if diff.cause == "program_changed" {
            parts.push("(program changed)".to_string());
        }
    }
    if diff.building > 0 {
        parts.push(if diff.generating {
            format!("{} still building", diff.building)
        } else {
            format!("{} waiting for catalysts", diff.building)
        });
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(name: &str, origin: &str, state: &str, skip: Option<&str>) -> Entity {
        Entity {
            entity_type: "link".to_string(),
            name: name.to_string(),
            origin: origin.to_string(),
            state: state.to_string(),
            skip_kind: skip.map(str::to_string),
            catalysts: usize::from(state == "ready"),
        }
    }

    fn snap(revision: &str, entities: Vec<Entity>) -> Snapshot {
        Snapshot {
            schema_version: ATTENTION_SCHEMA.to_string(),
            revision: revision.to_string(),
            captured_at_ms: 0,
            engine_version: "test".to_string(),
            documents: 3,
            catalysts: 0,
            entities,
        }
    }

    #[test]
    fn first_refresh_records_a_baseline_without_listing_everything_as_new() {
        let next = snap("a", vec![entity("alice", "automatic", "pending", None)]);
        let diff = diff(None, &next, true);
        assert!(diff.baseline);
        assert!(diff.added.is_empty());
        assert_eq!(diff.summary, "baseline recorded: learning about 1 thing · 1 still building");
    }

    #[test]
    fn diff_reports_added_removed_and_changed_with_the_cause() {
        let before = snap(
            "a",
            vec![
                entity("alice", "automatic", "pending", None),
                entity("projects", "reading", "ready", None),
            ],
        );
        let after = snap(
            "a",
            vec![
                entity("Alice", "automatic", "ready", None),
                entity("bob", "automatic", "skipped", Some("thin_context")),
            ],
        );
        let diff = diff(Some(&before), &after, true);
        assert_eq!(diff.cause, "notes_changed");
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(
            diff.summary,
            "+ bob · − projects · ~ Alice (building → ready)"
        );

        let program_changed = super::diff(Some(&before), &snap("b", before.entities.clone()), true);
        assert_eq!(program_changed.cause, "program_changed");
        assert_eq!(program_changed.summary, "no change in what Margins learns about · 1 still building");
        // A reading removed while the engine keeps picking it automatically.
        let mut moved = before.entities.clone();
        moved[1].origin = "automatic".to_string();
        let moved = super::diff(Some(&before), &snap("b", moved), true);
        assert_eq!(
            moved.summary,
            "~ projects (from a reading → picked automatically) · (program changed) · 1 still building"
        );
        let same = super::diff(Some(&before), &before, false);
        assert_eq!(same.summary, "no change in what Margins learns about · 1 waiting for catalysts");
        assert_eq!(same.cause, "none");
    }

    #[test]
    fn snapshot_round_trips_through_the_state_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), None);
        let snapshot = snap("a", vec![entity("alice", "reading", "ready", None)]);
        save(dir.path(), &snapshot).unwrap();
        assert_eq!(load(dir.path()), Some(snapshot));
    }
}
