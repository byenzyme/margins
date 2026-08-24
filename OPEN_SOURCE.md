# Margins open-source boundary

This repository is currently a mixed private/public development tree. The
files selected by `open-source-boundary.json` are a mechanically auditable
**candidate public surface**. The materialized result is a standalone,
buildable Rust repository; that is not a claim that the mixed development
repository is open source, that any crate has been published, or that the
candidate has completed legal and release review.

The boundary is deliberately fail-closed. A file is eligible only when one
named scope explicitly allows its literal path, and it must not match a denied
path, contain a forbidden private-runtime import, or contain a recognized
credential. `exact_allowlist` requires every scope's include list to equal its
required-file list and rejects globs, so new files do not become public merely
because they are added to this repository.

## Candidate public surface

The public surface is deliberately small: the local note pipeline you can read,
run, and fork, plus the one place where audio becomes text.

- **Rust contracts:** `margins-core` contains platform-neutral IDs, values, and
  in-process ports for capture, sessions, events, ASR, and diarization. It
  contains no native capture, model, server, or desktop implementation.
- **Store crate:** `margins-store` contains the SQLite `SessionRepository`, the
  behavior-preserving legacy database facade, and storage-only session index
  queries. It contains no note-file workflow, native capture, or desktop code.
- **Media crate:** `margins-media` is the transcription/diarization seam —
  portable audio, transcript, and resampling values, and optional ASR/diarization
  provider adapters selected by feature. This is where audio becomes text; a
  reader can see exactly how, rather than the step being hidden inside a binary.
- **Workflow crate:** `margins-workflows` owns portable project/setup resources,
  Granola import, vault publishing, alignment, recent/transcript views, confined
  artifact pruning, and process/transcribe orchestration through the public ASR
  and diarization ports. Root and desktop consumers use compatibility facades.
- **CLI:** the independently buildable `margins-cli` library and public
  development binary, `margins-public`. Released recall-capable product
  artifacts still install the user-facing `margins` command. The native
  application launcher and server runtime are not included.
- **Skills:** agent instructions, templates, tests, and plugin metadata. These
  are intended to remain readable and customizable.
- **Docs and boundary tooling:** this document, the license, manifest,
  exporter/verifier, tests, and CI workflow.
- **Repository shell:** a public-only virtual Cargo workspace, committed lock
  file, project README, and contributor guide. These files live under
  `public-repository/` in the mixed tree and are mapped to the root of the
  materialized export; the private root Cargo package is never selected.

The mixed tree's transitional root Rust package is intentionally not in the
candidate export. The standalone public development binary is `margins-public`,
owned by the public `margins-cli` crate; the mixed root remains a
compatibility/composition layer and is not needed to build the public
repository.

## Explicitly outside the boundary

The denylist covers the Tauri desktop application, native mic/system-audio
capture, CoreML and other native ASR/diarization implementations, private
server/runtime launchers, any remote-meeting transport/relay and its durable
runtime, internal agent workflows, credentials, signing material,
generated/downloaded model files, databases, recordings, transcripts, and other
user artifacts. Excluding a path is not a security claim about it; it means no
public artifact should contain it.

The public surface is not a modular platform and makes no such promise. It is
the local recording-to-note pipeline plus one honest extension point — the
`margins-media` transcription/diarization seam — kept legible instead of hidden.
Any remote/mobile/relay meeting infrastructure is intentionally outside the
boundary and is not part of the public design.

No export command reads ignored, untracked, or unstaged worktree files. At the
start of an invocation, the tool writes the Git index to an immutable tree and
uses those exact blobs and modes for selection, scanning, hashing, and copying.
Git replacement refs and environment overrides that redirect the repository,
index, or object database are ignored.
This matters because local `.env` files, model caches, recordings, vault data,
generated artifacts, and unstaged edits may exist beside indexed source.
Indexed candidate files are still scanned for common private-key and
service-token forms before an export can be written.

## Trust and customization scope

The candidate surface lets a reviewer inspect and change how persisted
sessions are represented, how audio becomes a transcript, how note material is
parsed and published, and how the Margins skill turns meeting context into
notes. Templates and skill instructions are ordinary text and can be forked
without modifying the native application.

The transcription/diarization step is shown, not hidden. `margins-media` makes
the "audio in, transcript and speakers out" boundary visible so a reader can
follow exactly how it happens. That is transparency about today's on-device
path, not a promise of a stable plugin ecosystem or of a hosted transcription
option; Margins does not currently offer hosted transcription.

This boundary does **not** make the desktop capture stack independently
auditable. A public-only checkout cannot verify claims about native audio
capture, on-device model execution, application signing, auto-update behavior,
or the private local server. Those components require separate distribution,
trust, and privacy review. Public code should communicate with a closed
component only through documented data boundaries; importing a denied runtime
module fails the audit.

The exported CLI/core/media/store/workflows/skills surface is meant to be read
and forked. Replacing capture backends, desktop UI behavior, signing, model
packaging, or private network-server policy is outside that surface.

## Deterministic audit and export

Run the CI check locally:

```bash
python3 scripts/open_source_boundary.py --check
python3 -m unittest discover -s tests -p 'test_open_source_boundary.py' -v
```

The default command is a dry run. It prints a sorted file plan and a digest but
writes nothing:

```bash
python3 scripts/open_source_boundary.py
python3 scripts/open_source_boundary.py --output /tmp/margins-public
```

Materialization requires an explicit confirmation flag and a destination that
does not already exist:

```bash
python3 scripts/open_source_boundary.py \
  --output /tmp/margins-public \
  --execute
python3 scripts/open_source_boundary.py --verify-tree /tmp/margins-public
python3 scripts/open_source_boundary.py --test-export /tmp/margins-public
```

Files are copied in lexical order with normalized file and directory
permissions and timestamps. The five explicit repository-shell mappings are
part of the manifest and the artifact digest; mappings must name exact required
source files and unique, normalized destinations. The tool refuses source and
verification symlinks, including a dangling output symlink, existing output
paths, mapping collisions, ambiguous scope ownership, denylist overlap, missing
required files, forbidden imports, credential signatures, and extra files in a
verified tree. It never publishes, uploads, commits, or replaces an export.

The current export has a top-level public-only Cargo workspace and committed
lockfile. CI builds and tests all five packages from that export root with the
locked graph in offline mode, then retains the per-crate graph, documentation,
package-inventory, and optional-feature checks. This ensures the repository
entrypoint works rather than proving only that separately copied crates happen
to compile.

The lockfile is the reproducibility baseline for development and CI in the
standalone workspace. Published libraries, if publication is separately
authorized, do not impose it on downstream consumers. First-party path and
version dependencies require registry publication in this order: `margins-core`;
then `margins-media` and `margins-store`; then `margins-workflows`; finally
`margins-cli`. Those package names are not claimed to be reserved or available.
Verify registry ownership and name availability, and consistently rename
manifests, dependency keys, docs, and lockfiles if needed, before any attempted
publication.

Before publishing a future public repository, review the dry-run inventory,
materialize into a new temporary directory, run `--verify-tree` there, inspect
the diff from the prior public release, build/test that standalone tree, and
perform a dedicated full-repository secret scan. The built-in signatures are a
small backstop, not a comprehensive secret detector. This scaffold is a
guardrail, not a substitute for human release review.

## Licensing

The candidate contains the repository's `LICENSE` file, which currently states
Apache License 2.0. Each public Cargo manifest declares `Apache-2.0`, and each
crate package includes an identical copy of that license text. This document
and the boundary tool do not themselves grant a license or verify copyright
ownership, provenance, patent rights, third-party notices, or the licensor's
authority for every selected file. Resolve any conflicting repository license
signals and complete a provenance/license review before publication.
Dependencies and third-party components retain their own licenses. Generated
model weights, application binaries, signing material, and excluded assets are
outside the candidate. Apache 2.0 does not separately grant trademark rights
in the Margins name or branding.
