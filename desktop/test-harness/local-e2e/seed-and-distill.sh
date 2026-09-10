#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PORT="${MARGINS_PORT:-8788}"
HOME_DIR="${MARGINS_HOME:?set MARGINS_HOME to the server HOME}"
DATA_DIR="${MARGINS_DATA_DIR:?set MARGINS_DATA_DIR to the server data dir}"

HOME="$HOME_DIR" \
MARGINS_DATA_DIR="$DATA_DIR" \
MARGINS_E2E_SERVER="http://127.0.0.1:$PORT" \
  "$HERE/distill-fixture.mjs"
