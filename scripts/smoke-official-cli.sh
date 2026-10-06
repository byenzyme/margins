#!/usr/bin/env bash
set -euo pipefail

binary=${1:?usage: smoke-official-cli.sh /path/to/margins}
test -x "$binary"
expect_capture=${MARGINS_SMOKE_EXPECT_CAPTURE:-1}

# The Google OAuth client must be embedded at build time; the release action
# builds with the MARGINS_GOOGLE_OAUTH_CLIENT_JSON secret and smokes without it.
# The binary also reads a client from the environment at run time, so every
# mode clears those variables and only `embedded` (the default, and the only
# mode the release action uses) proves the embedded client:
#   embedded       require a valid embedded client.
#   runtime-dummy  supply the structurally valid dummy below at run time, for
#                  local builds without the secret (core-product-smoke). The
#                  embedded client is NOT verified; the release action is.
oauth_mode=${MARGINS_SMOKE_OAUTH_CLIENT:-embedded}
DUMMY_OAUTH_CLIENT='{"installed":{"client_id":"margins-smoke-dummy.apps.googleusercontent.com","client_secret":"margins-smoke-dummy","auth_uri":"https://accounts.google.com/o/oauth2/auth","token_uri":"https://oauth2.googleapis.com/token","redirect_uris":["http://127.0.0.1"]}}'
oauth_env=(-u MARGINS_GOOGLE_OAUTH_CLIENT_JSON -u MARGINS_GOOGLE_OAUTH_CLIENT_FILE)
case "$oauth_mode" in
  embedded) ;;
  runtime-dummy)
    oauth_env+=("MARGINS_GOOGLE_OAUTH_CLIENT_JSON=$DUMMY_OAUTH_CLIENT")
    echo "smoke-official-cli: NOTE: using a runtime dummy Google OAuth client; the embedded client is not verified" >&2
    ;;
  *)
    echo "MARGINS_SMOKE_OAUTH_CLIENT must be embedded or runtime-dummy, not '$oauth_mode'" >&2
    exit 2
    ;;
esac

# The probe is read-only; run it against a throwaway home anyway so a
# regression cannot touch the caller's ~/.margins.
smoke_home=$(mktemp -d "${TMPDIR:-/tmp}/margins-release-smoke.XXXXXX")
trap 'rm -rf "$smoke_home"' EXIT

output=$(env "${oauth_env[@]}" HOME="$smoke_home" MARGINS_HOME="$smoke_home/.margins" \
  "$binary" __release-smoke 2>&1) || {
  printf '%s\n' "$output" >&2
  echo "official CLI composition smoke command failed" >&2
  exit 1
}
if [[ -e "$smoke_home/.margins" ]]; then
  echo "release smoke wrote to its Margins home: $(find "$smoke_home" | head -5)" >&2
  exit 1
fi

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
        "packaged binary does not contain a valid MARGINS_GOOGLE_OAUTH_CLIENT_JSON credential "
        "(local builds without the secret: MARGINS_SMOKE_OAUTH_CLIENT=runtime-dummy)"
    )
recall = report.get("recall") or {}
if recall.get("indexing") is not True or recall.get("lookup") is not True:
    raise SystemExit("packaged binary does not contain recall indexing and lookup")
provider = report.get("capture_provider")
if expect_capture and (not isinstance(provider, str) or not provider.strip()):
    raise SystemExit("packaged binary did not identify its capture provider")
PY
