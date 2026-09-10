# Product Critic: google-drive-export

## Strongest thing the diagnostic did
The do-not-do list is genuinely import-aware: "merge duplicate copies" and "run
another full import without user intent" directly address the duplicate
`client-kickoff` exports and the existing import run. Destination
`notes/meeting-notes/` matches expected.

## Most serious trust risk
This is the import-critical case and the user-facing narrative under-delivers on
it. Two problems:
1. "Hidden/cache state: no hidden app/cache folders visible" is misleading —
   `cache-state.json`, `import/drive-manifest.json`, and
   `google-drive-export.log` are present and record duplicate handling and parse
   errors. The diagnostic saw them (they are in evidence_files) but reported "no
   cache state."
2. The "History And Import" section is the same generic boilerplate used by
   empty and clean cases. The expected diagnostic wants the export churn
   (duplicates, half-converted `Untitled document.md`, partial cache) named so
   the user understands these are import leftovers, not curated notes.
The safety behavior is right; the explanation is generic where it most needs to
be specific.

## Would the user feel ready to start capture?
Yes.

## Is import/history understood as optional?
Optional, yes — but the user is not told what the import actually contains, so
"optional" lands without context.

## Surface / patch implicated
skill (primary): make the import narrative read the export log / manifest and
name duplicates and conversion leftovers in plain language. Also fix the
cache-state detection so visible (non-dot) import/cache files are reported.
