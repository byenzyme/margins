# Public workspace architecture

This document describes only the materialized public Rust workspace: the local
recording-to-note pipeline you can read, run, and fork. It is not a modular
platform and does not describe any remote, mobile, or relay meeting
infrastructure — those are intentionally outside the public boundary.

## Dependency layers

```text
margins-core
├── margins-media
├── margins-store
└── margins-workflows
        └── margins-cli

margins-media ─────┐
margins-store ─────┴──> margins-workflows ──> margins-cli
```

The arrows point from a prerequisite to a consumer. `margins-core` defines the
versioned values, IDs, and in-process ports; the upper layers compose them into
transcription, storage, note workflows, and commands. Crates use
path-plus-version dependencies so the workspace is convenient to develop without
assuming registry publication.

## The transcription/diarization seam

`margins-media` is the one place where audio becomes text. It accepts
caller-supplied media and turns it into a transcript with speakers, behind the
`AsrBackend` and `DiarizationBackend` ports defined in `margins-core`. Optional
ASR and diarization provider adapters are selected by Cargo feature; the default
build carries none of them.

This seam is kept legible on purpose. Rather than hide transcription inside a
binary, the boundary — "audio in, transcript and speakers out" — is visible in
source, so a reader can follow exactly how it happens on today's on-device path.
That is transparency about the current implementation, not a promise of a stable
plugin ABI or of a hosted transcription option.

## Contract boundaries

`margins-core` is an in-process domain contract. It does not choose a database
deployment, identity provider, device implementation, or hosted topology.
Keeping those choices out of the low layer lets a local tool compose sessions,
media, and notes without pulling in platform code.

`margins-store` implements the portable SQLite session repository used by the
higher-level workflows. `margins-media` adapters accept data supplied by their
caller; the optional ASR and diarization features do not decide how media was
captured or authorize its use. `margins-workflows` operates through explicit
repositories, backend ports, filesystem roots, and event sinks.

`margins-cli` exposes the portable composition as commands. An operation that
requires an unavailable application-supplied capability (such as native capture)
returns a stable, explicit error rather than importing an excluded
implementation.

## What you can read and change

- read exactly how audio becomes a transcript in `margins-media`;
- provide ASR, diarization, clock, event, and process services through the
  public ports;
- replace or extend note templates and readable agent skills;
- embed the CLI library with explicit services, paths, and output writers.

These are source-level Rust contracts and readable text files, not a promise of
a stable dynamic-plugin ABI or of an ecosystem of interchangeable backends.

## Trust boundary

The workspace validates portable values and confines selected local artifact
operations, but an integrator remains responsible for consent, authentication,
authorization, encryption, retention, deletion, observability, backups, model
providers, and incident response. See [README.md](../README.md) for the concise
privacy note and [OPEN_SOURCE.md](../OPEN_SOURCE.md) for the exact export and
review boundary.
