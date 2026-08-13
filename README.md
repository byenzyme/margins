# Margins

**Take notes during a meeting the way you already do — a few words when something matters. Margins turns those words, and the recording underneath them, into a clean note in your own folder.**

You know the moment. Someone says something and you think *that's the real problem* — so you type four words while they keep talking. Later the recording is an hour long and those four words are all you actually remember. Every other tool throws away the four words and keeps the hour.

Margins keeps the four words. They're the point.

---

## What it is

Margins is a meeting recorder you run from your terminal. Start it and a small notes pane opens. While you talk, you jot — half-sentences, reminders to yourself, the thing you don't want to lose. Each line is stamped to the second it happened, against the audio.

When the meeting ends, a note gets written into the folder you were standing in — the same folder as the rest of your notes. It's shaped by what *you* flagged, not by a generic summary. It's a plain Markdown file you own.

---

## The one line that is the whole product

Here's what you type mid-call — cryptic, fast, for no one but you:

    [14:32] same objection rui raised in march

Here's part of what comes back, written into `~/notes/` next to everything else:

    Leah's digest objection landed as recognition, not novelty. She described the
    digest freezing "a living thread into a snapshot," which then "becomes a
    second source of truth" until "two weeks later nobody knows which version
    carries the real decision." That is the same objection Rui raised in March —
    see [[meetings/2026-03-12 Atlas review with Rui|Atlas review with Rui]], where
    the concern was never day-one accuracy but a brief that ages into a competing
    authority while the project note changes underneath it.

You wrote six clipped words. You got back a paragraph that knows *which* objection you meant, and a link to the March note where Rui had already raised it. The call itself never named Rui or the earlier review; your clipped line did. The matching objection and note came from your folder. That's the trick: **the folder is the context.**

You wrote "Rui," so the note followed that thread. On day one, every connection traces to a word you typed — and it is enough. The first note you ever make is already useful, shaped by what you flagged that day.

Then the folder fills, and the links stop needing your words. The same objection comes back in a meeting phrased nothing like today's, and the note ties them together anyway. A worry you have circled three times — each time in different language — returns as one thread. A decision from a project you have not opened in weeks surfaces beside the thing that just reopened it. You never typed the link. The folder had enough history to catch it on its own.

There is nothing to turn on. An empty folder can only match the words you give it, so on day one you give it words. A folder with months of meetings starts finding what you left unsaid — the people you keep meeting, the projects that run for quarters, the threads your agents pick up and leave across the folder. The more it holds, the more it connects.

---

## A first session, start to finish

**1. Install.**

    brew install byenzyme/margins/margins

**2. Download local models (once).** This prepares transcription, speaker recognition, and the local catalyst model. Audio stays on your laptop.

    margins setup

**3. Record, in the folder where your notes live.** The first time you run `margins new` in a folder, that folder quietly becomes your notes home.

    cd ~/notes
    margins new --title "sync with priya"

A recorder opens: a bordered pane titled `margins`, a running clock, your mic and the other side's audio both captured. Type whenever you want to mark a moment. Press Enter to commit a line; its timestamp locks to that instant. `^S` saves, `^C` stops.

**4. Turn it into a note.** The writing step is a Claude Code plugin — it arrives as readable files from this same repo, not a service:

    # in Claude Code, once:
    /plugin marketplace add byenzyme/margins
    # then, per meeting:
    /margins sync-with-priya

It transcribes the recording, lines your jottings up against what was actually said, and writes a Markdown note into `~/notes/` — reviewing it with you first.

No account to record. No bot in your call. No dashboard. The writing step uses Claude Code or your configured AI key. A file appears next to your other files.

---

## What makes the note good

Your jottings aren't decoration — they're the priority signal. A line you typed is a moment you chose to keep, so the note leads with those. A line you went back and edited mid-meeting gets extra weight; you clearly weren't done with it. The rest of the transcript supports what you flagged instead of burying it.

The note keeps both sides of the room straight: what the other person actually proposed or pushed back on, versus what you were privately working out in your own notes. Those don't blur into one "the meeting covered…" summary.

And Margins reads the folder you already keep. If you have a page for a person or a project, new notes wire into it — that's how the note above reached Rui's March review, sitting in the same folder. You don't restructure anything; whatever shape your folder has is the shape new notes join.

---

## Not just meetings

Point it at any audio you already have:

    margins transcribe memo.m4a --speakers 2

A voice memo, a call you recorded elsewhere, a debrief you talk through alone — it transcribes the same way and lands in the same folder. Already have a pile of meetings in Granola? Bring them in, so your first distilled note has a past to reach back to:

    margins import granola <export-file>

Everything Margins makes is plain text on your disk. Look at the raw pieces of any recording:

    margins ls                 # your sessions
    margins recent             # recent meetings, as data
    margins transcript <id>    # the full transcript + your timed notes

---

## For people who like to take things apart

The last step is an agent reading your folder, so the workflow bends to you:

- Fork the note templates per kind of meeting — 1:1, design review, a talk you're just listening to.
- Talk to a session before you file it — ask the agent about the transcript, then let it write.
- Wire an ambient hook that distills each recording as it ends.
- Script against the Markdown and the local SQLite yourself; it's all on disk.
- Grow a personal CRM instead of buying one — every note already names its people, so an agent can roll up a page per person: what you last discussed, what you still owe them. See [docs/personal-crm.md](docs/personal-crm.md).

Margins gives you the parts. What you assemble on top is yours.

---

## Requirements

- macOS, for local on-device transcription.
- [Claude Code](https://claude.ai/code) for the note-writing step. Hosted recall search only turns on if you set an API key.
- Linux users: grab a binary from [GitHub Releases](../../releases).

Build from source with on-device transcription support:

    cargo install --locked --path crates/public/margins-cli --features coreml-asr

`margins setup` downloads local models for transcription, speaker recognition, and deeper recall. Override transcription model location or version with `MARGINS_FLUID_COREML_MODEL_DIR` and `MARGINS_FLUID_COREML_VERSION`.

---

## What's in this repository

This repo is the open, buildable core of Margins. What ships here is governed by a fail-closed allowlist — see [OPEN_SOURCE.md](OPEN_SOURCE.md) for exactly which crates are included and how the boundary is enforced. Recordings, transcripts, and notes stay on your machine; Margins is built so your sensitive personal data never has to leave it.

## License

Apache 2.0 — see [LICENSE](LICENSE). Inclusion here is not a claim that any crate has been published, and these terms do not grant trademark rights.
