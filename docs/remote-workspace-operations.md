# Remote Workspace operations

`margins-server` is an explicitly provisioned, long-lived service. It binds only
to loopback. Use a configured SSH alias for discovery/tunneling, or a trusted TLS
reverse proxy for HTTPS. Never expose the loopback bearer token in process output,
URLs, shared Shortcut templates, or agent conversations.

## Provision

Set absolute paths and stable identities before starting the service:

```sh
MARGINS_DATA_DIR=/var/lib/margins/service \
MARGINS_HOME=/var/lib/margins/home \
MARGINS_WORK_DIR=/srv/margins/capture \
MARGINS_WORKSPACE=practice \
MARGINS_INSTANCE_ID=studio-host \
MARGINS_SERVICE_PROVISION=1 \
margins-server
```

Provisioning creates the named Workspace only when its notes and Captures bindings
exactly match. Later starts should omit `MARGINS_SERVICE_PROVISION`; a mismatched
mapping is rejected rather than inferred from the current directory. Without
`MARGINS_SERVICE_PROVISION`, a start naming a missing Workspace creates nothing and
fails with `workspace_not_found`; a bb launch (`MARGINS_BB_CAPTURE_WORKSPACE=1`)
instead stays up briefly to answer `/v1/capabilities` with that error, so the panel
can show it. The server
writes non-secret discovery metadata to `$MARGINS_DATA_DIR/service.json` and keeps
hashed scoped credentials in an owner-only file.

For SSH, `margins --remote ssh://<config-alias>` runs only the fixed discovery
command `margins service discover --json`, then opens a loopback forward with
host-key behavior inherited from OpenSSH configuration. Usernames, arbitrary SSH
options, commands, and URL paths are rejected. HTTPS credentials are supplied with
`MARGINS_REMOTE_TOKEN`; plaintext remote HTTP is rejected.

## Backup and recovery

Stop mutations or take a SQLite-consistent snapshot. A usable backup includes the
Workspace config, Captures `.margins/sessions.sqlite`, `.margins/artifacts/`,
`.margins/imports/`, `.margins/meeting-blobs/`, and any pending client transfer
directories. A durable server receipt is not an independent backup.

New native remote captures resample each enabled mono lane to 16 kHz off the audio
callback, encode 20 ms Opus frames at a 24 kbps target per lane, and fsync immutable
500 ms packet-stream commands before delivery (terminal blocks may be shorter).
A separate 16 kHz PCM recovery
journal is checkpointed at least every 100 ms while a segment is open so a client
crash can reconstruct a decodable terminal stream; it is removed only after the
compressed chunks and close intent are durable. Packet commands are aggregated
into bounded HTTP batches independently of that persistence cadence.

Client packet commands are removed only after the matching durable ACK receipt is
fsynced. Existing unacknowledged 16 kHz and 48 kHz raw-PCM transfer directories
remain readable and retryable without relabeling their format. Inspect and retry
all formats with:

```sh
margins transfers list --json
margins transfers retry <transfer-id>
```

Do not delete a pending transfer merely because the server is unavailable. Copy
the whole owner-only transfer directory for recovery. There is intentionally no
automatic background delivery daemon in this implementation.

The prior `meeting-runtime.sqlite` substrate was never selected by production at
the implementation base: it was instantiated only by temporary tests. Production
cutover therefore moves no user store. Existing canonical `sessions.sqlite` files
are opened in place and extended transactionally. If a non-production experiment
has a standalone `meeting-runtime.sqlite`, preserve it as evidence; this release
does not silently import or delete it and requires an explicit one-off conversion
review before using that directory as a service Workspace.

## Capability and failure policy

Clients negotiate protocol version, instance ID, Workspace ID, limits, capture
formats, operations, ASR, and recall once while establishing each connection. The
uploader uses that cached contract and fences every capture mutation with the
negotiated instance ID; it does not poll capabilities during capture. A Linux
receiver built without ASR or recall reports those capabilities as unavailable;
saving original audio remains valid. Unknown commands fail before local filesystem
changes.

Credentials are scoped to Workspace and operations. Shortcut credentials receive
only import and receipt operations. Revoke by principal using `service revoke`.
Rotate a full local token by stopping the service, preserving the credential file
for audit, and using the deployment's credential rotation procedure; do not edit
hashed records in place.

Release, signing, installed-app, native-device, real-phone, and provisioned-host
verification remain separate platform gates.
