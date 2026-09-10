# Workspace Diagnosis Memory

## Language That Worked

- "You can start now" plus "existing files will not be moved, renamed,
  retagged, normalized, or rewritten" preserved trust across all cases.
- Naming the next note destination as a future capture default avoided making
  structure sound like a migration.
- Framing private cache, import, and repair as separate optional actions kept
  Enzyme setup from blocking first capture.

## Language That Failed

- Generic "history can help later" prose was too thin for Drive exports,
  transcript dumps, and partial migration state.
- Raw convention labels such as malformed metadata or CSV-style exports must be
  translated into user-facing outcome language.
- Cache/import/export evidence must be named even when it is in visible files
  rather than hidden `.enzyme` or `.margins` folders.

## Reusable Workspace Patterns

- Empty folders need a short destination default and almost no import language.
- Codebases need source-file non-mutation reassurance and a docs-adjacent meeting
  destination.
- Daily-note vaults may have competing Daily and meetings folder conventions;
  name the tradeoff instead of denying either.
- Foreign-domain folders need isolated meeting-note destinations so meetings do
  not blend into unrelated personal/admin material.
- Partial `.enzyme` / `.margins` state requires a temp-copy scan guard during
  evaluation.

## Import/Transcript Lessons

- Drive/Zoom/Granola-style leftovers should be described in plain language:
  duplicates, half-converted notes, export logs, transcript dumps, and import
  manifests.
- Import history can be useful later, but it must never sound required before
  first capture.

## Read/Write Boundary Lessons

- `enzyme scan --vault <workspace>` without `--write-config` is not enough when
  a fixture already contains `.enzyme/`; it may create runtime log residue.
- For fixtures with existing tool state, scan a temporary copy and record
  pre/post mutation guards.

## Skill Blind Spots

- The baseline diagnostic was safe but shallow: it treated the Enzyme lens as
  classification evidence rather than explaining usable vs. weak retrieval
  signal.
- Confidence should drop when a workspace has strong structure that is not
  clearly a meeting-note workflow.

## Fixture Gaps To Add Later

- A fixture with supported import preview files but no Markdown notes.
- A fixture with a large `.enzyme` directory that must be excluded from scan
  evidence.
- A fixture with multiple plausible meeting destinations and no manifest-chosen
  winner.
