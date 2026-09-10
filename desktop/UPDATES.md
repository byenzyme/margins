# Margins Desktop Updates

Margins uses `tauri-plugin-updater` and checks:

```text
https://api.enzyme.garden/site/margins-updates/{{target}}/{{arch}}/{{current_version}}
https://api.enzyme.garden/site/aside-updates/{{target}}/{{arch}}/{{current_version}}
```

The public updater key is stored in `src-tauri/tauri.conf.json`. The private key
must stay outside the repo. The current signing key was generated at:

```text
~/.tauri/aside-updater.key
```

To build a signed update release:

```bash
cd desktop
export TAURI_SIGNING_PRIVATE_KEY_PATH="$HOME/.tauri/aside-updater.key"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="..."
npm run tauri:build:updates
```

The GitHub Actions release flow runs the same Apple Silicon macOS Tauri build
when a `v*` tag is pushed. Uploads must include the Tauri updater `.app.tar.gz`
artifact and its matching `.sig` file on a public GitHub Release in
`byenzyme/margins-desktop`. The server in `../enzyme-support` proxies that release
and returns `204 No Content` when the installed version is already current.

Intel macOS is not part of the current release flow. The default desktop feature
set depends on `ort-sys`, whose prebuilt ONNX Runtime path does not support
`x86_64-apple-darwin` here.
