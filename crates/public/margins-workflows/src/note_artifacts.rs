//! Portable note frontmatter and note artifact helpers.
use serde_json::{Map, Value};
use std::path::Path;

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct NoteFrontmatter {
    pub created: Option<String>,
    pub created_sort: Option<String>,
    pub tags: Vec<String>,
    pub people: Vec<String>,
    pub people_present: bool,
    pub reflection_type: Option<String>,
    pub title: Option<String>,
    /// Durable backlink to the originating session, stamped into distilled
    /// notes at save time (`margins_session: <session name>`). Survives renames
    /// and DB resets, so the session lister can re-resolve a note even when the
    /// path-based link is gone. Absent on older notes (fully backward-compatible).
    pub margins_session: Option<String>,
}

pub fn read_note_frontmatter(path: Option<&str>) -> NoteFrontmatter {
    let Some(path) = path else {
        return NoteFrontmatter::default();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return NoteFrontmatter::default();
    };
    parse_note_frontmatter(&text)
}

pub fn update_people_frontmatter(text: &str, people: &[String]) -> String {
    let normalized = text.replace("\r\n", "\n");
    let lines: Vec<String> = normalized.lines().map(ToString::to_string).collect();
    let people_lines = render_people_frontmatter_lines(people);

    if lines.first().map(|line| line.trim()) == Some("---") {
        if let Some(end) = lines.iter().enumerate().skip(1).find_map(|(idx, line)| {
            if line.trim() == "---" {
                Some(idx)
            } else {
                None
            }
        }) {
            let mut updated = lines;
            if let Some(start) = updated[1..end]
                .iter()
                .position(|line| is_people_frontmatter_key(line))
                .map(|idx| idx + 1)
            {
                let mut remove_end = start + 1;
                while remove_end < end && is_frontmatter_continuation(&updated[remove_end]) {
                    remove_end += 1;
                }
                updated.splice(start..remove_end, people_lines);
            } else {
                updated.splice(end..end, people_lines);
            }
            return format!("{}\n", updated.join("\n"));
        }
    }

    let mut updated = Vec::new();
    updated.push("---".to_string());
    updated.extend(people_lines);
    updated.push("---".to_string());
    updated.push(String::new());
    updated.push(normalized.trim_start().to_string());
    format!("{}\n", updated.join("\n"))
}

fn render_people_frontmatter_lines(people: &[String]) -> Vec<String> {
    let mut lines = vec!["people:".to_string()];
    for person in people {
        let person = clean_frontmatter_scalar(person);
        if person.is_empty() {
            continue;
        }
        lines.push(format!("  - \"[[{}]]\"", person.replace('"', "\\\"")));
    }
    lines
}

fn is_people_frontmatter_key(line: &str) -> bool {
    if line.starts_with(char::is_whitespace) {
        return false;
    }
    let Some((key, _)) = line.split_once(':') else {
        return false;
    };
    matches!(key.trim(), "person" | "people" | "attendees")
}

fn is_frontmatter_continuation(line: &str) -> bool {
    line.trim().is_empty()
        || line.starts_with(char::is_whitespace)
        || line.trim_start().starts_with('-')
}

pub fn parse_note_frontmatter(text: &str) -> NoteFrontmatter {
    let normalized = text.strip_prefix('\u{feff}').unwrap_or(text);
    if !normalized.starts_with("---") {
        return NoteFrontmatter::default();
    }

    let Some(rest) = normalized.strip_prefix("---") else {
        return NoteFrontmatter::default();
    };
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
        .unwrap_or(rest);
    let mut block_lines = Vec::new();
    for line in rest.lines() {
        if line.trim() == "---" {
            return parse_frontmatter_block(&block_lines.join("\n"));
        }
        block_lines.push(line);
    }
    NoteFrontmatter::default()
}

fn parse_frontmatter_block(block: &str) -> NoteFrontmatter {
    let mut frontmatter = NoteFrontmatter::default();
    let mut current_key: Option<String> = None;

    for raw in block.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }

        let trimmed = line.trim_start();
        if trimmed.starts_with("- ") {
            if let Some(key) = current_key.as_deref() {
                frontmatter_push_value(&mut frontmatter, key, trimmed.trim_start_matches("- "));
            }
            continue;
        }

        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }

        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        let value = value.trim();
        current_key = Some(key.clone());
        mark_frontmatter_key(&mut frontmatter, &key);
        if !value.is_empty() {
            for item in parse_frontmatter_values(value) {
                frontmatter_push_value(&mut frontmatter, &key, &item);
            }
        }
    }

    frontmatter.tags.sort();
    frontmatter.tags.dedup();
    frontmatter.people.sort();
    frontmatter.people.dedup();
    frontmatter.created_sort = frontmatter
        .created
        .as_deref()
        .and_then(frontmatter_created_sort_key);
    frontmatter
}

fn mark_frontmatter_key(frontmatter: &mut NoteFrontmatter, key: &str) {
    match key {
        "person" | "people" | "attendees" => frontmatter.people_present = true,
        _ => {}
    }
}

fn frontmatter_push_value(frontmatter: &mut NoteFrontmatter, key: &str, raw: &str) {
    let value = clean_frontmatter_scalar(raw);
    if value.is_empty() || value == "[]" {
        return;
    }

    match key {
        "tag" | "tags" => frontmatter
            .tags
            .push(value.trim_start_matches('#').to_string()),
        "person" | "people" | "attendees" => frontmatter.people.push(clean_wikilink(&value)),
        "reflectionType" | "reflection_type" | "type" => frontmatter.reflection_type = Some(value),
        "created" | "created_at" | "date" => frontmatter.created = Some(value),
        "margins_session" => frontmatter.margins_session = Some(value),
        "title" => frontmatter.title = Some(value),
        _ => {}
    }
}

fn parse_frontmatter_values(value: &str) -> Vec<String> {
    let value = value.trim();
    if value.starts_with('[') && value.ends_with(']') {
        return value[1..value.len() - 1]
            .split(',')
            .map(clean_frontmatter_scalar)
            .filter(|v| !v.is_empty())
            .collect();
    }
    vec![value.to_string()]
}

pub fn clean_frontmatter_scalar(value: &str) -> String {
    let mut cleaned = value.trim().trim_matches(',').trim().to_string();
    for _ in 0..3 {
        let trimmed = cleaned.trim();
        if trimmed.len() < 2 {
            break;
        }
        let quote = trimmed.chars().next().unwrap_or_default();
        if !matches!(quote, '"' | '\'') || !trimmed.ends_with(quote) {
            break;
        }
        let inner = &trimmed[1..trimmed.len() - 1];
        cleaned = match quote {
            '\'' => inner.replace("''", "'"),
            '"' => inner.replace("\\\"", "\""),
            _ => inner.to_string(),
        }
        .trim()
        .to_string();
    }
    cleaned.trim().to_string()
}

fn clean_wikilink(value: &str) -> String {
    let value = value.trim();
    value
        .strip_prefix("[[")
        .and_then(|v| v.strip_suffix("]]"))
        .unwrap_or(value)
        .to_string()
}

fn frontmatter_created_sort_key(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    for i in 0..bytes.len().saturating_sub(9) {
        if bytes[i].is_ascii_digit()
            && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)
            && bytes.get(i + 2).is_some_and(u8::is_ascii_digit)
            && bytes.get(i + 3).is_some_and(u8::is_ascii_digit)
            && bytes.get(i + 4) == Some(&b'-')
            && bytes.get(i + 5).is_some_and(u8::is_ascii_digit)
            && bytes.get(i + 6).is_some_and(u8::is_ascii_digit)
            && bytes.get(i + 7) == Some(&b'-')
            && bytes.get(i + 8).is_some_and(u8::is_ascii_digit)
            && bytes.get(i + 9).is_some_and(u8::is_ascii_digit)
        {
            return Some(value[i..i + 10].to_string());
        }
    }
    None
}

pub fn safe_note_file_name(name: &str) -> String {
    let stem = name
        .trim()
        .chars()
        .map(|ch| {
            if matches!(ch, '/' | '\\' | ':') {
                '-'
            } else {
                ch
            }
        })
        .collect::<String>();
    format!("{}.md", stem.trim().trim_end_matches(".md"))
}

pub fn ensure_people_files(
    vault: &Path,
    people_folder: &str,
    person_note_template: &str,
    people: &[String],
) -> Result<(), String> {
    let people_dir = vault.join(people_folder.trim());
    if !people_dir.exists() {
        return Ok(());
    }
    if !people_dir.is_dir() {
        return Err(format!(
            "Configured people folder is not a directory: {}",
            people_dir.display()
        ));
    }
    for person in people {
        let path = people_dir.join(safe_note_file_name(person));
        if !path.exists() {
            let content = person_note_template.replace("{{name}}", person);
            std::fs::write(&path, content)
                .map_err(|e| format!("Could not create people note {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

pub fn serde_yaml_like_frontmatter(text: &str) -> Map<String, Value> {
    let mut map = Map::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim().is_empty() || line.starts_with(' ') {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        let rest = rest.trim();
        if rest.is_empty() {
            let mut arr = Vec::new();
            while let Some(next) = lines.peek().copied() {
                let trimmed = next.trim();
                if !trimmed.starts_with('-') {
                    break;
                }
                let item = trimmed.trim_start_matches('-').trim().to_string();
                let item = clean_frontmatter_scalar(&item);
                arr.push(Value::String(item));
                let _ = lines.next();
            }
            map.insert(key, Value::Array(arr));
        } else {
            map.insert(key, Value::String(clean_frontmatter_scalar(rest)));
        }
    }
    map
}

pub fn render_frontmatter_map(map: &Map<String, Value>) -> String {
    let mut out = String::new();
    for (key, value) in map {
        match value {
            Value::Array(arr) => {
                out.push_str(&format!("{key}:\n"));
                for item in arr {
                    out.push_str(&format!(
                        "  - \"{}\"\n",
                        item.as_str().unwrap_or_default().replace('"', "\\\"")
                    ));
                }
            }
            Value::String(s) => out.push_str(&format!("{key}: '{}'\n", s.replace('\'', "''"))),
            _ => out.push_str(&format!("{key}: {value}\n")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_arrays_and_wikilinks() {
        let parsed = parse_note_frontmatter(
            r#"---
created: '[[2026-06-01]]'
tags: [meeting, "project"]
people:
  - "[[Ada Lovelace]]"
  - Grace Hopper
reflectionType: retro
---
# Body
"#,
        );

        assert_eq!(parsed.created.as_deref(), Some("[[2026-06-01]]"));
        assert_eq!(parsed.created_sort.as_deref(), Some("2026-06-01"));
        assert_eq!(parsed.tags, vec!["meeting", "project"]);
        assert_eq!(parsed.people, vec!["Ada Lovelace", "Grace Hopper"]);
        assert_eq!(parsed.reflection_type.as_deref(), Some("retro"));
    }

    #[test]
    fn parses_scalar_title() {
        let parsed = parse_note_frontmatter(
            r#"---
title: 'Customer Sync Recap'
---
# Body
"#,
        );

        assert_eq!(parsed.title.as_deref(), Some("Customer Sync Recap"));
    }

    #[test]
    fn parses_quoted_yaml_titles_without_literal_quote_wrappers() {
        for (raw, expected) in [
            (r#"title: "Customer Sync Recap""#, "Customer Sync Recap"),
            (r#"title: '"Customer Sync Recap"'"#, "Customer Sync Recap"),
            (r#"title: "\"Customer Sync Recap\"""#, "Customer Sync Recap"),
            (
                r#"title: 'Customer Sync''s Recap'"#,
                "Customer Sync's Recap",
            ),
        ] {
            let parsed = parse_note_frontmatter(&format!("---\n{raw}\n---\n# Body\n"));
            assert_eq!(parsed.title.as_deref(), Some(expected), "{raw}");
        }
    }

    #[test]
    fn yaml_like_frontmatter_normalizes_nested_quoted_title() {
        let map = serde_yaml_like_frontmatter(r#"title: '"Customer Sync Recap"'"#);
        assert_eq!(
            map.get("title").and_then(Value::as_str),
            Some("Customer Sync Recap")
        );
    }

    #[test]
    fn ignores_missing_or_unclosed_frontmatter() {
        assert_eq!(parse_note_frontmatter("# Body"), NoteFrontmatter::default());
        assert_eq!(
            parse_note_frontmatter("---\ntags: x"),
            NoteFrontmatter::default()
        );
    }

    #[test]
    fn sanitizes_people_note_file_names() {
        assert_eq!(
            safe_note_file_name("Team/Platform: Notes.md"),
            "Team-Platform- Notes.md"
        );
    }

    #[test]
    fn ensure_people_files_does_nothing_when_people_folder_is_absent() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();

        ensure_people_files(
            vault,
            "people",
            "# {{name}}\n",
            &["Ada Lovelace".to_string()],
        )
        .unwrap();

        assert!(!vault.join("people").exists());
    }

    #[test]
    fn ensure_people_files_maintains_pages_when_people_folder_exists() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let people_dir = vault.join("people");
        std::fs::create_dir_all(&people_dir).unwrap();
        std::fs::write(people_dir.join("Grace Hopper.md"), "existing\n").unwrap();

        ensure_people_files(
            vault,
            "people",
            "# {{name}}\n",
            &["Ada Lovelace".to_string(), "Grace Hopper".to_string()],
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(people_dir.join("Ada Lovelace.md")).unwrap(),
            "# Ada Lovelace\n"
        );
        assert_eq!(
            std::fs::read_to_string(people_dir.join("Grace Hopper.md")).unwrap(),
            "existing\n"
        );
    }

    #[test]
    fn renders_frontmatter_maps() {
        let mut map = Map::new();
        map.insert(
            "created".to_string(),
            Value::String("[[2026-06-01]]".to_string()),
        );
        map.insert(
            "people".to_string(),
            Value::Array(vec![Value::String("[[Ada]]".to_string())]),
        );
        let rendered = render_frontmatter_map(&map);
        assert!(rendered.contains("created: '[[2026-06-01]]'"));
        assert!(rendered.contains("people:\n  - \"[[Ada]]\""));
    }

    #[test]
    fn updates_existing_people_frontmatter() {
        let updated = update_people_frontmatter(
            r#"---
title: 'Customer Sync'
people:
  - "[[Ada]]"
created: '2026-06-01'
---
# Body
"#,
            &["Grace Hopper".to_string(), "Alan Kay".to_string()],
        );

        assert!(updated.contains("title: 'Customer Sync'\npeople:\n  - \"[[Grace Hopper]]\"\n  - \"[[Alan Kay]]\"\ncreated: '2026-06-01'"));
        assert_eq!(
            parse_note_frontmatter(&updated).people,
            vec!["Alan Kay", "Grace Hopper"]
        );
    }

    #[test]
    fn adds_people_frontmatter_when_missing() {
        let updated = update_people_frontmatter("# Body\n", &["Ada Lovelace".to_string()]);

        assert!(updated.starts_with("---\npeople:\n  - \"[[Ada Lovelace]]\"\n---\n\n# Body\n"));
        assert_eq!(
            parse_note_frontmatter(&updated).people,
            vec!["Ada Lovelace"]
        );
    }
}
