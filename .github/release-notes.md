Margins 0.4.16 makes recording start and stop faster and more reliable.
The release archives contain `margins` and `margins-server`.

- Margins Menu (BB mode) opens your microphone before the meeting session is
  ready and keeps what you say while it starts, so the first words are no
  longer lost. The menu says **Starting…** until capture is live. Pause and
  Finish work during that window; Finish turns the microphone off right away.
  If bb cannot be reached, the audio already captured is kept in
  `~/.margins/unsent` and is not discarded.
- Finishing a Menu recording no longer waits for live transcription to wind
  down, so Stop reaches *saved* in about a second. The server then starts the
  final transcript automatically. Before this fix, a Menu recording could stay
  untranscribed until the next job came in.
- The bb recording timer for Menu recordings now keeps real time. It used
  to run about three times too fast with 48 kHz microphones. The *Saved*
  label shows the length of the saved recording.
- When live transcription finishes warming up, it catches up on the audio
  recorded so far. It no longer starts partway into the meeting.
- BB browser recordings keep audio the server has not yet acknowledged in the
  browser's storage. A page reload no longer loses those chunks.
- `margins --version` reports the version, commit, and build kind.
  `margins setup --only` no longer fails when no catalyst is configured.

Known limits: Linux live transcription is not available.
