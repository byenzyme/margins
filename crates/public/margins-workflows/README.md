# margins-workflows

`margins-workflows` owns Margins' portable application services: project
resolution and setup resources, Granola import and vault publishing, transcript
and memo alignment, transcript/recent/artifact views, artifact confinement and
pruning, and file transcription/session processing through the public
`AsrBackend` and `DiarizationBackend` ports.

The crate contains no device capture, Tauri, CPAL, CIDRE, desktop, or private
runtime dependency. Callers provide explicit project and `.margins` paths and
model backends. The transitional root crate exposes thin compatibility facades
so existing CLI and desktop behavior continues to use the same implementation
and on-disk schema.
