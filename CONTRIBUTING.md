# Contributing to Margins

This repository is the public source tree. It contains the CLI/TUI, meeting
runtime, media, workflows, BB plugin, and project server. Keep credentials,
recordings, transcripts, personal databases and vault contents, model files,
and signing material out of source. The Enzyme recall engine is a separate
private git dependency used only for official recall compositions.

## Prerequisites

- stable Rust and Cargo
- Python 3.11 or newer for the public tree scanner
- Node.js 22 for BB plugin changes
- platform toolchains required by any optional media feature you deliberately
  enable

The default workspace validation does not require native capture or model
files.

## Build and test

Run the public gate and build the public CLI from the repository root:

```bash
scripts/local-gate public
scripts/cargo-lane shared -- cargo build -p margins-cli --bin margins-public --no-default-features --locked
```

The public gate builds and tests all root workspace targets without access to
enzyme-rust. For a narrow change, run `scripts/local-gate quick <changed-path>`.
All Cargo commands in this repository use `scripts/cargo-lane`; the local gate
enters its disposable lane automatically. For documentation checks, use:

```bash
RUSTDOCFLAGS='-D warnings' scripts/cargo-lane disposable -- cargo doc --workspace --no-default-features --no-deps
```

After fetching public dependencies once, you can verify the locked public graph
offline with `scripts/cargo-lane shared -- cargo metadata --no-deps --locked
--offline` and `scripts/cargo-lane shared -- cargo tree --workspace
--no-default-features --locked --offline`. The root `Cargo.lock` has no private
git source. Official builds use `scripts/with-private-recall` and the pinned
`Cargo.private-recall.lock`; this requires read access to the private
`byenzyme/enzyme-rust` repository. See the
[official CLI release](docs/official-cli-release.md).

Format Rust changes with `scripts/cargo-lane shared -- cargo fmt --all`.
Test the smallest affected crate while iterating, then run the affected crate
tests and a shipped binary check before requesting review. Optional model
features have platform and runtime requirements described in the relevant crate
README.

Google integration in a source build accepts a Desktop OAuth client JSON file
through `MARGINS_GOOGLE_OAUTH_CLIENT_FILE`, or the JSON itself through
`MARGINS_GOOGLE_OAUTH_CLIENT_JSON`. Do not commit credentials or user data.

The BB plugin commits its `dist/` output. Run its typecheck, tests, and build
when changing it.

## Public tree and history checks

The [public history scanner](scripts/scan-public-history.py) reports every
secret-shaped, email, and personal-path hit without printing matched values:

```bash
scripts/local-gate scan
scripts/local-gate scan 54a7a9d7c..HEAD
```

The retired export workflow and its original contributor notes remain in
[the archive](docs/contributing-export-archive.md) for historical context.

## Dependency and lockfile changes

The root `Cargo.lock` is committed for deterministic workspace builds and CI.
Do not hand-edit it. Make targeted updates through the Cargo lane, for example:

```bash
scripts/cargo-lane shared -- cargo update -p serde
```

Review transitive changes, run the public gate, and commit the lockfile with
the manifest change. Public library packages do not force this lockfile on
downstream users.

Path-plus-version first-party dependencies support workspace development and
future registry packaging. They do not mean a package name is reserved or a
crate has been published.

## Crate publication order

No command in this repository publishes crates. If maintainers later decide
to publish, check the current workspace dependency graph before establishing
a publication order. The original export described this partial order:

1. `margins-core`
2. `margins-media`, then `margins-store`
3. `margins-workflows`
4. `margins-cli`

The public workspace now also contains `margins-meeting-protocol`,
`margins-meeting-runtime`, `margins-capture`, and the root CLI. The standalone
public server will join it after extraction. Their dependencies must be
included in any future publication plan.

The package names are not asserted to be reserved or available on crates.io.
Before any release, verify registry ownership and availability; if a name
must change, update package names, dependency keys, documentation, and
lockfiles as one reviewed change. Also perform provenance, third-party
notice, and release reviews rather than treating a successful `cargo package`
as authorization.

## Tests and fixtures

Use synthetic, minimal fixtures. Never contribute real meeting audio,
transcripts, vault contents, identifiers, credentials, tokens, or database
snapshots. Tests should make trust-boundary behavior explicit and should not
depend on a local desktop application, native recorder, hosted service, or
private server unless that integration is the subject of the test.

The included license metadata and files are not proof of licensing authority.
Contributors and maintainers must not represent a change as published,
endorsed, trademark-authorized, or legally cleared merely because the automated
checks pass.
