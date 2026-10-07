Margins 0.4.21 gives the recording screen a clearer start and a visible
recording state. The release archives contain `margins`, `margins-server`, and
the bundled `enzyme` 0.12.2.

- Starting a recording (`margins new`, bare `margins`, or `margins attach`)
  plays a short ignition sweep around the editor frame. A white-hot spark runs
  clockwise and cools through flame colors into the steady border. It plays
  once per session, and pausing, resuming, or switching microphones does not
  replay it.
- The frame title shows the recording state and session:
  `● rec · margins — <session>`, plus `(resumed)` after `margins attach`. The
  red dot fades gently while recording. Pausing shows `‖ paused`, and resuming
  flashes the dot. On narrow terminals the title keeps the recording indicator
  and drops the rest first.
- Set `MARGINS_NO_ANIMATION=1` to turn off the sweep and the fading dot, or
  `NO_COLOR=1` to turn off color as well.

Upgrade the CLI and the bb plugin (with its `margins-server`) together. The
plugin installs `margins`, `margins-server`, and `enzyme` from the 0.4.21
archive as one runtime.

Known limits: Linux live transcription is not available.
