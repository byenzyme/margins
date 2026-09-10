# margins

Local web/mobile launcher for Margins. It starts `margins-server`, waits for the
health check, and prints the URL for the browser app.

This package is currently intended to be run from the local checkout with `npx`.
It is not relying on a published npm package yet.

## Usage

From the repo root:

```bash
# Build the frontend that the server embeds.
cd desktop
npm install
npm run build
cd ..

# Build the server binary.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(pwd)/target}"
cargo build --manifest-path desktop/src-tauri/Cargo.toml \
  --bin margins-server \
  --features server

# Start the web app.
npx --yes ./packages/margins
```

The launcher finds a local `margins-server` binary in `CARGO_TARGET_DIR` when it
is set. You can also point at a binary explicitly:

```bash
MARGINS_SERVER_BIN=/path/to/margins-server npx --yes ./packages/margins
```

## Open From Another Device

By default the server only binds to localhost. To open it from a phone or
another computer on the same LAN:

```bash
MARGINS_HOST=0.0.0.0 npx --yes ./packages/margins
```

Then open:

```text
http://<your-computer-ip>:8787
```

Mobile recording requires a secure browser context. A phone opening the app over
plain LAN HTTP can load the UI, but browsers usually block microphone access
unless the page is served over HTTPS. Put the local server behind a trusted HTTPS
reverse proxy or tunnel before treating phone recording as supported.

Keep the printed auth token private if the server is reachable outside
localhost.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `MARGINS_PORT` | `8787` | Port the server listens on. Set to `0` for a random port. |
| `MARGINS_HOST` | `127.0.0.1` | Host to bind to. Set to `0.0.0.0` to expose on LAN/server. |
| `MARGINS_DATA_DIR` | `~/.margins-app` | Directory for server state, cached binaries, and auth token. |
| `MARGINS_SERVER_BIN` | (unset) | Path to a pre-built margins-server binary. If set, skips download. |
| `MARGINS_HEALTH_TIMEOUT_MS` | `30000` | How long to wait (in milliseconds) for the server to report health. |
| `CARGO_TARGET_DIR` | (unset) | Cargo target directory to search for a locally built `margins-server`. |

## Requirements

Web recording finalization is self-contained in `margins-server`: browser
WebM/Opus uploads are decoded natively and streamed as mono 16 kHz WAV files.
`ffmpeg` is optional and only used when the server is started with
`MARGINS_HOSTED_WEBM_FINALIZER=ffmpeg` as a migration compatibility fallback.

The browser UI is embedded into the Rust binary at compile time from
`desktop/dist`, so rerun `npm run build` before rebuilding `margins-server` when
frontend files change.

## Startup

On startup, the launcher:
1. Uses `MARGINS_SERVER_BIN`, or finds a local binary in `CARGO_TARGET_DIR` or common target dirs
2. Spawns the server process
3. Polls `/health` endpoint every 250ms up to the timeout
4. Prints the server URL
5. If an auth token exists at `~/.margins-app/token`, prints it

## Notes

- Downloading server binaries from GitHub Releases is wired in but not the primary local workflow yet.
- The auth token is automatically printed on startup if one exists
- Use `Ctrl+C` to gracefully shut down the server
- System-audio capture, native file dialogs, app restart/install actions, and some desktop integrations are native-app-only.
