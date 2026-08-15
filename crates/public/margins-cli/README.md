# margins-cli

`margins-cli` owns the public Margins command parser, dispatch, and output
contracts. It composes the portable `margins-core`, `margins-media`,
`margins-store`, and `margins-workflows` crates without depending on the
private desktop or native-capture implementation.

The default binary intentionally has no device-capture or model backend. Its
capture, ASR, and diarization failures are typed and stable; callers may inject
implementations through `CliServices` when embedding the library.

This crate is not the product fresh-install route and its binary is named
`margins-public` to avoid colliding with the official `margins` command.
Workspace setup and recall are intentionally unavailable in this composition:
they require the official recall-capable binary, which creates
`.margins/recall/index.db` and uses `~/.margins/config.toml` plus
`~/.margins/models/`. Use the repository root `./install.sh` or official
release artifacts when you need the user-facing `margins` command.

For parser/contract development, run it from the public repository root:

```console
cargo run --manifest-path crates/public/margins-cli/Cargo.toml -- capabilities
```

Or run it without installing:

```console
cargo run --manifest-path crates/public/margins-cli/Cargo.toml -- recent
```
