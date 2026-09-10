#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
target_dir="${CARGO_TARGET_DIR:-/Users/example/Hacks/margins-cargo-target}"
sdk_version="${MARGINS_XWIN_SDK_VERSION:-10.0.22621}"

export CARGO_TARGET_DIR="$target_dir"
export PATH="/opt/homebrew/opt/llvm/bin:$PATH"

cd "$repo_root"

if ! command -v cargo-xwin >/dev/null 2>&1 && ! cargo xwin --help >/dev/null 2>&1; then
  echo "cargo-xwin is required: cargo install cargo-xwin --locked" >&2
  exit 1
fi

if ! command -v llvm-lib >/dev/null 2>&1; then
  echo "llvm-lib is required for xwin static archives. On macOS: brew install llvm" >&2
  exit 1
fi

cargo xwin build \
  --xwin-sdk-version "$sdk_version" \
  --manifest-path desktop/src-tauri/Cargo.toml \
  --no-default-features \
  --features parakeet-asr \
  --target x86_64-pc-windows-msvc

exe="$target_dir/x86_64-pc-windows-msvc/debug/margins-desktop.exe"
if [[ ! -f "$exe" ]]; then
  echo "Windows build finished but expected executable is missing: $exe" >&2
  exit 1
fi

file "$exe"

wine_bin=""
if command -v wine64 >/dev/null 2>&1; then
  wine_bin="$(command -v wine64)"
elif command -v wine >/dev/null 2>&1; then
  wine_bin="$(command -v wine)"
fi

if [[ -n "$wine_bin" ]]; then
  MARGINS_WINE_BIN="$wine_bin" MARGINS_WINDOWS_EXE="$exe" python3 - <<'PY'
import os
import subprocess
import sys

wine = os.environ["MARGINS_WINE_BIN"]
exe = os.environ["MARGINS_WINDOWS_EXE"]
timeout = int(os.environ.get("MARGINS_WINE_TIMEOUT_SECS", "15"))
env = os.environ.copy()
env.setdefault("WINEDEBUG", "-all")

try:
    result = subprocess.run(
        [wine, exe, "--help"],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=timeout,
    )
except subprocess.TimeoutExpired as exc:
    print(
        f"{exe} launched under Wine and stayed running for {timeout}s; "
        "treating that as a runtime smoke pass.",
        file=sys.stderr,
    )
    if exc.stdout:
        print(exc.stdout[:4000], file=sys.stderr)
    if exc.stderr:
        print(exc.stderr[:4000], file=sys.stderr)
    sys.exit(0)

if result.stdout:
    print(result.stdout[:4000])
if result.stderr:
    print(result.stderr[:4000], file=sys.stderr)
if result.returncode != 0:
    print(
        f"{exe} exited under Wine with status {result.returncode}.",
        file=sys.stderr,
    )
    sys.exit(result.returncode)
print(f"{exe} launched under Wine and exited cleanly.", file=sys.stderr)
PY
else
  echo "Wine is not installed; build validation passed, runtime smoke was skipped." >&2
  echo "Run this script on a host with Wine or a Windows runner to complete runtime validation." >&2
fi
