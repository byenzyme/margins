# Contributing to Margins

The public tree is the source tree. It contains the CLI/TUI, meeting runtime,
media, workflows, BB plugin, and project server. The Enzyme recall engine is a
separate private git dependency used only for official recall compositions.

## Build and test

Install stable Rust and Cargo, Python 3.11 or newer, and Node.js 22 for BB
plugin changes. From the repository root:

```bash
scripts/local-gate public
scripts/cargo-lane shared -- cargo build -p margins-cli --bin margins-public --no-default-features --locked
```

The public gate builds and tests all root workspace targets without access to
enzyme-rust. For a narrow change, run `scripts/local-gate quick <changed-path>`.
All Cargo commands in this repository use `scripts/cargo-lane`; the local gate
enters its disposable lane automatically.

The root `Cargo.lock` intentionally has no private git source. Official builds
use `scripts/with-private-recall` and the pinned
`Cargo.private-recall.lock`; this requires read access to the private
`byenzyme/enzyme-rust` repository. See [official CLI release](docs/official-cli-release.md).

Google integration in a source build accepts a Desktop OAuth client JSON file
through `MARGINS_GOOGLE_OAUTH_CLIENT_FILE`, or the JSON itself through
`MARGINS_GOOGLE_OAUTH_CLIENT_JSON`. Do not commit credentials or user data.

## Changes

Format Rust with `cargo fmt --all`. Run the affected crate tests and a shipped
binary check before requesting review. The BB plugin commits its `dist/` output;
run its typecheck, tests, and build when changing it. Keep synthetic fixtures
small and avoid real meetings, vault contents, access tokens, or account data.

The [public history scanner](scripts/scan-public-history.py) reports every
secret-shaped, email, and personal-path hit without printing matched values:

```bash
scripts/local-gate scan
scripts/local-gate scan 54a7a9d7c..HEAD
```

The retired export workflow and its original contributor notes remain in
[the archive](docs/contributing-export-archive.md) for historical context.
