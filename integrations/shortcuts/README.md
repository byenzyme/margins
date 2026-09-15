# Margins voice-memo intake Shortcut

This integration sends the original recording to one explicitly paired Margins
Workspace. The Shortcut does not receive note-file or general session access.

## Pair the phone

On the server host, provision and start `margins-server` for an explicit
`MARGINS_WORKSPACE`, then issue an upload-only credential:

```sh
margins --workspace <workspace-id> service pair-shortcut \
  --principal shortcut-<phone-name> --json
```

Copy the HTTPS service URL, Workspace ID, principal name, and returned credential
into private Shortcut variables. Do not put credentials in a shared Shortcut
template. The HTTPS endpoint must terminate TLS at a trusted reverse proxy and
forward only to the server's loopback listener.

## Shortcut actions

1. Accept Files and Media from the share sheet. If nothing was shared, use
   “Select File”. Keep the input as a file; do not transcode it.
2. Generate one UUID and retain it as `upload_id` for every retry of this file.
3. `POST` a multipart form to
   `https://<service>/v1/workspaces/<workspace-id>/imports` with:
   `upload_id`, optional `session_id`, optional `title`, and `file`.
4. Add the header `Authorization: Bearer <credential>`.
5. Treat the operation as saved only when HTTP is successful and the JSON body
   has `ok: true`, the expected `upload_id`, a `session_id`, digest, byte count,
   and durable `stored_path`. Display that receipt to the user.
6. On timeout, interruption, app switch, or a non-success response, retain the
   same UUID and retry. Repeating identical bytes returns the original receipt;
   reusing the UUID for different bytes returns a conflict.

The upload credential cannot list sessions, read artifacts, access Sources, or
publish notes. Revoke a lost phone independently:

```sh
margins --workspace <workspace-id> service revoke shortcut-<phone-name> --json
```

The API's retry behavior is covered by portable fixtures. Background execution,
phone lock, cellular transitions, and the share-sheet UX require the documented
real-iPhone verification gate; no portable test can establish those behaviors.
