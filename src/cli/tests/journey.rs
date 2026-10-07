    #[cfg(feature = "recall")]
    #[test]
    fn explain_reads_the_engine_explain_view_in_margins_words() {
        // enzyme 0.12.2 (E6): the explain view beside the readings.
        let plan = serde_json::json!({
            "schema": "enzyme.spec-plan.v1",
            "vaults": [{
                "readings": [
                    {"source": "folder \"Meetings\""},
                    {"source": "folder \"People\" including linked pages"}
                ],
                "entities": [
                    {"kind": "folder", "name": "meetings", "origin": "reading", "reading": 0,
                     "status": "planned", "questions": 15},
                    {"kind": "folder", "name": "people", "origin": "reading", "reading": 1,
                     "status": "skipped", "questions": 0,
                     "skip": {"code": "thin_context",
                              "reason": "too little material: 1 note(s), 10 tokens; needs at least 1 and 32"}},
                    {"kind": "link", "name": "alice", "origin": "expanded", "reading": 1,
                     "expanded_from": "People", "status": "planned", "questions": 15},
                    {"kind": "tag", "name": "focus", "origin": "automatic", "reading": null,
                     "status": "planned", "questions": 3}
                ],
                "skipped": [
                    {"scope": "entity", "code": "thin_context", "reason": "…", "reading": 1,
                     "kind": "folder", "name": "people", "count": 1},
                    {"scope": "candidates", "code": "below_link_frequency",
                     "reason": "4 linked page(s) are linked fewer than 3 times",
                     "reading": 1, "kind": null, "name": null, "count": 4},
                    {"scope": "candidates", "code": "brand_new_code",
                     "reason": "a reason about 12 tokens", "reading": 0, "kind": null,
                     "name": null, "count": 1}
                ],
                "counts": {"selected": 4, "planned": 3, "skipped": 1, "candidates_skipped": 5}
            }]
        });
        let view = explain_view(&plan);
        assert_eq!((view.planned, view.skipped, view.candidates_skipped), (3, 1, 5));
        assert!(!view.truncated);
        let groups = view
            .readings
            .iter()
            .map(|reading| (reading.reading.as_str(), reading.learns.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            groups,
            [
                ("folder \"Meetings\"", vec!["meetings".to_string()]),
                (
                    "folder \"People\" including linked pages",
                    vec!["alice (linked page)".to_string()]
                ),
                ("picked automatically", vec!["focus".to_string()]),
            ]
        );
        let people = &view.readings[1].skipped;
        assert_eq!(people[0].what, "people");
        assert_eq!(people[0].why, "too little written about it yet");
        assert_eq!(people[1].what, "4 more pages");
        assert_eq!(people[1].why, "linked fewer than 3 times");
        // An unknown code never shows engine wording about tokens.
        assert_eq!(view.readings[0].skipped[0].why, "skipped (brand new code)");
        assert!(view
            .readings
            .iter()
            .flat_map(|reading| &reading.skipped)
            .all(|skip| !skip.why.contains("token")));

        // enzyme 0.12.1: per-reading jobs and skips (TODO(E6): drop).
        let legacy = serde_json::json!({
            "schema": "enzyme.spec-plan.v1",
            "vaults": [{
                "readings": [{
                    "source": "folder \"Meetings\"",
                    "jobs": [{"entity_name": "meetings", "entity_type": "folder"}],
                    "skipped": [{"entity_name": "standup", "entity_type": "link",
                                 "reason": {"kind": "thin_context"}}]
                }],
                "other_jobs": [], "other_skipped": [],
                "totals": {"jobs": 1, "skipped": 1}
            }]
        });
        let view = explain_view(&legacy);
        assert_eq!((view.planned, view.skipped), (1, 1));
        assert_eq!(view.readings.len(), 1, "an empty automatic group is dropped");
        assert_eq!(view.readings[0].learns, ["meetings"]);
        assert_eq!(view.readings[0].skipped[0].why, "too little written about it yet");
    }
