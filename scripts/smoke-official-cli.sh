#!/usr/bin/env bash
set -euo pipefail

binary=${1:?usage: smoke-official-cli.sh /path/to/margins}
test -x "$binary"
expect_capture=${MARGINS_SMOKE_EXPECT_CAPTURE:-1}

output=$($binary __release-smoke 2>&1) || {
  printf '%s\n' "$output" >&2
  echo "official CLI composition smoke command failed" >&2
  exit 1
}

# This deliberately checks the installed binary, not source features. The
# command is side-effect-free: it must inspect composition without opening an
# audio device, requesting permission, creating a session, or recording.
python3 - "$output" "$expect_capture" <<'PY'
import json
import sys

raw = sys.argv[1]
expect_capture = sys.argv[2] != "0"
if expect_capture:
    for forbidden in ("UnavailableCaptureProvider", "capture_unavailable"):
        if forbidden.lower() in raw.lower():
            raise SystemExit(f"forbidden unavailable capture adapter in smoke output: {forbidden}")

try:
    report = json.loads(raw)
except json.JSONDecodeError as error:
    raise SystemExit(f"release smoke output is not JSON: {error}: {raw!r}")

if report.get("schema") != 1:
    raise SystemExit(f"unsupported release smoke schema: {report.get('schema')!r}")
if expect_capture and report.get("capture_available") is not True:
    raise SystemExit("packaged binary does not contain a usable capture composition")
if report.get("tui_available") is not True:
    raise SystemExit("packaged binary does not contain the recording TUI composition")
if report.get("official") is not True:
    raise SystemExit("packaged binary is not the official Margins composition")
if report.get("oauth_client") != "valid":
    raise SystemExit(
        "packaged binary does not contain a valid MARGINS_GOOGLE_OAUTH_CLIENT_JSON credential"
    )
recall = report.get("recall") or {}
if recall.get("scan") is not True or recall.get("indexing") is not True or recall.get("lookup") is not True:
    raise SystemExit("packaged binary does not contain recall scan, indexing, and lookup")
provider = report.get("capture_provider")
if expect_capture and (not isinstance(provider, str) or not provider.strip()):
    raise SystemExit("packaged binary did not identify its capture provider")
PY
