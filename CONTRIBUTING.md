# Contributing to the Margins public Rust workspace

This tree is maintained inside a mixed private/public development repository
and exported through an exact allowlist. Keep changes inside the portable
public contracts and implementations; do not copy device capture, desktop,
credentials, recordings, transcripts, databases, model files, signing
material, or private runtime code into the public tree.

## Prerequisites

- stable Rust and Cargo
- Python 3.11 or newer for the boundary audit
- platform toolchains required by any optional media feature you deliberately
  enable

The default workspace validation does not require native capture or model
files.

## Build and test

Run repository-level commands from the materialized export root:

```bash
cargo build --workspace --all-targets --no-default-features --locked
cargo test --workspace --all-targets --no-default-features --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-default-features --no-deps
```

To prove the locked dependency graph works without network access, fetch it
once and rerun offline:

```bash
cargo fetch --locked
cargo build --workspace --all-targets --no-default-features --locked --offline
cargo test --workspace --all-targets --no-default-features --locked --offline
```

Format Rust changes with `cargo fmt --all`. Test the smallest affected crate
while iterating, but run the workspace commands before proposing a change.
Optional model features have platform and runtime requirements described in
the relevant crate README; enabling one never adds a device-capture backend.

## Boundary and export checks

In the mixed development checkout, run:

```bash
python3 -m unittest discover -s tests -p 'test_open_source_boundary.py' -v
python3 scripts/open_source_boundary.py --check
```

The exporter reads an immutable snapshot of the Git index. Stage the intended
candidate files before materializing a local export, then use a new destination
directory:

```bash
python3 scripts/open_source_boundary.py \
  --output /tmp/margins-public \
  --execute
python3 scripts/open_source_boundary.py --verify-tree /tmp/margins-public
```

Build and test from `/tmp/margins-public`, not from copied crate directories.
Generate a second export and compare its reported digest and file hashes when
changing the repository shell, exporter, or allowlist.

Every new public file must be listed literally in both `include` and
`required_files` for exactly one scope in `open-source-boundary.json`. Globs,
implicit directory inclusion, private path dependencies, and files owned by
multiple scopes are rejected.

## Dependency and lockfile changes

The root `Cargo.lock` is committed for deterministic workspace builds and CI.
Do not hand-edit it. Make targeted updates, for example:

```bash
cargo update -p serde
```

Review transitive changes, run the locked offline workspace tests, and commit
the lockfile with the manifest change. Public library packages do not force
this lockfile on downstream users.

Path-plus-version first-party dependencies support workspace development and
future registry packaging. They do not mean a package name is reserved or a
crate has been published.

## Crate publication order

No command in this repository publishes crates. If maintainers later decide
to publish, crates.io dependencies require this order (crates on the same
line may follow one another after their prerequisites are available):

1. `margins-meeting-protocol`
2. `margins-core`, then `margins-meeting-runtime`
3. `margins-media`, then `margins-store`
4. `margins-workflows`
5. `margins-cli`

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
private server.

The included license metadata and files are boundary inputs, not proof of
licensing authority. Contributors and maintainers must not represent a change
as published, endorsed, trademark-authorized, or legally cleared merely
because the automated checks pass.
