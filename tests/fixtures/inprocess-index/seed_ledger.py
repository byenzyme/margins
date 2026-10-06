#!/usr/bin/env python3
"""Seed a Workspace ledger with four synced Gmail threads, as a fresh sync of
the default Gmail declaration for fixture@example.com leaves it.

usage: seed_ledger.py <ledger.db>   (the schema must already exist)
"""
import json
import sqlite3
import sys

ACCOUNT = "fixture@example.com"
FINGERPRINT = "gmail-selector-v1:" + json.dumps(
    {"query": "-in:spam -in:trash", "backfill_days": 365}, separators=(",", ":")
)

ledger = sqlite3.connect(sys.argv[1])
for index in range(4):
    thread = f"relay-thread-{index}"
    at = f"2026-09-2{index}T10:00:00Z"
    ledger.execute(
        "INSERT INTO thread_evidence (connector_id, source_account, thread_id, occurred_from,"
        " occurred_to, body_text, href) VALUES ('email', ?, ?, ?, ?, ?, ?)",
        (
            ACCOUNT, thread, at, at,
            f"Saffron relay dispatch {index}: Iris Okafor confirms the station handover, "
            "spare antenna stock, logbook audit, crew rotation, and the next relay checkpoint.",
            f"https://mail.example/{thread}",
        ),
    )
    ledger.execute(
        "INSERT INTO participant_threads (connector_id, source_account, participant, thread_id,"
        " last_interaction, sampling_score) VALUES ('email', ?, 'iris@relay.test', ?, ?, 100)",
        (ACCOUNT, thread, at),
    )
ledger.execute(
    "INSERT INTO connectors (connector_id, account, last_sync_at, materialization_fingerprint,"
    " health_status, updated_at) VALUES ('email', ?, '2026-09-30T00:00:00Z', ?, 'fresh',"
    " '2026-09-30T00:00:00Z') ON CONFLICT(connector_id, account) DO UPDATE SET"
    " last_sync_at = excluded.last_sync_at,"
    " materialization_fingerprint = excluded.materialization_fingerprint,"
    " health_status = 'fresh', updated_at = excluded.updated_at",
    (ACCOUNT, FINGERPRINT),
)
ledger.commit()
