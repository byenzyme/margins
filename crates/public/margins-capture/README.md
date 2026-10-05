# margins-capture

Public terminal and native capture adapters for Margins. This crate owns the
memo editor, TUI, recorder, and local speech orchestration. It uses
`margins-meeting-runtime` and `margins-store` for session authority and
`margins-media` for speech providers and model lookup. Native device capture is
an optional feature; the default build remains portable.
