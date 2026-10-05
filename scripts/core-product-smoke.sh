#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
CARGO_LANE="$REPO_ROOT/scripts/cargo-lane"

DURATION_SECS="${MARGINS_CORE_PRODUCT_CAPTURE_SECS:-6}"
RUN_BUILD=1
RUN_TESTS=1
RUN_WORKSPACE=1
RUN_WORKSPACE_RECALL=0
RUN_LIVE=1
KEEP_RUNROOT="${KEEP_MARGINS_CORE_PRODUCT_SMOKE:-0}"
TITLE="core-product-smoke"
INTERNAL_FOCUSED_TESTS=0

usage() {
  cat <<'EOF'
Usage: scripts/core-product-smoke.sh [options]

Runs the high-signal Margins product smoke:
  - focused capture/permission/TUI/finalizer/private-composition tests in one lane
  - canonical native release build and __release-smoke contract
  - hermetic workspace structure smoke, with optional generator-backed recall
  - hermetic terminal-owned live capture smoke with artifact inspection

Options:
  --skip-build        Do not run cargo check/build; use existing release binary
  --skip-tests        Do not run focused cargo tests
  --skip-workspace    Do not run synthetic workspace/recall smoke
  --workspace-recall  Also run strict generator-backed workspace/recall e2e
  --skip-live         Do not run real terminal capture smoke
  --duration SECS     Live capture duration before Ctrl-C (default: 6)
  --title TITLE       Live capture title (default: core-product-smoke)
  --keep-runroot      Keep the hermetic live smoke temp tree
  -h, --help          Show this help

Environment:
  MARGINS_CORE_PRODUCT_BIN       Existing binary to test instead of target release path
  MARGINS_CORE_PRODUCT_LOG_DIR   Directory for logs (default: /tmp/margins-core-product-smoke.*)
  MARGINS_FLUID_COREML_MODEL_DIR Explicit FluidAudio CoreML model path for live smoke
  KEEP_MARGINS_CORE_PRODUCT_SMOKE=1 keeps the live temp tree

Run the live smoke from a real macOS terminal app with Microphone permission.
The script does not install the desktop app, touch /Applications/Margins.app, or
reset TCC.
EOF
}

die() {
  printf 'core-product-smoke: ERROR: %s\n' "$*" >&2
  exit 1
}

log() {
  printf '%s\n' "$*" | tee -a "$SUMMARY"
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --skip-build) RUN_BUILD=0; shift ;;
    --skip-tests) RUN_TESTS=0; shift ;;
    --skip-workspace) RUN_WORKSPACE=0; shift ;;
    --workspace-recall) RUN_WORKSPACE_RECALL=1; shift ;;
    --skip-live) RUN_LIVE=0; shift ;;
    --duration)
      [ "$#" -ge 2 ] || die "--duration requires a value"
      DURATION_SECS="$2"
      shift 2
      ;;
    --title)
      [ "$#" -ge 2 ] || die "--title requires a value"
      TITLE="$2"
      shift 2
      ;;
    --keep-runroot) KEEP_RUNROOT=1; shift ;;
    --internal-focused-tests) INTERNAL_FOCUSED_TESTS=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

case "$DURATION_SECS" in
  ''|*[!0-9]*) die "--duration must be a positive integer" ;;
  0) die "--duration must be greater than zero" ;;
esac

[ -x "$CARGO_LANE" ] || die "missing executable cargo lane wrapper: $CARGO_LANE"

run_logged() {
  local label="$1"
  shift
  local logfile="$LOG_ROOT/${label}.log"
  log "RUN $label: $*"
  set +e
  (
    cd "$REPO_ROOT"
    "$@"
  ) 2>&1 | tee "$logfile"
  local status=${PIPESTATUS[0]}
  set -e
  if [ "$status" -ne 0 ]; then
    log "FAIL $label status=$status log=$logfile"
    exit "$status"
  fi
  log "PASS $label log=$logfile"
}

binary_path() {
  if [ -n "${MARGINS_CORE_PRODUCT_BIN:-}" ]; then
    printf '%s\n' "$MARGINS_CORE_PRODUCT_BIN"
    return
  fi
  local target_dir
  target_dir="$("$CARGO_LANE" target-dir)"
  printf '%s/release/margins-private\n' "$target_dir"
}

require_origin() {
  local origin
  origin="$(cd "$REPO_ROOT" && git remote get-url origin)"
  case "$origin" in
    https://github.com/byenzyme/margins-desktop.git|https://github.com/byenzyme/margins.git) ;;
    *) die "unexpected origin: $origin" ;;
  esac
  log "origin=$origin"
}

find_coreml_model_dir() {
  if [ -n "${MARGINS_FLUID_COREML_MODEL_DIR:-}" ]; then
    printf '%s\n' "$MARGINS_FLUID_COREML_MODEL_DIR"
    return
  fi
  local real_home="${HOME:?HOME is required}"
  local candidate
  for candidate in \
    "$real_home/Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v2" \
    "$real_home/Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v3"
  do
    if [ -d "$candidate/Preprocessor.mlmodelc" ] \
        && [ -d "$candidate/Encoder.mlmodelc" ] \
        && [ -d "$candidate/Decoder.mlmodelc" ] \
        && [ -d "$candidate/JointDecision.mlmodelc" ] \
        && [ -f "$candidate/parakeet_vocab.json" ]; then
      printf '%s\n' "$candidate"
      return
    fi
  done
  return 1
}

run_focused_tests() {
  cd "$REPO_ROOT"

  printf '\n== margins-core capture permission/health ==\n'
  cargo test -p margins-core --lib capture::tests::

  printf '\n== public CLI permission contract ==\n'
  cargo test -p margins-cli --test command_contract permission

  printf '\n== native permission preflight ==\n'
  cargo test -p margins --lib \
    --no-default-features --features audio-capture,coreml-asr native_permission_preflight

  printf '\n== optional transcript finalization ==\n'
  cargo test -p margins --lib \
    --no-default-features --features audio-capture,coreml-asr optional_transcript

  printf '\n== capture queue accounting ==\n'
  cargo test -p margins --lib \
    --no-default-features --features audio-capture,coreml-asr queue_accounting

  printf '\n== TUI capture health ==\n'
  cargo test -p margins --lib \
    --no-default-features --features audio-capture,coreml-asr tui::tests::

  printf '\n== private production composition ==\n'
  cargo test -p margins --test private_cli_composition production_

  printf '\n== packaged native composition ==\n'
  cargo test -p margins --test private_cli_composition \
    packaged_binary_reports_private_native_composition
}

run_live_smoke() {
  local binary="$1"
  local model_dir="$2"
  local smoke_root
  smoke_root="$(mktemp -d "$LOG_ROOT/live-runroot.XXXXXX")"
  mkdir -p "$smoke_root/home" "$smoke_root/margins-home" "$smoke_root/vault"

  log "RUN live-capture: root=$smoke_root duration=${DURATION_SECS}s model=$model_dir"
  python3 - "$binary" "$smoke_root" "$DURATION_SECS" "$TITLE" "$model_dir" "$LOG_ROOT/live-pty.log" <<'PY'
import os
import pathlib
import pty
import select
import signal
import sys
import time

binary = pathlib.Path(sys.argv[1])
root = pathlib.Path(sys.argv[2])
duration = int(sys.argv[3])
title = sys.argv[4]
model_dir = sys.argv[5]
pty_log = pathlib.Path(sys.argv[6])

env = os.environ.copy()
env["HOME"] = str(root / "home")
env["MARGINS_HOME"] = str(root / "margins-home")
env["MARGINS_FLUID_COREML_MODEL_DIR"] = model_dir
env.pop("MARGINS_PROJECT", None)
env.pop("MARGINS_WORKSPACE", None)

pid, fd = pty.fork()
if pid == 0:
    os.chdir(root / "vault")
    os.execve(str(binary), [str(binary), "new", "--title", title], env)

sent_ctrl_c = False
deadline = time.monotonic() + duration
hard_deadline = deadline + 90
status = None
with pty_log.open("wb") as out:
    while True:
        now = time.monotonic()
        if not sent_ctrl_c and now >= deadline:
            os.write(fd, b"\x03")
            sent_ctrl_c = True
        if now >= hard_deadline:
            os.kill(pid, signal.SIGTERM)
            raise SystemExit("live capture did not exit after Ctrl-C")
        ready, _, _ = select.select([fd], [], [], 0.25)
        if ready:
            try:
                chunk = os.read(fd, 4096)
            except OSError:
                chunk = b""
            if chunk:
                sys.stdout.buffer.write(chunk)
                sys.stdout.buffer.flush()
                out.write(chunk)
                out.flush()
        waited_pid, raw_status = os.waitpid(pid, os.WNOHANG)
        if waited_pid == pid:
            status = raw_status
            break

if os.WIFEXITED(status):
    code = os.WEXITSTATUS(status)
elif os.WIFSIGNALED(status):
    code = 128 + os.WTERMSIG(status)
else:
    code = 1
raise SystemExit(code)
PY
  log "PASS live-capture command log=$LOG_ROOT/live-pty.log"

  python3 - "$smoke_root" "$LOG_ROOT" <<'PY'
import json
import pathlib
import re
import sqlite3
import struct
import sys
import wave

root = pathlib.Path(sys.argv[1])
log_root = pathlib.Path(sys.argv[2])
margins_dir = root / "vault" / ".margins"
current = margins_dir / "current"
if not current.is_file():
    raise SystemExit("missing current pointer")
session_name = current.read_text().strip()
memo = margins_dir / f"{session_name}.md"
wav_path = margins_dir / f"{session_name}_seg0.wav"
db_path = margins_dir / "sessions.sqlite"
cli_log = root / "home" / "Library" / "Logs" / "Margins" / "cli.log"
for path in (memo, wav_path, db_path):
    if not path.exists():
        raise SystemExit(f"missing smoke artifact: {path}")

with wave.open(str(wav_path), "rb") as handle:
    channels = handle.getnchannels()
    sample_rate = handle.getframerate()
    sample_width = handle.getsampwidth()
    frames = handle.getnframes()
    raw = handle.readframes(frames)
if channels != 2:
    raise SystemExit(f"expected stereo WAV, got {channels} channels")
if frames <= 0 or len(raw) <= 0:
    raise SystemExit("WAV has no audio frames")
if sample_width != 2:
    raise SystemExit(f"expected 16-bit PCM WAV, got sample width {sample_width}")

values = struct.unpack("<" + "h" * (len(raw) // 2), raw)
per_channel = []
for index in range(channels):
    channel = values[index::channels]
    absolute = [abs(value) for value in channel]
    per_channel.append({
        "channel": index,
        "samples": len(channel),
        "nonzero_samples": sum(1 for value in channel if value != 0),
        "max_abs": max(absolute) if absolute else 0,
        "mean_abs": round(sum(absolute) / len(absolute), 3) if absolute else 0,
    })

db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
db.row_factory = sqlite3.Row
session = db.execute("select * from sessions where name = ?", [session_name]).fetchone()
segment = db.execute(
    "select * from session_segments where session_name = ? order by segment_index",
    [session_name],
).fetchall()
artifacts = db.execute(
    "select * from session_artifacts where session_name = ? order by kind, ordinal",
    [session_name],
).fetchall()
if session is None:
    raise SystemExit("session row was not durable")
if not segment:
    raise SystemExit("segment row was not durable")
if not any(row["kind"] == "audio" and row["retention_class"] == "durable" for row in artifacts):
    raise SystemExit("durable audio artifact row was not recorded")

logs = cli_log.read_text(errors="replace") if cli_log.exists() else ""
pty = (log_root / "live-pty.log").read_text(errors="replace")
panic_terms = re.compile(r"panic|panicked|arithmetic|overflow|attempt to subtract", re.I)
if panic_terms.search(logs) or panic_terms.search(pty):
    raise SystemExit("panic-like text appeared in live smoke logs")
if "<margins_error" in pty or "command_failed" in pty:
    raise SystemExit("live smoke command returned a Margins command failure")

summary = {
    "smoke_root": str(root),
    "session_name": session_name,
    "memo_path": str(memo),
    "memo_bytes": memo.stat().st_size,
    "wav_path": str(wav_path),
    "wav_bytes": wav_path.stat().st_size,
    "channels": channels,
    "sample_rate": sample_rate,
    "sample_width_bytes": sample_width,
    "frames": frames,
    "duration_s": frames / sample_rate if sample_rate else None,
    "per_channel": per_channel,
    "session_row": dict(session),
    "segment_rows": [dict(row) for row in segment],
    "artifact_rows": [dict(row) for row in artifacts],
    "live_worker_started": "live_worker_warmup_started" in logs,
    "live_worker_ready": "live_worker_ready" in logs,
    "live_worker_degraded": "live_worker_finished status=unavailable" in logs
        or "live_worker_unavailable" in logs
        or "live_worker_warmup_failed" in logs,
    "closed_channel_recorded_nonfatal": "sending on a closed channel" in logs,
    "system_audio_empty": per_channel[1]["nonzero_samples"] == 0,
}
(log_root / "live-summary.json").write_text(json.dumps(summary, indent=2) + "\n")
(log_root / "live-cli.log").write_text(logs)
print(json.dumps(summary, indent=2))
PY
  log "PASS live-inspect log=$LOG_ROOT/live-summary.json"

  if [ "$KEEP_RUNROOT" = "1" ]; then
    log "keeping live runroot=$smoke_root"
  else
    find "$smoke_root" -maxdepth 5 -print | sort > "$LOG_ROOT/live-tree-before-cleanup.log"
    rm -R -- "$smoke_root"
    log "removed live runroot=$smoke_root"
  fi
}

run_workspace_structural_smoke() {
  local binary="$1"
  local workspace_root
  workspace_root="$(mktemp -d "$LOG_ROOT/workspace-runroot.XXXXXX")"
  mkdir -p "$workspace_root/home" "$workspace_root/margins-home" "$workspace_root/notes/projects"

  cat > "$workspace_root/notes/projects/Atlas.md" <<'EOF'
# Atlas
The phosphorescent handoff preserves the decision boundary after forty-eight hours.
Rui owns the continuity scorecard and Leah reviews exceptions.
EOF

  log "RUN workspace-structural: root=$workspace_root"
  run_logged "12a-workspace-new" \
    env HOME="$workspace_root/home" MARGINS_HOME="$workspace_root/margins-home" \
    "$binary" workspace new core-product-smoke --home "$workspace_root/notes"
  run_logged "12b-workspace-scan" \
    env HOME="$workspace_root/home" MARGINS_HOME="$workspace_root/margins-home" \
    "$binary" --workspace core-product-smoke scan
  run_logged "12c-workspace-status" \
    env HOME="$workspace_root/home" MARGINS_HOME="$workspace_root/margins-home" \
    "$binary" --workspace core-product-smoke workspace status --json
  run_logged "12d-source-list" \
    env HOME="$workspace_root/home" MARGINS_HOME="$workspace_root/margins-home" \
    "$binary" --workspace core-product-smoke source list --json

  cp "$LOG_ROOT/12c-workspace-status.log" "$LOG_ROOT/workspace-status.json"
  cp "$LOG_ROOT/12d-source-list.log" "$LOG_ROOT/workspace-sources.json"
  python3 - "$workspace_root" "$LOG_ROOT" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
log_root = pathlib.Path(sys.argv[2])
status = json.loads((log_root / "workspace-status.json").read_text())
sources = json.loads((log_root / "workspace-sources.json").read_text())
state = root / "margins-home" / "workspaces" / "core-product-smoke"
notes = root / "notes"
if status.get("id") != "core-product-smoke":
    raise SystemExit(f"unexpected workspace id: {status.get('id')!r}")
if pathlib.Path(status.get("home", "")).resolve() != notes.resolve():
    raise SystemExit("workspace home did not resolve to fixture notes")
if not (state / "config.toml").is_file():
    raise SystemExit("workspace config.toml was not created")
if (notes / ".margins").exists():
    raise SystemExit("workspace state leaked into notes home")
home_sources = [source for source in sources if source.get("role") == "home"]
if len(home_sources) != 1:
    raise SystemExit(f"expected one home source, got {len(home_sources)}")
summary = {
    "workspace_root": str(root),
    "workspace_id": status.get("id"),
    "workspace_state": str(state),
    "home": str(notes),
    "source_count": len(sources),
    "home_source": home_sources[0],
}
(log_root / "workspace-summary.json").write_text(json.dumps(summary, indent=2) + "\n")
print(json.dumps(summary, indent=2))
PY
  log "PASS workspace-structural log=$LOG_ROOT/workspace-summary.json"

  if [ "$KEEP_RUNROOT" = "1" ]; then
    log "keeping workspace runroot=$workspace_root"
  else
    find "$workspace_root" -maxdepth 5 -print | sort > "$LOG_ROOT/workspace-tree-before-cleanup.log"
    rm -R -- "$workspace_root"
    log "removed workspace runroot=$workspace_root"
  fi
}

if [ "$INTERNAL_FOCUSED_TESTS" = "1" ]; then
  run_focused_tests
  exit 0
fi

LOG_ROOT="${MARGINS_CORE_PRODUCT_LOG_DIR:-}"
if [ -z "$LOG_ROOT" ]; then
  LOG_ROOT="$(mktemp -d "/tmp/margins-core-product-smoke.XXXXXX")"
else
  mkdir -p "$LOG_ROOT"
fi
LOG_ROOT="$(CDPATH= cd -- "$LOG_ROOT" && pwd -P)"
SUMMARY="$LOG_ROOT/summary.txt"
: > "$SUMMARY"

require_origin
log "repo=$REPO_ROOT"
log "logs=$LOG_ROOT"
run_logged "00-cargo-lane-status" "$CARGO_LANE" status

if [ "$RUN_TESTS" = "1" ]; then
  run_logged "02-focused-tests" \
    "$CARGO_LANE" disposable -- "$REPO_ROOT/scripts/core-product-smoke.sh" \
    --internal-focused-tests
fi

if [ "$RUN_BUILD" = "1" ]; then
  run_logged "10-build-release-margins-private" \
    "$CARGO_LANE" shared -- cargo build --release --bin margins-private
fi

BINARY="$(binary_path)"
[ -x "$BINARY" ] || die "binary is not executable: $BINARY"
log "binary=$BINARY"
run_logged "11-release-smoke" "$REPO_ROOT/scripts/smoke-official-cli.sh" "$BINARY"
"$BINARY" __release-smoke > "$LOG_ROOT/release-smoke.json"

if [ "$RUN_WORKSPACE" = "1" ]; then
  run_workspace_structural_smoke "$BINARY"
  if [ "$RUN_WORKSPACE_RECALL" = "1" ]; then
    run_logged "12f-workspace-recall-smoke" \
      env MARGINS_BIN="$BINARY" "$REPO_ROOT/scripts/e2e-fresh-workspace-setup.sh" synthetic
  fi
fi

if [ "$RUN_LIVE" = "1" ]; then
  MODEL_DIR="$(find_coreml_model_dir)" || \
    die "no FluidAudio CoreML model cache found; set MARGINS_FLUID_COREML_MODEL_DIR for live smoke"
  run_live_smoke "$BINARY" "$MODEL_DIR"
fi

run_logged "99-cargo-lane-status" "$CARGO_LANE" status
log "core-product-smoke: PASS"
