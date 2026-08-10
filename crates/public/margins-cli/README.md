# margins-cli

`margins-cli` owns the public `margins` command parser, dispatch, and output
contracts. It composes the portable `margins-core`, `margins-media`,
`margins-store`, and `margins-workflows` crates without depending on the
private desktop or native-capture implementation.

The default binary intentionally has no device-capture or model backend. Its
capture, ASR, and diarization failures are typed and stable; callers may inject
implementations through `CliServices` when embedding the library.

Install it from the public repository root:

```console
cargo install --locked --path crates/public/margins-cli
margins --help
```

Or run it without installing:

```console
cargo run --manifest-path crates/public/margins-cli/Cargo.toml -- recent
```
