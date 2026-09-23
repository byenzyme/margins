import { defineRpcContract } from "@get-bb/plugin-sdk";
import { z } from "zod";

export const PANEL_STATE_SCHEMA = "margins.bb.recording.panel.v2";
// This is the existing hosted browser-capture protocol implemented by
// desktop/src-tauri/src/web_session.rs. Keep the cross-language contract test
// beside the plugin so a release cannot silently ship mismatched clients.
export const CAPTURE_PROTOCOL_VERSION = 2;
export const CAPTURE_DISCONNECT_GRACE_MS = 25_000;

export const clientCapabilitiesSchema = z.object({
  clientId: z.string().min(1),
  platform: z.enum(["macos", "mobile", "other"]),
  secureContext: z.boolean(),
  browserMicrophone: z.boolean(),
  nativeMacCapture: z.boolean(),
}).strict();
export const recordingStateSchema = z.enum([
  "needs_setup", "ready", "getting_ready", "recording", "paused",
  "recovering", "saving", "saved", "recording_elsewhere", "needs_attention", "unavailable",
]);
export const primaryActionSchema = z.enum(["start", "pause", "resume", "retry", "none"]);
export const projectTargetSchema = z.object({
  projectId: z.string().min(1), hostId: z.string().min(1), projectRoot: z.string().min(1),
}).strict();
export const notepadSchema = z.object({ text: z.string(), revision: z.string().min(1) }).strict();
export const hostCaptureSnapshotSchema = z.object({
  recordingId: z.string().min(1), status: z.enum(["recording", "paused", "saving"]),
  notepad: notepadSchema,
}).strict();
export const hostErrorSchema = z.object({
  code: z.string().min(1), message: z.string().min(1), retryable: z.boolean(),
}).strict();
export const hostResultSchema = z.discriminatedUnion("ok", [
  z.object({ ok: z.literal(true), snapshot: hostCaptureSnapshotSchema.nullable() }).strict(),
  z.object({ ok: z.literal(false), error: hostErrorSchema }).strict(),
]);
export const connectedNoteContextSchema = z.object({
  schema: z.literal("margins.bb.connected-note-context.v1"),
  instanceId: z.string().min(1),
  workspaceId: z.string().min(1),
  sessionId: z.string().min(1),
  title: z.string().nullable(),
  transcript: z.object({ available: z.boolean(), terminal: z.boolean(), live: z.boolean(), updatedAtUnixMs: z.number().int().nonnegative() }).strict(),
  memo: z.object({ revision: z.string().min(1), lineCount: z.number().int().nonnegative() }).strict(),
  artifacts: z.array(z.object({ artifactId: z.string().min(1), kind: z.string().min(1), retentionClass: z.string().min(1) }).strict()),
  noteAssociation: z.object({ sourceId: z.string().min(1), relativePath: z.string().min(1), revision: z.number().int().nonnegative() }).strict().nullable(),
  instructions: z.string().min(1),
}).strict();
export const connectedNoteResultSchema = z.discriminatedUnion("ok", [
  z.object({ ok: z.literal(true), context: connectedNoteContextSchema }).strict(),
  z.object({ ok: z.literal(false), error: hostErrorSchema }).strict(),
]);
export const transcriptionRequestResultSchema = z.discriminatedUnion("ok", [
  z.object({ ok: z.literal(true), status: z.enum(["queued", "running", "complete", "failed"]), attempt: z.number().int().positive() }).strict(),
  z.object({ ok: z.literal(false), error: hostErrorSchema }).strict(),
]);

const ownedCaptureInputSchema = z.object({ target: projectTargetSchema }).extend({
  recordingId: z.string().min(1), ownerId: z.string().min(1),
}).strict();
export const marginsHostContract = defineRpcContract({
  sessionExists: {
    input: z.object({ target: projectTargetSchema, recordingId: z.string().min(1) }).strict(),
    output: z.discriminatedUnion("ok", [
      z.object({ ok: z.literal(true), found: z.boolean() }).strict(),
      z.object({ ok: z.literal(false), error: hostErrorSchema }).strict(),
    ]),
  },
  captureAuthority: {
    input: z.object({ target: projectTargetSchema }).strict(),
    output: z.discriminatedUnion("ok", [
      z.object({ ok: z.literal(true), instanceId: z.string().min(1), workspaceId: z.string().min(1) }).strict(),
      z.object({ ok: z.literal(false), error: hostErrorSchema }).strict(),
    ]),
  },
  startBrowserCapture: {
    input: z.object({ target: projectTargetSchema, ownerId: z.string().min(1), name: z.string().min(1).max(160) }).strict(),
    output: hostResultSchema,
  },
  readCapture: { input: ownedCaptureInputSchema, output: hostResultSchema },
  heartbeat: { input: ownedCaptureInputSchema, output: hostResultSchema },
  pause: { input: ownedCaptureInputSchema, output: hostResultSchema },
  resume: { input: ownedCaptureInputSchema, output: hostResultSchema },
  stop: { input: ownedCaptureInputSchema, output: hostResultSchema },
  updateNotepad: {
    input: ownedCaptureInputSchema.extend({ expectedRevision: z.string().min(1), text: z.string().max(100_000) }).strict(),
    output: hostResultSchema,
  },
  uploadChunk: {
    input: ownedCaptureInputSchema.extend({ sequence: z.number().int().nonnegative(), bytesBase64: z.string() }).strict(),
    output: z.object({ ok: z.boolean(), error: hostErrorSchema.optional() }).strict(),
  },
  connectedNoteContext: {
    input: z.object({ target: projectTargetSchema, recordingId: z.string().min(1) }).strict(),
    output: connectedNoteResultSchema,
  },
  requestTranscription: {
    input: z.object({ target: projectTargetSchema, recordingId: z.string().min(1) }).strict(),
    output: transcriptionRequestResultSchema,
  },
});
export const hostSignals = {
  changed: { payload: z.object({
    projectId: z.string().min(1),
    reason: z.enum(["start", "pause", "resume", "stop", "notepad"]),
  }).strict() },
};

export const captureRecordSchema = z.object({
  projectId: z.string().min(1), hostId: z.string().min(1), projectRoot: z.string().min(1),
  recordingId: z.string().min(1), clientId: z.string().min(1), ownerId: z.string().min(1),
  lastHeartbeatUnixMs: z.number().int().nonnegative(),
});
export const panelStateSchema = z.object({
  schema: z.literal(PANEL_STATE_SCHEMA), state: recordingStateSchema,
  title: z.string().min(1), detail: z.string().min(1),
  sourceLabel: z.string().nullable(), storageLabel: z.string().nullable(),
  primaryAction: primaryActionSchema, primaryLabel: z.string().min(1),
  canStop: z.boolean(), canEditNotepad: z.boolean(), ownsRecording: z.boolean(),
  recordingId: z.string().nullable(), notepad: notepadSchema.nullable(),
  lastSessionId: z.string().nullable(),
  error: hostErrorSchema.nullable(),
}).strict();

const threadClientInputSchema = z.object({
  threadId: z.string().min(1), client: clientCapabilitiesSchema,
}).strict();
const captureClientInputSchema = threadClientInputSchema.extend({
  recordingId: z.string().min(1),
  operationId: z.string().min(1),
}).strict();
export const marginsRpcContract = defineRpcContract({
  captureAuthority: {
    input: z.object({ threadId: z.string().min(1) }).strict(),
    output: z.discriminatedUnion("ok", [
      z.object({ ok: z.literal(true), instanceId: z.string().min(1), workspaceId: z.string().min(1) }).strict(),
      z.object({ ok: z.literal(false), error: hostErrorSchema }).strict(),
    ]),
  },
  pinNativeSession: {
    input: z.object({ threadId: z.string().min(1), sessionId: z.string().min(1), instanceId: z.string().min(1), workspaceId: z.string().min(1) }).strict(),
    output: z.discriminatedUnion("ok", [
      z.object({ ok: z.literal(true) }).strict(),
      z.object({ ok: z.literal(false), error: hostErrorSchema }).strict(),
    ]),
  },
  getPanelState: { input: threadClientInputSchema, output: panelStateSchema },
  beginBrowserCapture: {
    input: threadClientInputSchema.extend({ ownerId: z.string().min(1), title: z.string().trim().max(160).optional() }).strict(),
    output: panelStateSchema,
  },
  heartbeat: { input: captureClientInputSchema, output: panelStateSchema },
  pause: { input: captureClientInputSchema, output: panelStateSchema },
  resume: { input: captureClientInputSchema, output: panelStateSchema },
  stop: { input: captureClientInputSchema, output: panelStateSchema },
  updateNotepad: {
    input: captureClientInputSchema.extend({ expectedRevision: z.string().min(1), text: z.string().max(100_000) }).strict(),
    output: panelStateSchema,
  },
  connectedNoteContext: {
    input: z.object({ threadId: z.string().min(1), sessionId: z.string().min(1) }).strict(),
    output: connectedNoteResultSchema,
  },
  transcribePinnedSession: {
    input: z.object({ threadId: z.string().min(1), sessionId: z.string().min(1) }).strict(),
    output: transcriptionRequestResultSchema,
  },
});

export type ClientCapabilities = z.infer<typeof clientCapabilitiesSchema>;
export type RecordingState = z.infer<typeof recordingStateSchema>;
export type PrimaryAction = z.infer<typeof primaryActionSchema>;
export type ProjectTarget = z.infer<typeof projectTargetSchema>;
export type HostError = z.infer<typeof hostErrorSchema>;
export type HostCaptureSnapshot = z.infer<typeof hostCaptureSnapshotSchema>;
export type HostResult = z.infer<typeof hostResultSchema>;
export type ConnectedNoteContext = z.infer<typeof connectedNoteContextSchema>;
export type ConnectedNoteResult = z.infer<typeof connectedNoteResultSchema>;
export type TranscriptionRequestResult = z.infer<typeof transcriptionRequestResultSchema>;
export type CaptureRecord = z.infer<typeof captureRecordSchema>;
export type PanelState = z.infer<typeof panelStateSchema>;
