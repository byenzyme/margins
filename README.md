# Margins

Margins is a local meeting notepad that's built to make sense of the stray words you just happened to write down. It helps those brush strokes paint the full picture, empowering agentic workflows with the context they need most.

And because it picks up the conversation's thread, it produces something that you'll actually find rewarding to read.

Most meeting summaries help humans keep record, but agents need something more batteries-included, otherwise they lose track of the context. Margins does one better than raw transcripts, using deep context to better notice why the other person pushed back on your ideas.

**So your agents can leverage the way that your knowledge base already compounds.**

---

## Getting started

**1. Install.**

    brew install byenzyme/margins/margins

Building from source: see [CONTRIBUTING.md](CONTRIBUTING.md).

**2. Download local models and copy the setup handoff.** Run this from the folder you want to use as the Margins base, then paste the printed prompt into your agent.

    margins setup

**3. Record, in the folder where your notes live.** Recordings go to the Workspace that covers that folder, or to your default Workspace; if there is none yet, `margins new` asks you to run `margins init` there first.

    cd ~/notes
    margins new

A recorder opens: a bordered pane titled `margins`, a running clock, your mic and the other side's audio both captured. Type into a memo pad with timestamped lines. Hit `^C` to stop the session.

**4. Turn it into a note.** `margins setup` installs the writing skill into Claude Code, Codex, or Cursor automatically (but you can customize its templates):

    /margins latest

It finishes transcribing the recording, lines your jottings up against what was actually said, and writes a Markdown note into `~/notes/` — reviewing it with you first.

---

## Compilation for your knowledge base

Margins is built on Enzyme, a local-first compile step for your knowledge base that's extremely token efficient (sublinear with corpus size) and designed around the way that it compounds.

It generates "catalysts" by first temporally sampling content in markdown (e.g. folders, a frontmatter field representing people, or around interleaved tags and wikilinks) or in SQLite tables (e.g. around columns that represent simliar). Then, it embeds both documents and catalysts and ranks the best content for each catalyst. Catalysts are a layer of indirection that lets even sparse agent queries to find deep connections.

Your Workspace is one editable program, `~/.margins/configs/<id>.enzyme`. Setup starts it from the Margins meetings preset; it looks like this:

```enzyme
workspace "notes" {
  source markdown "home" { path "/Users/me/notes" }

  learn questions from folder "Meetings"
    about operational
  learn questions from folder "People"
    including linked pages
    about relationships
  learn questions automatically

  leave out folders ["Templates", "Attachments"]

  remember in folder "Meetings" create note
}
```

Read it with `margins --workspace notes workspace show --text` and change it with `margins --workspace notes workspace edit`, which shows the change in plain language before applying it. `margins guide glossary` explains the words. Margins ships its own copy of the engine, separate from any `enzyme` you install; `margins enzyme --workspace notes status` runs it on Margins' data.

Using this, Enzyme then periodically refreshes its index with new content and directions.

---

## More ways to import

Point it at any audio you already have:

    margins transcribe memo.m4a --speakers 2

A voice memo, a call you recorded elsewhere, a debrief you talk through alone — it transcribes the same way and lands in the same folder. Already have a pile of meetings in Granola? Bring them in, so your first distilled note has a past to reach back to:

    margins import granola <export-file>

Margins converts each meeting into an ordinary note in your vault.

Look at the raw pieces of any recording:

    margins ls                 # your sessions
    margins recent             # recent meetings, as data
    margins transcript <id>    # the full transcript + your timed notes

---

## Margins is for people who like to take things apart

Customize how you want your agent to use Margins:

- Fork and version your meeting templates, whether it's a design reviews, a talk you're just listening to.
- Steer the distilled meeting artifact before you finalize it — ask the agent to draw out all sides of an architecture discussion, or analyze an exchange to give feedback, all captured into the note.
- Script against the Markdown and the local SQLite yourself; it's all on disk.
- Grow your personal, simple CRM - because every meeting names its people. See [docs/personal-crm.md](docs/personal-crm.md).

---

## License

Apache 2.0 — see [LICENSE](LICENSE)
