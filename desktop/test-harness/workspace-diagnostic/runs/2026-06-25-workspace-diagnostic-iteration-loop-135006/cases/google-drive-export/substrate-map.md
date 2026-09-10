# google-drive-export substrate map

## 1 Substrate Pass
- Snapshot has 10 files: exported Drive folders, import manifest, cache-state.json, export log, notes/, and raw transcript/text leftovers.
- enzyme scan counted 5 Markdown files and entities folder:mydrive-shared, folder:notes, #atlas, #roadmap; it did not index JSON/log/txt import evidence.
- Evidence: google-drive-export.log reports duplicate filename and malformed heading warning; import/drive-manifest.json records duplicate_title: client-kickoff and empty_file: Untitled document.md; cache-state.json is partial with parse errors.
- Meeting material is split between MyDrive-Shared/Meetings, MyDrive-Shared/Projects/Atlas, and top-level notes/.

## Evidence implications
- Treat scan output as bounded evidence, not the full workspace.
- Preserve raw/import material as source boundary; do not treat it as finished meeting notes.
