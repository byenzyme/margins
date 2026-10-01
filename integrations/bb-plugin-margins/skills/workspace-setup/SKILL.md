---
name: workspace-setup
description: Help the user show Margins which notes it may read and where it may write. Use when the user wants to point Margins at notes, get started, change sources, or confirm setup worked. Do not use for live meeting control or post-meeting distillation.
allowed-tools: Bash, Read, Glob, Grep
---

# Workspace Setup

Help the user show Margins how one set of notes is organized. This is separate
from live meeting feedback and from turning a finished meeting into a note.

For first recording setup in bb, open **Meetings** and select the notes project.
The panel discovers existing note structure, chooses the `inbox` note destination
by default, shows the exact Workspace changes, and applies them after the user
accepts. No agent conversation is required to begin recording. An empty notes
folder is valid; recall enrichment can be completed later.

For additional Sources, corrections to the recall policy, and recall proof,
use the canonical Margins guide as the source of truth:

```bash
margins guide workspace-setup
```

Follow that guide end to end for advanced setup. The direct Meetings form uses
the same `workspace plan --desired` and reviewed `apply` contract.

The setup conversation should leave the user with:

- a plain-language account of what their notes appear to be about;
- the folder Margins may write approved notes into;
- any other folders Margins may search but not change;
- only the settings needed to keep that account true; and
- one real example showing that Margins can find an existing note.

Important boundaries:

- Do not start, stop, or inspect live meeting capture as part of setup.
- Do not transcribe a meeting, process a session, or draft a connected note.
- Do not create `.margins` inside the user's notes folder.
- Do not read credential bundles, auth files, token stores, shell history, or
  environment variable values.
- Do not run recall before `init` and `sync` have completed for the selected
  Workspace.
- Do not apply a workspace plan until the user has seen the plan and explicitly
  agreed.

When setup finishes, report the Workspace, its home and reference Sources, what
changed, the `init` and `sync` result, and the recall proof. Say what Margins can
now remember in the user's own terms.
