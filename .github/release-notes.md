Margins 0.4.22 adds a downloadable Apple Silicon `Margins.app` for recording
from the Mac menu bar. The app includes its capture helper and is signed,
notarized, and stapled for installation.

The release also contains the `margins` CLI, `margins-server`, and bundled
`enzyme` archives for Apple Silicon macOS and x86-64 Linux. Upgrade the bb
plugin with the runtime: the plugin pins this release and installs the matching
server and CLI on the project host. Install `Margins.app` separately on the
recording Mac for microphone and computer audio.

Known limit: Linux live transcription is not available.
