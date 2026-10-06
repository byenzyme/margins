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
    /// People entries that could not be interpreted without guessing. These
    /// remain available as source evidence instead of being projected as a
    /// corrupted identity.
    pub people_unparsed: Vec<String>,
    /// Human-readable reasons corresponding to rejected people evidence.
    pub people_parse_warnings: Vec<String>,
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
    parse_people_frontmatter(block, &mut frontmatter);
    let mut current_key: Option<String> = None;

    for raw in block.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }

        let trimmed = line.trim_start();
        if trimmed.starts_with("- ") {
            if let Some(key) = current_key.as_deref() {
                if !is_people_key(key) {
                    frontmatter_push_value(&mut frontmatter, key, trimmed.trim_start_matches("- "));
                }
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
        if !value.is_empty() && !is_people_key(&key) {
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
    if is_people_key(key) {
        frontmatter.people_present = true;
    }
}

fn is_people_key(key: &str) -> bool {
    matches!(key, "person" | "people" | "attendees")
}

fn parse_people_frontmatter(block: &str, frontmatter: &mut NoteFrontmatter) {
    let lines = block.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if line.starts_with(char::is_whitespace) {
            index += 1;
            continue;
        }
        let Some((key, inline)) = line.split_once(':') else {
            index += 1;
            continue;
        };
        if !is_people_key(key.trim()) {
            index += 1;
            continue;
        }

        frontmatter.people_present = true;
        let inline = inline.trim();
        if !inline.is_empty() && inline != "[]" {
            for value in parse_frontmatter_values(inline) {
                push_people_scalar(frontmatter, &value, &value);
            }
        }

        index += 1;
        let mut entry: Vec<&str> = Vec::new();
        while index < lines.len() && lines[index].starts_with(char::is_whitespace) {
            let trimmed = lines[index].trim();
            if let Some(item) = trimmed.strip_prefix('-') {
                flush_people_entry(frontmatter, &entry);
                entry.clear();
                entry.push(item.trim());
            } else if !trimmed.is_empty() {
                entry.push(trimmed);
            }
            index += 1;
        }
        flush_people_entry(frontmatter, &entry);
    }
}

fn flush_people_entry(frontmatter: &mut NoteFrontmatter, lines: &[&str]) {
    if lines.is_empty() {
        return;
    }
    let raw = lines.join("\n");
    let first = lines[0].trim();
    let quoted = first.starts_with(['\'', '"']);
    let mapping_style = !quoted && first.contains(':');
    if !mapping_style {
        if lines.len() == 1 {
            push_people_scalar(frontmatter, first, &raw);
        } else {
            preserve_unparsed_person(frontmatter, &raw, "unexpected continuation lines");
        }
        return;
    }

    let mut name = None;
    let mut email = None;
    for line in lines {
        let Some((key, value)) = line.split_once(':') else {
            preserve_unparsed_person(frontmatter, &raw, "mapping field has no colon");
            return;
        };
        let value = clean_frontmatter_scalar(value);
        match key.trim() {
            "name" | "display_name" | "displayName" if name.is_none() => name = Some(value),
            "email" if email.is_none() => email = Some(value),
            known if matches!(known, "name" | "display_name" | "displayName" | "email") => {
                preserve_unparsed_person(frontmatter, &raw, "duplicate mapping field");
                return;
            }
            _ => {
                preserve_unparsed_person(frontmatter, &raw, "unsupported mapping field");
                return;
            }
        }
    }

    let supplied_email = email.filter(|value| !value.is_empty());
    let scalar = name
        .filter(|value| !value.is_empty())
        .or_else(|| supplied_email.clone());
    let Some(scalar) = scalar else {
        preserve_unparsed_person(frontmatter, &raw, "mapping has no name or email value");
        return;
    };
    match normalize_person_scalar(&scalar, supplied_email.as_deref()) {
        Some(person) => frontmatter.people.push(person),
        None => preserve_unparsed_person(frontmatter, &raw, "invalid name/email evidence"),
    }
}

fn push_people_scalar(frontmatter: &mut NoteFrontmatter, scalar: &str, raw: &str) {
    match normalize_person_scalar(scalar, None) {
        Some(person) => frontmatter.people.push(person),
        None => preserve_unparsed_person(frontmatter, raw, "unrecognized scalar identity"),
    }
}

fn normalize_person_scalar(raw: &str, supplied_email: Option<&str>) -> Option<String> {
    let cleaned = clean_frontmatter_scalar(raw);
    if cleaned.is_empty() || cleaned == "[]" {
        return None;
    }

    let (identity, embedded_email) = split_angle_email(&cleaned);
    let email = supplied_email
        .map(str::trim)
        .filter(|value| is_email(value))
        .or(embedded_email);
    if supplied_email.is_some() && email.is_none() {
        return None;
    }

    let identity = identity.trim();
    let name = if let Some(inner) = identity
        .strip_prefix("[[")
        .and_then(|value| value.strip_suffix("]]"))
    {
        let target = inner.split(['|', '#']).next().unwrap_or_default().trim();
        target.rsplit('/').next().unwrap_or(target).trim()
    } else {
        if identity.contains("[[") || identity.contains("]]") {
            return None;
        }
        identity
    };
    if name.is_empty() && email.is_none() {
        return None;
    }
    if email.is_none() && !name.chars().any(char::is_alphabetic) {
        return None;
    }
    let name = if name.is_empty() { email? } else { name };
    Some(match email {
        Some(email) if name != email => format!("{name} <{email}>"),
        _ => name.to_string(),
    })
}

fn split_angle_email(value: &str) -> (&str, Option<&str>) {
    let Some(open) = value.rfind('<') else {
        return (value, None);
    };
    let Some(email) = value[open + 1..].strip_suffix('>') else {
        return (value, None);
    };
    if is_email(email.trim()) {
        (&value[..open], Some(email.trim()))
    } else {
        (value, None)
    }
}

fn is_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !value.chars().any(char::is_whitespace)
}

fn preserve_unparsed_person(frontmatter: &mut NoteFrontmatter, raw: &str, reason: &str) {
    frontmatter.people_unparsed.push(raw.to_string());
    frontmatter.people_parse_warnings.push(format!(
        "people entry preserved but not projected ({reason}): {raw}"
    ));
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
    fn parses_people_frontmatter_shapes_without_mangling_identity() {
        let cases = [
            ("people: Kevin", "Kevin"),
            ("people:\n  - 'Marcus Webb'", "Marcus Webb"),
            ("people:\n  - [[Ada Lovelace]]", "Ada Lovelace"),
            (
                "attendees:\n  - '[[Alice Morgan]] <alice@example.com>'",
                "Alice Morgan <alice@example.com>",
            ),
            ("people:\n  - name: Kevin", "Kevin"),
            (
                "attendees:\n  - name: Ada Lovelace\n    email: ada@example.com",
                "Ada Lovelace <ada@example.com>",
            ),
        ];

        for (frontmatter, expected) in cases {
            let parsed = parse_note_frontmatter(&format!("---\n{frontmatter}\n---\n# Body\n"));
            assert_eq!(parsed.people, vec![expected], "{frontmatter}");
            assert!(parsed.people_unparsed.is_empty(), "{frontmatter}");
            assert!(parsed.people_parse_warnings.is_empty(), "{frontmatter}");
        }
    }

    #[test]
    fn preserves_and_flags_unparseable_people_evidence() {
        let parsed = parse_note_frontmatter(
            "---\npeople:\n  - role: decision-maker\n  - name: Ada Lovelace\n    email: not-an-email\n---\n",
        );

        assert!(parsed.people.is_empty());
        assert_eq!(
            parsed.people_unparsed,
            vec![
                "role: decision-maker",
                "name: Ada Lovelace\nemail: not-an-email"
            ]
        );
        assert_eq!(parsed.people_parse_warnings.len(), 2);
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
