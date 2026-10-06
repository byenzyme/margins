#!/usr/bin/env bash
# Hermetic regression for scripts/smoke-official-cli.sh: the default mode must
# prove the *embedded* Google OAuth client (a client in the caller's
# environment must not satisfy it), the runtime-dummy mode is explicit, and
# the probe runs against a throwaway home. Also guards core-product-smoke
# against the retired `margins scan`.
set -euo pipefail

ROOT="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/margins-smoke-test.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

fail() { echo "test_smoke_official_cli: FAIL: $*" >&2; exit 1; }

# A fake packaged binary: the embedded client is FAKE_EMBEDDED; like the real
# binary, it also accepts a client from the run-time environment.
FAKE="$TMP/margins"
cat > "$FAKE" <<'SH'
#!/usr/bin/env bash
[ "$1" = "__release-smoke" ] || exit 64
printf '%s\n%s\n' "$HOME" "${MARGINS_HOME:-}" > "$FAKE_RECORD"
oauth=missing
if [ -n "${MARGINS_GOOGLE_OAUTH_CLIENT_JSON:-}" ]; then
  case "$MARGINS_GOOGLE_OAUTH_CLIENT_JSON" in *client_id*) oauth=valid ;; *) oauth=invalid ;; esac
elif [ "${FAKE_EMBEDDED:-0}" = "1" ]; then
  oauth=valid
fi
printf '{"schema":1,"official":true,"oauth_client":"%s","capture_available":true,"capture_provider":"native-recorder","tui_available":true,"recall":{"indexing":true,"lookup":true}}\n' "$oauth"
SH
chmod +x "$FAKE"
export FAKE_RECORD="$TMP/record"
SMOKE="$ROOT/scripts/smoke-official-cli.sh"
CALLER_CLIENT='{"installed":{"client_id":"caller"}}'

# 1. Default (embedded): a client only in the caller's environment fails.
if env FAKE_EMBEDDED=0 MARGINS_GOOGLE_OAUTH_CLIENT_JSON="$CALLER_CLIENT" \
  "$SMOKE" "$FAKE" >"$TMP/out1" 2>&1; then
  fail "embedded mode accepted a run-time client: $(cat "$TMP/out1")"
fi
grep -q "runtime-dummy" "$TMP/out1" || fail "embedded failure does not name the local escape hatch"

# 2. Embedded client present: passes, in a throwaway home.
env FAKE_EMBEDDED=1 MARGINS_GOOGLE_OAUTH_CLIENT_JSON="$CALLER_CLIENT" "$SMOKE" "$FAKE" \
  || fail "embedded mode rejected an embedded client"
smoke_home="$(sed -n 1p "$FAKE_RECORD")"
[ "$smoke_home" != "$HOME" ] || fail "release smoke ran against the caller's HOME"
[ "$(sed -n 2p "$FAKE_RECORD")" = "$smoke_home/.margins" ] || fail "MARGINS_HOME is not inside the throwaway home"
[ ! -e "$smoke_home" ] || fail "throwaway home was not removed"

# 3. runtime-dummy: explicit, noted, and passes without an embedded client.
env FAKE_EMBEDDED=0 MARGINS_SMOKE_OAUTH_CLIENT=runtime-dummy "$SMOKE" "$FAKE" 2>"$TMP/err3" \
  || fail "runtime-dummy mode failed"
grep -q "embedded client is not verified" "$TMP/err3" || fail "runtime-dummy mode is not announced"

# 4. Unknown modes are refused.
if env MARGINS_SMOKE_OAUTH_CLIENT=bogus "$SMOKE" "$FAKE" >/dev/null 2>&1; then
  fail "unknown MARGINS_SMOKE_OAUTH_CLIENT accepted"
fi

# 5. The core product smoke no longer runs the retired `margins scan`.
if grep -nE '(^|[[:space:]])scan([[:space:]]|$)' "$ROOT/scripts/core-product-smoke.sh" \
  | grep -v '^[0-9]*:[[:space:]]*#'; then
  fail "core-product-smoke.sh still runs \`margins scan\`"
fi

echo "test_smoke_official_cli: ok"
