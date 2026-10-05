#!/usr/bin/env python3
"""Report secret and personal-data hits in tracked files or added commit lines.

The scanner reads Git objects directly for history, so it also sees files that
have since been deleted. Output names the location and rule but never the
matched value. Every hit is reported; consumers decide which are synthetic or
which history paths the public import must filter.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys


ROOT = Path(__file__).resolve().parent.parent
CONFIG = json.loads((ROOT / "scripts/public_scan_config.json").read_text())
RULES = {name: re.compile(pattern) for name, pattern in CONFIG["rules"].items()}
FILTER_PATHS = CONFIG["history_filter_paths"]
SYNTHETIC_DOMAINS = set(CONFIG["synthetic_email_domains"])
SYNTHETIC_SECRET_PATHS = set(CONFIG["synthetic_secret_paths"])
DOCUMENTED_PERSONAL_PATH_PATHS = set(
    CONFIG.get("documented_personal_path_paths", [])
)
RULE_PATHS = {name: set(paths) for name, paths in CONFIG.get("rule_paths", {}).items()}
SECRET_RULES = set(RULES) - {"email-address", "personal-macos-path", "personal-linux-path"}


def git(*args: str) -> bytes:
    result = subprocess.run(
        ["git", "-C", str(ROOT), *args], capture_output=True, check=False
    )
    if result.returncode:
        raise RuntimeError(result.stderr.decode("utf-8", "replace").strip())
    return result.stdout


def disposition(path: str, rule: str, matched: str) -> str:
    if any(path == item or item.endswith("/") and path.startswith(item) for item in FILTER_PATHS):
        return "filter-from-public-history"
    if rule == "email-address":
        domain = matched.rsplit("@", 1)[-1].lower()
        if domain in SYNTHETIC_DOMAINS or domain.endswith((".test", ".example", ".invalid")):
            return "synthetic-email"
    if (
        rule in {"personal-macos-path", "personal-linux-path"}
        and path in DOCUMENTED_PERSONAL_PATH_PATHS
    ):
        return "documented-replacement"
    if rule in SECRET_RULES and path in SYNTHETIC_SECRET_PATHS:
        return "synthetic-test-secret"
    return "review"


def scan_line(path: str, number: int, line: str, commit: str | None = None) -> list[dict]:
    hits = []
    for rule, pattern in RULES.items():
        if rule in RULE_PATHS and path not in RULE_PATHS[rule]:
            continue
        for match in pattern.finditer(line):
            hits.append({
                "commit": commit,
                "path": path,
                "line": number,
                "rule": rule,
                "disposition": disposition(path, rule, match.group()),
            })
    return hits


def scan_tree() -> list[dict]:
    hits = []
    for raw_path in git("ls-files", "--cached", "--others", "--exclude-standard", "-z").split(b"\0"):
        if not raw_path:
            continue
        path = raw_path.decode("utf-8", "replace")
        source = ROOT / path
        if not source.is_file() or source.is_symlink():
            continue
        data = source.read_bytes()
        if b"\0" in data:
            continue
        for number, line in enumerate(data.decode("utf-8", "replace").splitlines(), 1):
            hits.extend(scan_line(path, number, line))
    return hits


def scan_history(revision_range: str) -> list[dict]:
    hits = []
    commits = git("rev-list", "--reverse", revision_range).decode().splitlines()
    for commit in commits:
        # --unified=0 scans newly added lines once per commit, rather than
        # restating unchanged content for every subsequent snapshot.
        patch = git("show", "--format=", "--no-ext-diff", "--unified=0", "--no-renames", commit)
        path = None
        line_number = 0
        for line in patch.decode("utf-8", "replace").splitlines():
            if line.startswith("+++ b/"):
                path = line[6:]
            elif line.startswith("@@"):
                found = re.search(r"\+(\d+)", line)
                line_number = int(found.group(1)) if found else 0
            elif line.startswith("+") and not line.startswith("+++"):
                if path is not None:
                    hits.extend(scan_line(path, line_number, line[1:], commit))
                line_number += 1
            elif line.startswith(" "):
                line_number += 1
    return hits


def scan_git_tree(revision: str) -> list[dict]:
    """Scan every text blob in one committed tree, including a future squash base."""
    resolved = git("rev-parse", "--verify", f"{revision}^{{commit}}").decode().strip()
    hits = []
    paths = git("ls-tree", "-r", "--name-only", "-z", resolved).split(b"\0")
    for raw_path in paths:
        if not raw_path:
            continue
        path = raw_path.decode("utf-8", "replace")
        data = git("show", f"{resolved}:{path}")
        if b"\0" in data:
            continue
        for number, line in enumerate(data.decode("utf-8", "replace").splitlines(), 1):
            hits.extend(scan_line(path, number, line, resolved))
    return hits


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group()
    source.add_argument(
        "--range",
        dest="revision_range",
        help="Git revision range, e.g. 54a7a9d7c..HEAD",
    )
    source.add_argument(
        "--tree", dest="tree_revision", help="single committed tree, e.g. 54a7a9d7c"
    )
    parser.add_argument("--json", action="store_true", help="machine-readable report")
    parser.add_argument("--fail-on-review", action="store_true", help="fail if any hit needs review")
    parser.add_argument("--fail-on-secret", action="store_true", help="fail on an unreviewed secret-shaped hit")
    args = parser.parse_args()
    try:
        if args.revision_range:
            hits = scan_history(args.revision_range)
        elif args.tree_revision:
            hits = scan_git_tree(args.tree_revision)
        else:
            hits = scan_tree()
    except RuntimeError as error:
        parser.exit(2, f"scan-public-history: {error}\n")
    if args.json:
        print(
            json.dumps(
                {
                    "range": args.revision_range,
                    "tree": args.tree_revision,
                    "hits": hits,
                },
                indent=2,
            )
        )
    else:
        print(f"scan-public-history: {len(hits)} hit(s)")
        for hit in hits:
            where = f"{hit['path']}:{hit['line']}"
            if hit["commit"]:
                where = f"{hit['commit'][:12]} {where}"
            print(f"  {where} {hit['rule']} [{hit['disposition']}]")
    unreviewed = [hit for hit in hits if hit["disposition"] == "review"]
    return int(args.fail_on_review and unreviewed or args.fail_on_secret and any(hit["rule"] in SECRET_RULES for hit in unreviewed))


if __name__ == "__main__":
    sys.exit(main())
