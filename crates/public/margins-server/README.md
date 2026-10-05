# margins-server

Loopback HTTP service for one declared Margins Workspace. It exposes the
WorkspaceService API and adapts BB browser capture to the meeting runtime.

The server has no desktop frontend or Tauri dependency. `MARGINS_WORKSPACE`
selects the Workspace; `MARGINS_HOME`, `MARGINS_DATA_DIR`, `MARGINS_HOST`, and
`MARGINS_PORT` configure the local service. The token is stored in the data
directory, and the selected Workspace remains the authority for sessions.
