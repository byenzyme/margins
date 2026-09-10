use chrono::{Local, TimeZone};
use margins_store::{legacy, list_session_index, SessionIndexQuery};
use tempfile::tempdir;

#[test]
fn absent_database_is_empty_without_materializing_a_directory() {
    let temporary = tempdir().unwrap();
    let absent = temporary.path().join("never-created");
    assert!(list_session_index(&absent, SessionIndexQuery::default())
        .unwrap()
        .is_empty());
    assert!(!absent.exists());
}

#[test]
fn storage_index_filters_sorts_and_honors_zero_limit() {
    let temporary = tempdir().unwrap();
    let margins_dir = temporary.path().join(".margins");
    let earlier = Local.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
    let later = Local.with_ymd_and_hms(2026, 1, 2, 10, 0, 0).unwrap();
    legacy::create_session(&margins_dir, "earlier", &earlier, "earlier.md").unwrap();
    legacy::create_session(&margins_dir, "later", &later, "later.md").unwrap();
    legacy::add_segment(&margins_dir, "later", 0, "later.wav", 0, Some(3.5)).unwrap();
    legacy::set_people(&margins_dir, "later", vec!["Ada".to_string()]).unwrap();

    let all = list_session_index(&margins_dir, SessionIndexQuery::default()).unwrap();
    assert_eq!(
        all.iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["later", "earlier"]
    );
    assert_eq!(all[0].duration_secs, 3.5);
    assert_eq!(all[0].people, vec!["Ada"]);

    let filtered = list_session_index(
        &margins_dir,
        SessionIndexQuery {
            started_after: Some("2026-01-01T12:00:00Z".to_string()),
            started_before: None,
            limit: Some(1),
        },
    )
    .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].name, "later");
    assert!(list_session_index(
        &margins_dir,
        SessionIndexQuery {
            limit: Some(0),
            ..SessionIndexQuery::default()
        },
    )
    .unwrap()
    .is_empty());
}

#[test]
fn storage_index_orders_rfc3339_offsets_by_instant() {
    let temporary = tempdir().unwrap();
    let margins_dir = temporary.path().join(".margins");
    let first = chrono::DateTime::parse_from_rfc3339("2026-01-01T23:30:00-08:00")
        .unwrap()
        .with_timezone(&Local);
    let second = chrono::DateTime::parse_from_rfc3339("2026-01-02T01:00:00+00:00")
        .unwrap()
        .with_timezone(&Local);
    legacy::create_session(&margins_dir, "actually-later", &first, "later.md").unwrap();
    legacy::create_session(&margins_dir, "lexically-later", &second, "earlier.md").unwrap();

    let entries = list_session_index(&margins_dir, SessionIndexQuery::default()).unwrap();
    assert_eq!(entries[0].name, "actually-later");
    assert_eq!(entries[1].name, "lexically-later");
}
