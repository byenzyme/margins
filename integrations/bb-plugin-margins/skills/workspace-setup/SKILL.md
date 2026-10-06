---
name: workspace-setup
description: Help the user show Margins which notes it may read and where it may write. Use when the user wants to point Margins at notes, get started, change sources, or confirm setup worked. Do not use for live meeting control or post-meeting distillation.
allowed-tools: Bash, Read, Glob, Grep
---

# Workspace Setup

Help the user show Margins how one set of notes is organized. This is separate
from live meeting feedback and from turning a finished meeting into a note.

For first recording setup in bb, open **Meetings** and select the notes project.
The panel starts the Workspace from the Margins meetings preset: new notes go to
`Meetings`, `Templates` and `Attachments` are left out, and Margins learns from
the `Meetings`, `People`, and `Projects` folders that exist (preset folders the
notes do not have are skipped). It shows the exact Workspace changes and the
program path, and applies them after the user accepts. No agent conversation is
required to begin recording. An empty notes folder is valid.

To finish setup—index, prove recall, add Sources, or change the program—use the
canonical Margins guide as the source of truth:

```bash
margins guide workspace-setup
```

Follow that guide end to end. It uses the same `workspace plan --preset
margins-meetings` and reviewed `apply` the panel uses, then `init` and an
exact-phrase recall proof. Running it on a Workspace the panel already set up
changes nothing in the program.

The setup conversation should leave the user with:

- the folder Margins may write approved notes into;
- the folders it learns from, and any other folders it may search but not change;
- where the Workspace program lives (`margins --workspace <id> workspace show`)
  and that `margins --workspace <id> workspace edit`, or **Workspace program**
  in the bb Meetings panel, changes it; and
- one real example showing that Margins can find an existing note.

Important boundaries:

- Do not start, stop, or inspect live meeting capture as part of setup.
- Do not transcribe a meeting, process a session, or draft a connected note.
- Do not create `.margins` inside the user's notes folder.
- Do not read credential bundles, auth files, token stores, shell history, or
  environment variable values.
- Do not run recall before `init` and `sync` have completed for the selected
  Workspace.
- Do not apply a workspace plan until the user has seen the plan; the setup
  request authorizes applying the preset plan unchanged.

When setup finishes, report the Workspace, its home and reference Sources, what
changed, the `init` and `sync` result, the recall proof, and where to edit the
program. Say what Margins can now remember in the user's own terms.
