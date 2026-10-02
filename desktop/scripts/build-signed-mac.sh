#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_DIR="$(cd "$DESKTOP_DIR/.." && pwd)"

load_env_file() {
  local env_file="$1"
  [[ -f "$env_file" ]] || return 0

  # Source shell-compatible dotenv files. Existing exported values are restored
  # below so one-off command-line overrides still win over local defaults.
  set -a
  # shellcheck disable=SC1090
  source "$env_file"
  set +a
}

restore_if_preexisting() {
  local name="$1"
  local value="$2"
  local was_set="$3"
  if [[ "$was_set" == "1" ]]; then
    printf -v "$name" '%s' "$value"
    export "$name"
  fi
}

PRESET_APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY+x}"
PRESET_APPLE_SIGNING_IDENTITY_VALUE="${APPLE_SIGNING_IDENTITY:-}"
PRESET_APPLE_ID="${APPLE_ID+x}"
PRESET_APPLE_ID_VALUE="${APPLE_ID:-}"
PRESET_APPLE_PASSWORD="${APPLE_PASSWORD+x}"
PRESET_APPLE_PASSWORD_VALUE="${APPLE_PASSWORD:-}"
PRESET_APPLE_TEAM_ID="${APPLE_TEAM_ID+x}"
PRESET_APPLE_TEAM_ID_VALUE="${APPLE_TEAM_ID:-}"
PRESET_APPLE_API_ISSUER="${APPLE_API_ISSUER+x}"
PRESET_APPLE_API_ISSUER_VALUE="${APPLE_API_ISSUER:-}"
PRESET_APPLE_API_KEY="${APPLE_API_KEY+x}"
PRESET_APPLE_API_KEY_VALUE="${APPLE_API_KEY:-}"
PRESET_APPLE_API_KEY_PATH="${APPLE_API_KEY_PATH+x}"
PRESET_APPLE_API_KEY_PATH_VALUE="${APPLE_API_KEY_PATH:-}"

load_env_file "$REPO_DIR/.env"
load_env_file "$DESKTOP_DIR/.env"

restore_if_preexisting APPLE_SIGNING_IDENTITY "$PRESET_APPLE_SIGNING_IDENTITY_VALUE" "${PRESET_APPLE_SIGNING_IDENTITY:+1}"
restore_if_preexisting APPLE_ID "$PRESET_APPLE_ID_VALUE" "${PRESET_APPLE_ID:+1}"
restore_if_preexisting APPLE_PASSWORD "$PRESET_APPLE_PASSWORD_VALUE" "${PRESET_APPLE_PASSWORD:+1}"
restore_if_preexisting APPLE_TEAM_ID "$PRESET_APPLE_TEAM_ID_VALUE" "${PRESET_APPLE_TEAM_ID:+1}"
restore_if_preexisting APPLE_API_ISSUER "$PRESET_APPLE_API_ISSUER_VALUE" "${PRESET_APPLE_API_ISSUER:+1}"
restore_if_preexisting APPLE_API_KEY "$PRESET_APPLE_API_KEY_VALUE" "${PRESET_APPLE_API_KEY:+1}"
restore_if_preexisting APPLE_API_KEY_PATH "$PRESET_APPLE_API_KEY_PATH_VALUE" "${PRESET_APPLE_API_KEY_PATH:+1}"

APP_NAME="${MARGINS_APP_NAME:-Margins}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${HOME}/.cache/margins-cargo-target}"
TAURI_CONFIG="${MARGINS_TAURI_CONFIG:-tauri.bundle-mac.conf.json}"
TAURI_BUNDLES="${MARGINS_TAURI_BUNDLES:-app,dmg}"

export CARGO_TARGET_DIR

has_apple_id_notary_auth() {
  [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]
}

has_api_key_notary_auth() {
  [[ -n "${APPLE_API_ISSUER:-}" && -n "${APPLE_API_KEY:-}" && -n "${APPLE_API_KEY_PATH:-}" ]]
}

notarytool_auth_args() {
  if has_apple_id_notary_auth; then
    printf '%s\0' \
      "--apple-id" "$APPLE_ID" \
      "--password" "$APPLE_PASSWORD" \
      "--team-id" "$APPLE_TEAM_ID"
  elif has_api_key_notary_auth; then
    printf '%s\0' \
      "--issuer" "$APPLE_API_ISSUER" \
      "--key-id" "$APPLE_API_KEY" \
      "--key" "$APPLE_API_KEY_PATH"
  fi
}

notarize_and_staple() {
  local artifact="$1"
  local auth_args=()

  while IFS= read -r -d '' arg; do
    auth_args+=("$arg")
  done < <(notarytool_auth_args)

  echo "Notarizing $artifact..."
  xcrun notarytool submit "$artifact" --wait "${auth_args[@]}"
  echo "Stapling $artifact..."
  xcrun stapler staple "$artifact"
}

if [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  APPLE_SIGNING_IDENTITY="$(
    security find-identity -v -p codesigning \
      | awk -F\" '/Developer ID Application/ { print $2; exit }'
  )"
fi
export APPLE_SIGNING_IDENTITY

IDENTITY_LINE="$(
  security find-identity -v -p codesigning \
    | grep -F "\"$APPLE_SIGNING_IDENTITY\"" \
    | head -n 1 \
    || true
)"

if [[ -z "$IDENTITY_LINE" && "$APPLE_SIGNING_IDENTITY" =~ ^[A-Fa-f0-9]{40}$ ]]; then
  IDENTITY_LINE="$(
    security find-identity -v -p codesigning \
      | awk -v hash="$APPLE_SIGNING_IDENTITY" '$0 ~ hash { print; exit }'
  )"
fi

if [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  cat >&2 <<'EOF'
No Developer ID Application signing identity was found.

Install the Developer ID Application certificate in Keychain Access, then retry.
Current visible code-signing identities:
EOF
  security find-identity -v -p codesigning >&2 || true
  exit 1
fi

if [[ -z "$IDENTITY_LINE" ]]; then
  cat >&2 <<EOF
The configured signing identity is not visible to codesign:
$APPLE_SIGNING_IDENTITY
EOF
  exit 1
fi

if [[ "$IDENTITY_LINE" != *\"Developer\ ID\ Application:*\"* ]]; then
  cat >&2 <<EOF
APPLE_SIGNING_IDENTITY must be a Developer ID Application identity for public distribution.
Got: $APPLE_SIGNING_IDENTITY
EOF
  exit 1
fi

if ! has_apple_id_notary_auth && ! has_api_key_notary_auth; then
  cat >&2 <<'EOF'
Missing Apple notarization credentials.

Use Apple ID auth:
  export APPLE_ID="you@example.com"
  export APPLE_PASSWORD="app-specific-password"
  export APPLE_TEAM_ID="TEAMID"

Or App Store Connect API auth:
  export APPLE_API_ISSUER="issuer-id"
  export APPLE_API_KEY="key-id"
  export APPLE_API_KEY_PATH="/path/to/AuthKey_KEYID.p8"
EOF
  exit 1
fi

echo "Building signed and notarized ${APP_NAME}.app..."
echo "  identity: ${APPLE_SIGNING_IDENTITY}"
echo "  target:   ${CARGO_TARGET_DIR}"
echo "  bundles:  ${TAURI_BUNDLES}"

TAURI_SIGNING_CONFIG="$(
  node -e '
    const identity = process.env.APPLE_SIGNING_IDENTITY;
    const providerShortName = process.env.APPLE_TEAM_ID || null;
    const macOS = { signingIdentity: identity };
    if (providerShortName) macOS.providerShortName = providerShortName;
    process.stdout.write(JSON.stringify({ bundle: { macOS } }));
  '
)"

(
  cd "$DESKTOP_DIR"
  npm run tauri -- build --bundles "$TAURI_BUNDLES" --config "$TAURI_CONFIG" --config "$TAURI_SIGNING_CONFIG" "$@"
)

APP_PATH="${CARGO_TARGET_DIR}/release/bundle/macos/${APP_NAME}.app"
if [[ ! -d "$APP_PATH" ]]; then
  echo "Expected app bundle not found: $APP_PATH" >&2
  exit 1
fi

DMG_DIR="${CARGO_TARGET_DIR}/release/bundle/dmg"
shopt -s nullglob
DMG_MATCHES=("$DMG_DIR"/"${APP_NAME}"_*.dmg)
shopt -u nullglob

if [[ "${#DMG_MATCHES[@]}" -eq 0 ]]; then
  echo "Expected DMG not found in: $DMG_DIR" >&2
  exit 1
fi

DMG_PATH="${DMG_MATCHES[0]}"

echo "Verifying app signature and notarization..."
codesign --verify --deep --strict --verbose=2 "$APP_PATH"
spctl --assess --type execute --verbose=4 "$APP_PATH"
xcrun stapler validate "$APP_PATH"

if [[ "${MARGINS_NOTARIZE_DMG:-1}" == "1" ]]; then
  notarize_and_staple "$DMG_PATH"
fi

echo "Verifying DMG signature and notarization..."
spctl --assess --type open --context context:primary-signature --verbose=4 "$DMG_PATH"
xcrun stapler validate "$DMG_PATH"

echo "Signed release artifacts are ready:"
echo "  $APP_PATH"
echo "  $DMG_PATH"
