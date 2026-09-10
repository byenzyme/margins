# Expected diagnostic

## Import history (transcript dump evidence)
- This is primarily a transcript archive, not a curated meeting vault.
- `zoom-export-log.json` says `status: partial`, `incomplete: true`, and `transcript_imported_count: 4`.
- `import/zoom-manifest.csv` shows mixed naming and reliability:
  - `2026-06-17_q2-planning.txt` (ok)
  - `06-20 product review.txt` (speaker labels missing)
  - `06-20-product-review.vtt` (separate format of same meeting)
  - `06-20 product review (copy).txt` (reduplicated transcript)
  - `2026-06-22_customer_checkin.txt` (partial/truncated)
- VTT/TXT mismatch is visible in the workspace:
  - `.txt` files carry conversational timestamp prefixes (`00:00`, `0:00`, etc.).
  - `.vtt` uses WEBVTT blocks with frame-style timings.
- Cache state is partial and should be treated as incomplete import state only.

## Starting capture now
- Keep this usable for immediate meeting capture.
- Safe destination: `notes/meetings/<new-note>.md` (or `YYYY-MM-DD topic.md` equivalent).
- Start a new note today and defer transcript normalization until user asks.

## Explicitly avoid
- Do not convert every transcript into a meeting note automatically on first run.
- Do not require import completion before first capture.
- Do not invent project/people structure from filenames alone.

## Tone
- Use reassuring, non-blocking language: “You can start now and capture new meetings without restructuring this dump.”
