use chrono::{DateTime, Duration as ChronoDuration, Local, Utc};
use margins_workflows::integrations::{EvidenceFreshness, FreshnessStatus, SurveyRange};
use rusqlite::{params, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::settings::Settings;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CalendarEventSuggestion {
    pub(crate) title: String,
    pub(crate) start: Option<String>,
    pub(crate) end: Option<String>,
    pub(crate) calendar_id: Option<String>,
    pub(crate) event_id: Option<String>,
    pub(crate) people: Vec<String>,
    pub(crate) filename: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct CalendarSuggestionResult {
    pub(crate) schema_version: &'static str,
    pub(crate) suggestion: Option<CalendarEventSuggestion>,
    pub(crate) freshness: EvidenceFreshness,
}

/// Percent-encode OAuth/query parameters for the remaining concrete drivers.
/// Calendar no longer owns an OAuth flow; Granola still uses this utility.
pub(crate) fn url_encode(input: &str) -> String {
    input
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            b' ' => "%20".to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

pub(crate) fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (candidate, value) = pair.split_once('=')?;
        (candidate == key).then(|| url_decode(value))
    })
}

fn url_decode(input: &str) -> String {
    let mut out = String::new();
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&input[index + 1..index + 3], 16) {
                out.push(byte as char);
                index += 3;
                continue;
            }
        }
        out.push(if bytes[index] == b'+' {
            ' '
        } else {
            bytes[index] as char
        });
        index += 1;
    }
    out
}

/// Resolve the nearest active/upcoming event exclusively from the active
/// Workspace's authoritative Calendar materialization. This path is read-only:
/// it never probes credentials, calls Google, refreshes, or reads raw cache.
pub(crate) fn calendar_event_suggestion(
    workspace: &margins_workflows::workspace::ResolvedWorkspace,
    settings: &Settings,
    now: DateTime<Local>,
) -> Result<CalendarSuggestionResult, String> {
    let has_calendar_binding = workspace.config.bindings.values().any(|binding| {
        matches!(
            binding,
            margins_workflows::workspace::WorkspaceBinding::GoogleCalendar { .. }
        )
    });
    if !workspace.ledger_path().is_file() {
        return Ok(CalendarSuggestionResult {
            schema_version: "margins.calendar-suggestion.v1",
            suggestion: None,
            freshness: if has_calendar_binding {
                stale_freshness("never_refreshed", None)
            } else {
                not_applicable_freshness()
            },
        });
    }
    let connection = rusqlite::Connection::open_with_flags(
        workspace.ledger_path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("Calendar ledger could not be opened read-only: {error}"))?;
    let freshness = calendar_materialization_freshness(&connection, workspace, now)?;
    let window_from = (now - ChronoDuration::minutes(45)).with_timezone(&Utc);
    let window_to = (now + ChronoDuration::hours(4)).with_timezone(&Utc);
    let mut suggestions = Vec::new();

    for binding in workspace.config.bindings.values() {
        let margins_workflows::workspace::WorkspaceBinding::GoogleCalendar { account, calendar } =
            binding
        else {
            continue;
        };
        let scope = margins_workflows::integrations::GoogleCalendarScope::for_selector(
            calendar,
            now.with_timezone(&Utc),
        )
        .map_err(|error| error.to_string())?;
        let occurred_from = window_from.max(scope.occurred_from);
        let occurred_to = window_to.min(scope.occurred_to);
        let mut statement = connection
            .prepare(
                r#"
                SELECT source_id, calendar_id, occurred_from, occurred_to, title
                FROM calendar_event_evidence
                WHERE connector_id = 'gcal' AND source_account = ?1
                  AND tombstoned_at IS NULL
                  AND occurred_from >= ?2 AND occurred_from <= ?3
                ORDER BY occurred_from, source_id
                "#,
            )
            .map_err(|error| format!("Calendar evidence query could not be prepared: {error}"))?;
        let occurred_from = occurred_from.to_rfc3339();
        let occurred_to = occurred_to.to_rfc3339();
        let rows = statement
            .query_map(params![account, occurred_from, occurred_to], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(|error| format!("Calendar evidence query failed: {error}"))?;
        for row in rows {
            let (source_id, calendar_id, start, end, title) =
                row.map_err(|error| format!("Calendar evidence row is invalid: {error}"))?;
            let start_at = DateTime::parse_from_rfc3339(&start)
                .map_err(|error| format!("Calendar evidence start is invalid: {error}"))?
                .with_timezone(&Local);
            let end_at = end
                .as_deref()
                .map(DateTime::parse_from_rfc3339)
                .transpose()
                .map_err(|error| format!("Calendar evidence end is invalid: {error}"))?
                .map(|value| value.with_timezone(&Local));
            if end_at.is_some_and(|end| end < now - ChronoDuration::minutes(5)) {
                continue;
            }
            let self_declined = connection
                .query_row(
                    r#"
                    SELECT 1 FROM calendar_event_attendees
                    WHERE connector_id = 'gcal' AND source_account = ?1 AND source_id = ?2
                      AND is_self = 1 AND response_status = 'declined'
                    LIMIT 1
                    "#,
                    [account.as_str(), source_id.as_str()],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|error| format!("Calendar attendee state could not be read: {error}"))?
                .is_some();
            if self_declined {
                continue;
            }
            let mut attendee_statement = connection
                .prepare(
                    r#"
                    SELECT display_name FROM calendar_event_attendees
                    WHERE connector_id = 'gcal' AND source_account = ?1 AND source_id = ?2
                      AND is_self = 0
                    ORDER BY position, attendee_key
                    "#,
                )
                .map_err(|error| {
                    format!("Calendar attendee query could not be prepared: {error}")
                })?;
            let people = attendee_statement
                .query_map([account.as_str(), source_id.as_str()], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|error| format!("Calendar attendee query failed: {error}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("Calendar attendee row is invalid: {error}"))?;
            suggestions.push(CalendarEventSuggestion {
                filename: render_filename_template(
                    &settings.note_filename_template,
                    start_at,
                    &title,
                ),
                title,
                start: Some(start),
                end,
                event_id: source_id
                    .strip_prefix(&format!("{calendar_id}:"))
                    .map(str::to_string)
                    .or(Some(source_id)),
                calendar_id: Some(calendar_id),
                people: normalize_people(people),
            });
        }
    }
    suggestions.sort_by_key(|event| calendar_event_rank(event, now));
    Ok(CalendarSuggestionResult {
        schema_version: "margins.calendar-suggestion.v1",
        suggestion: suggestions.into_iter().next(),
        freshness,
    })
}

fn calendar_materialization_freshness(
    connection: &rusqlite::Connection,
    workspace: &margins_workflows::workspace::ResolvedWorkspace,
    now: DateTime<Local>,
) -> Result<EvidenceFreshness, String> {
    let mut aggregate = not_applicable_freshness();
    let mut found_binding = false;
    for binding in workspace.config.bindings.values() {
        let margins_workflows::workspace::WorkspaceBinding::GoogleCalendar { account, calendar } =
            binding
        else {
            continue;
        };
        found_binding = true;
        let row = connection
            .query_row(
                "SELECT health_status, last_sync_at, materialization_fingerprint, scope_boundary_json
                 FROM connectors WHERE connector_id = 'gcal' AND account = ?1",
                [account],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("Calendar freshness could not be read: {error}"))?;
        let Some((health, last_sync, fingerprint, stored_scope)) = row else {
            aggregate = merge_freshness(aggregate, stale_freshness("never_refreshed", None));
            continue;
        };
        let last_sync = last_sync
            .as_deref()
            .map(DateTime::parse_from_rfc3339)
            .transpose()
            .map_err(|error| format!("Calendar last refresh timestamp is invalid: {error}"))?
            .map(|value| value.with_timezone(&Utc));
        let expected_fingerprint = calendar
            .materialization_fingerprint()
            .map_err(|error| error.to_string())?;
        let expected_scope = margins_workflows::integrations::GoogleCalendarScope::for_selector(
            calendar,
            now.with_timezone(&Utc),
        )
        .map_err(|error| error.to_string())?
        .as_range();
        let stored_scope = stored_scope
            .as_deref()
            .map(serde_json::from_str::<SurveyRange>)
            .transpose()
            .map_err(|error| format!("Calendar refresh boundary is invalid: {error}"))?;
        let freshness = match health.as_str() {
            "error" => EvidenceFreshness {
                status: FreshnessStatus::Error,
                stale: true,
                reason: Some("refresh_failed".to_string()),
                last_successful_refresh: last_sync,
            },
            "needs-auth" => EvidenceFreshness {
                status: FreshnessStatus::NeedsAuth,
                stale: true,
                reason: Some("needs_auth".to_string()),
                last_successful_refresh: last_sync,
            },
            "stale" => stale_freshness("source_stale", last_sync),
            "fresh"
                if fingerprint.as_deref() == Some(expected_fingerprint.as_str())
                    && stored_scope.as_ref() == Some(&expected_scope) =>
            {
                EvidenceFreshness {
                    status: FreshnessStatus::Fresh,
                    stale: false,
                    reason: None,
                    last_successful_refresh: last_sync,
                }
            }
            "fresh" => stale_freshness("refresh_required", last_sync),
            _ => stale_freshness("source_stale", last_sync),
        };
        aggregate = merge_freshness(aggregate, freshness);
    }
    Ok(if found_binding {
        aggregate
    } else {
        not_applicable_freshness()
    })
}

fn stale_freshness(
    reason: &str,
    last_successful_refresh: Option<DateTime<Utc>>,
) -> EvidenceFreshness {
    EvidenceFreshness {
        status: FreshnessStatus::Stale,
        stale: true,
        reason: Some(reason.to_string()),
        last_successful_refresh,
    }
}

fn not_applicable_freshness() -> EvidenceFreshness {
    EvidenceFreshness {
        status: FreshnessStatus::NotApplicable,
        stale: false,
        reason: None,
        last_successful_refresh: None,
    }
}

fn merge_freshness(left: EvidenceFreshness, right: EvidenceFreshness) -> EvidenceFreshness {
    fn severity(status: FreshnessStatus) -> u8 {
        match status {
            FreshnessStatus::Error => 4,
            FreshnessStatus::NeedsAuth => 3,
            FreshnessStatus::Stale => 2,
            FreshnessStatus::Fresh => 1,
            FreshnessStatus::NotApplicable => 0,
        }
    }
    if severity(right.status) > severity(left.status) {
        right
    } else if severity(right.status) < severity(left.status) {
        left
    } else {
        EvidenceFreshness {
            last_successful_refresh: match (
                left.last_successful_refresh,
                right.last_successful_refresh,
            ) {
                (Some(left), Some(right)) => Some(left.min(right)),
                (left, right) => left.or(right),
            },
            ..left
        }
    }
}

fn calendar_event_rank(event: &CalendarEventSuggestion, now: DateTime<Local>) -> (u8, i64) {
    let start = event
        .start
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Local))
        .unwrap_or(now + ChronoDuration::hours(24));
    let end = event
        .end
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Local));
    if start <= now && end.is_some_and(|value| value >= now) {
        (0, (now - start).num_seconds())
    } else {
        (1, (start - now).num_seconds().max(0))
    }
}

pub(crate) fn normalize_people(people: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for person in people {
        let cleaned = person
            .trim()
            .trim_matches('[')
            .trim_matches(']')
            .trim()
            .trim_matches('|')
            .trim()
            .to_string();
        if !cleaned.is_empty() && seen.insert(cleaned.to_lowercase()) {
            out.push(cleaned);
        }
    }
    out
}

fn render_filename_template(template: &str, date: DateTime<Local>, event_title: &str) -> String {
    let mut out = template.to_string();
    while let Some(start) = out.find("{{date:") {
        let Some(relative_end) = out[start..].find("}}") else {
            break;
        };
        let end = start + relative_end + 2;
        let format = &out[start + 7..end - 2];
        out.replace_range(start..end, &date.format(format).to_string());
    }
    out = out
        .replace("{{event_title}}", &slugish(event_title))
        .replace("{{title}}", &slugish(event_title));
    safe_markdown_filename_stem(&out)
}

fn slugish(input: &str) -> String {
    safe_markdown_filename_stem(&input.to_lowercase())
}

fn safe_markdown_filename_stem(input: &str) -> String {
    let mut out = String::new();
    let mut last_separator = false;
    for character in input.trim().trim_end_matches(".md").chars() {
        if character.is_ascii_alphanumeric() {
            out.push(character);
            last_separator = false;
        } else if (character.is_whitespace() || character == '-' || character == '_')
            && !last_separator
        {
            out.push('-');
            last_separator = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use margins_workflows::integrations::{
        CalendarEventAttendee, CalendarEventDelta, CalendarEventEvidence, ConnectorCtx,
        IntegrationsStore, SurveyRange, GOOGLE_CALENDAR_CONNECTOR_ID,
    };
    use margins_workflows::workspace::{self, CalendarCollectionSelector, WorkspaceBinding};

    #[test]
    fn ledger_suggestion_reads_authoritative_events_and_attendees() {
        let temp = tempfile::tempdir().unwrap();
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace =
            workspace::create_workspace(temp.path(), "practice", None, &notes).unwrap();
        let selector = CalendarCollectionSelector {
            lookback_days: 30,
            lookahead_days: 30,
        };
        workspace::add_source(
            &mut workspace,
            "calendar",
            WorkspaceBinding::GoogleCalendar {
                account: "owner@example.com".to_string(),
                calendar: selector.clone(),
            },
        )
        .unwrap();
        let ctx = ConnectorCtx {
            vault_root: workspace.state_dir.clone(),
            connector_id: GOOGLE_CALENDAR_CONNECTOR_ID.to_string(),
            account: "owner@example.com".to_string(),
            command_path: None,
        };
        let now = Local.with_ymd_and_hms(2026, 8, 25, 10, 0, 0).unwrap();
        let scope = margins_workflows::integrations::GoogleCalendarScope::for_selector(
            &selector,
            now.with_timezone(&Utc),
        )
        .unwrap()
        .as_range();
        IntegrationsStore::open(&workspace.state_dir)
            .unwrap()
            .apply_calendar_event_delta(
                &ctx,
                CalendarEventDelta {
                    events: vec![CalendarEventEvidence {
                        source_id: "primary:event-1".to_string(),
                        calendar_id: "primary".to_string(),
                        occurred_from: Utc.with_ymd_and_hms(2026, 8, 25, 10, 5, 0).unwrap(),
                        occurred_to: Some(Utc.with_ymd_and_hms(2026, 8, 25, 10, 35, 0).unwrap()),
                        title: "Customer review".to_string(),
                        body_text: "Title: Customer review".to_string(),
                        href: Some("https://calendar.google.com/event".to_string()),
                    }],
                    attendees: vec![CalendarEventAttendee {
                        source_id: "primary:event-1".to_string(),
                        attendee_key: "email:alex@example.com".to_string(),
                        position: 0,
                        display_name: "Alex Smith".to_string(),
                        email: Some("alex@example.com".to_string()),
                        response_status: Some("accepted".to_string()),
                        is_self: false,
                        organizer: false,
                    }],
                    tombstone_source_ids: Vec::new(),
                    raw_items: Vec::new(),
                    scope: SurveyRange {
                        occurred_from: scope.occurred_from,
                        occurred_to: scope.occurred_to,
                    },
                    complete_snapshot: true,
                    materialization_fingerprint: selector.materialization_fingerprint().unwrap(),
                    next_cursor: None,
                },
                None,
            )
            .unwrap();

        let result = calendar_event_suggestion(&workspace, &Settings::default(), now).unwrap();
        assert_eq!(result.schema_version, "margins.calendar-suggestion.v1");
        assert_eq!(result.freshness.status, FreshnessStatus::Fresh);
        let suggestion = result.suggestion.unwrap();
        assert_eq!(suggestion.title, "Customer review");
        assert_eq!(suggestion.people, vec!["Alex Smith"]);
        assert_eq!(suggestion.calendar_id.as_deref(), Some("primary"));
        assert_eq!(suggestion.event_id.as_deref(), Some("event-1"));

        IntegrationsStore::open(&workspace.state_dir)
            .unwrap()
            .update_health(
                &ctx,
                margins_workflows::integrations::HealthStatus::Error,
                Some("quota"),
            )
            .unwrap();
        let stale = calendar_event_suggestion(&workspace, &Settings::default(), now).unwrap();
        assert!(
            stale.suggestion.is_some(),
            "last coherent snapshot remains usable"
        );
        assert_eq!(stale.freshness.status, FreshnessStatus::Error);
        assert!(stale.freshness.stale);
        assert_eq!(stale.freshness.reason.as_deref(), Some("refresh_failed"));
    }
}
