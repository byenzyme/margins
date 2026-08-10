# Public workspace architecture

This document describes only the materialized public Rust workspace. The
ownership decisions and crate boundaries come from the mixed repository's
open-source boundary and modularization work, distilled here without treating
excluded implementations as part of the public design.

## Dependency layers

```text
margins-meeting-protocol
├── margins-meeting-runtime
└── margins-core
    ├── margins-media
    ├── margins-store
    └── margins-workflows
        └── margins-cli

margins-media ─────┐
margins-store ─────┴──> margins-workflows ──> margins-cli
```

The arrows point from a prerequisite to a consumer. The low layers define
versioned values and behavior; the upper layers compose them into storage,
processing, notes, and commands. Crates use path-plus-version dependencies so
the workspace is convenient to develop without assuming registry publication.

## Remote meeting flow

1. An application-owned remote client sends versioned commands and ordered,
   opaque media chunks described by `margins-meeting-protocol`.
2. An application-owned authenticated transport delivers those messages and
   reconnect cursors. Transport and identity policy are deliberately outside
   this workspace.
3. `margins-meeting-runtime` applies lifecycle, idempotency, acknowledgement,
   discontinuity, replay, and finalization rules. Its included in-memory
   adapters are deterministic references for integration and tests.
4. Application adapters translate durable meeting events into the domain ports
   in `margins-core` and provide any required persistence.
5. `margins-media`, `margins-store`, and `margins-workflows` process
   caller-supplied media, maintain portable session records, and produce
   transcripts, artifacts, and customized notes.
6. `margins-cli` exposes the portable composition as commands. Operations that
   require an unavailable application-supplied capability return a stable,
   explicit error rather than importing an excluded implementation.

## Contract boundaries

The meeting protocol is a wire contract; the core crate is an in-process domain
contract. Neither chooses a network framework, database deployment, identity
provider, device implementation, or hosted topology. Keeping those choices
outside the low layers lets a mobile client, browser client, relay, local tool,
or service share meeting semantics without sharing platform code.

Persistence has two distinct roles. The meeting runtime defines what must be
durable for ordered remote operation, while `margins-store` implements the
portable SQLite session repository used by higher-level workflows. A production
remote deployment must provide and verify its own durable runtime adapter; the
in-memory implementation is not that adapter.

Media adapters accept data supplied by their caller. Optional ASR and
diarization features do not decide how media was captured or authorize its use.
Workflows likewise operate through explicit repositories, backend ports,
filesystem roots, and event sinks.

## Customization seams

- implement the protocol over an application-chosen authenticated transport;
- provide durable meeting state and artifact storage;
- inject ASR, diarization, clock, event, and process services through public
  ports;
- replace or extend note templates and readable agent skills;
- embed the CLI library with explicit services, paths, and output writers; or
- build a separate application around the lower-level crates.

These seams are source-level Rust contracts and versioned messages, not a
promise of a stable dynamic-plugin ABI.

## Trust boundary

The workspace validates meeting state and confines selected local artifact
operations, but an integrator remains responsible for consent, authentication,
authorization, encryption, retention, deletion, observability, backups, model
providers, and incident response. See [README.md](../README.md) for the concise
privacy checklist and [OPEN_SOURCE.md](../OPEN_SOURCE.md) for the exact export
and review boundary.
