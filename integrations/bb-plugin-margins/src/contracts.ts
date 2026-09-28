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
  workspaceId: z.string().min(1).optional(),
}).strict();
export const notepadSchema = z.object({ text: z.string(), revision: z.string().min(1) }).strict();
export const workspaceMeetingSchema = z.object({
  sessionId: z.string().min(1), title: z.string().nullable(),
  startedAt: z.string().min(1), inputFinalized: z.boolean(), notepad: notepadSchema,
}).strict();
export const hostCaptureSnapshotSchema = z.object({
  recordingId: z.string().min(1), sessionId: z.string().min(1), status: z.enum(["recording", "paused", "saving"]),
  notepad: notepadSchema,
}).strict();
export const hostErrorSchema = z.object({
  code: z.string().min(1), message: z.string().min(1), retryable: z.boolean(),
}).strict();
export const workspaceMeetingResultSchema = z.discriminatedUnion("ok", [
  z.object({ ok: z.literal(true), meeting: workspaceMeetingSchema.nullable(), candidates: z.array(z.string()) }).strict(),
  z.object({ ok: z.literal(false), error: hostErrorSchema }).strict(),
]);
export const workspaceMeetingSummarySchema = workspaceMeetingSchema.omit({ notepad: true }).extend({
  durationMs: z.number().nonnegative().nullable().default(null),
  audioSource: z.string().nullable().default(null),
  workspaceId: z.string().nullable().default(null),
  workspaceName: z.string().nullable().default(null),
  originProjectId: z.string().nullable().default(null),
  originProjectName: z.string().nullable().default(null),
  archived: z.boolean().default(false),
  notePath: z.string().nullable(),
  noteFile: z.object({ hostId: z.string().min(1), path: z.string().min(1) }).strict().nullable().default(null),
  threadIds: z.array(z.string()).default([]),
  threadLinks: z.array(z.object({ id: z.string(), title: z.string() }).strict()).default([]),
  distilledMemoRevision: z.string().nullable().default(null),
}).strict();
export const workspaceMeetingsResultSchema = z.discriminatedUnion("ok", [
  z.object({ ok: z.literal(true), meetings: z.array(workspaceMeetingSummarySchema) }).strict(),
  z.object({ ok: z.literal(false), error: hostErrorSchema }).strict(),
]);
export const workspaceMeetingActionResultSchema = z.discriminatedUnion("ok", [
  z.object({ ok: z.literal(true) }).strict(),
  z.object({ ok: z.literal(false), error: hostErrorSchema }).strict(),
]);
export const workspaceTranscriptResultSchema = z.discriminatedUnion("ok", [
  z.object({ ok: z.literal(true), body: z.string() }).strict(),
  z.object({ ok: z.literal(false), error: hostErrorSchema }).strict(),
]);
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
  workspaceOptions: {
    input: z.object({}).strict(),
    output: z.object({ defaultWorkspaceId: z.string().nullable(), autoSelected: z.boolean(),
      workspaces: z.array(z.object({ id: z.string(), name: z.string().nullable() }).strict()) }).strict(),
  },
  workspacePaths: {
    input: z.object({ workspaceId: z.string().min(1) }).strict(),
    output: z.object({ notes: z.string(), recordings: z.string() }).strict(),
  },
  previewWorkspaceSetup: {
    input: z.object({ target: projectTargetSchema, homeRoot: z.string(), noteFolder: z.string() }).strict(),
    output: z.object({ previewId: z.string(), workspaceId: z.string(), homeRoot: z.string(), destination: z.string(),
      mode: z.enum(["jev", "automatic_fallback", "empty"]), warning: z.string().nullable(), filesScanned: z.number().int(),
      selectedEntities: z.array(z.string()), actions: z.array(z.unknown()) }).strict(),
  },
  applyWorkspaceSetup: {
    input: z.object({ previewId: z.string() }).strict(),
    output: z.object({ workspaceId: z.string(), destination: z.string() }).strict(),
  },
  listWorkspaceMeetings: {
    input: z.object({ target: projectTargetSchema }).strict(),
    output: workspaceMeetingsResultSchema,
  },
  readWorkspaceMeeting: {
    input: z.object({ target: projectTargetSchema, sessionId: z.string().min(1).optional() }).strict(),
    output: workspaceMeetingResultSchema,
  },
  saveWorkspaceMemo: {
    input: z.object({ target: projectTargetSchema, sessionId: z.string().min(1),
      expectedRevision: z.string().min(1), text: z.string().max(100_000),
    }).strict(),
    output: workspaceMeetingResultSchema,
  },
  renameWorkspaceMeeting: {
    input: z.object({ target: projectTargetSchema, sessionId: z.string().min(1), title: z.string().trim().min(1).max(160) }).strict(),
    output: workspaceMeetingActionResultSchema,
  },
  discardWorkspaceMeeting: {
    input: z.object({ target: projectTargetSchema, sessionId: z.string().min(1) }).strict(),
    output: workspaceMeetingActionResultSchema,
  },
  readWorkspaceTranscript: {
    input: z.object({ target: projectTargetSchema, sessionId: z.string().min(1) }).strict(),
    output: workspaceTranscriptResultSchema,
  },
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
  relayWorkspaceHttp: {
    input: z.object({ target: projectTargetSchema, method: z.enum(["GET", "POST", "PUT"]), path: z.string().min(1).max(300),
      bodyBase64: z.string().max(2_000_000), contentType: z.string().max(120).optional(),
      producerToken: z.string().max(300).optional(), instanceId: z.string().max(300).optional() }).strict(),
    output: z.object({ status: z.number().int().min(100).max(599), bodyBase64: z.string().max(4_000_000) }).strict(),
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
  workspaceId: z.string().min(1),
  sessionId: z.string().min(1), recordingId: z.string().min(1), clientId: z.string().min(1), ownerId: z.string().min(1),
  lastHeartbeatUnixMs: z.number().int().nonnegative(),
});
export const panelStateSchema = z.object({
  schema: z.literal(PANEL_STATE_SCHEMA), state: recordingStateSchema,
  title: z.string().min(1), detail: z.string().min(1),
  sourceLabel: z.string().nullable(), storageLabel: z.string().nullable(),
  primaryAction: primaryActionSchema, primaryLabel: z.string().min(1),
  canStop: z.boolean(), canEditNotepad: z.boolean(), ownsRecording: z.boolean(),
  recordingId: z.string().nullable(), sessionId: z.string().nullable(), notepad: notepadSchema.nullable(),
  lastSessionId: z.string().nullable(),
  error: hostErrorSchema.nullable(),
}).strict();

const threadClientInputSchema = z.object({
  threadId: z.string().min(1), client: clientCapabilitiesSchema,
}).strict();
const captureClientInputSchema = z.object({
  sessionId: z.string().min(1), client: clientCapabilitiesSchema,
  operationId: z.string().min(1),
}).strict();
export const marginsRpcContract = defineRpcContract({
  availableWorkspaces: {
    input: z.object({ projectId: z.string().min(1) }).strict(),
    output: z.object({ defaultWorkspaceId: z.string().nullable(), resolvedWorkspaceId: z.string().nullable(), autoSelected: z.boolean(),
      workspaces: z.array(z.object({ id: z.string(), name: z.string().nullable() }).strict()) }).strict(),
  },
  workspacePaths: {
    input: z.object({ projectId: z.string().min(1) }).strict(),
    output: z.object({ workspaceId: z.string().nullable(), notes: z.string().nullable(), recordings: z.string().nullable() }).strict(),
  },
  previewWorkspaceSetup: {
    input: z.object({ projectId: z.string().min(1), homeRoot: z.string(), noteFolder: z.string() }).strict(),
    output: z.object({ previewId: z.string(), workspaceId: z.string(), homeRoot: z.string(), destination: z.string(),
      mode: z.enum(["jev", "automatic_fallback", "empty"]), warning: z.string().nullable(), filesScanned: z.number().int(),
      selectedEntities: z.array(z.string()), actions: z.array(z.unknown()) }).strict(),
  },
  applyWorkspaceSetup: {
    input: z.object({ projectId: z.string().min(1), previewId: z.string() }).strict(),
    output: z.object({ workspaceId: z.string(), destination: z.string() }).strict(),
  },
  availableProjects: {
    input: z.object({}).strict(),
    output: z.object({ projects: z.array(z.object({ id: z.string(), name: z.string() }).strict()) }).strict(),
  },
  getProjectPanelState: {
    input: z.object({ projectId: z.string().min(1), client: clientCapabilitiesSchema }).strict(),
    output: panelStateSchema,
  },
  beginProjectCapture: {
    input: z.object({ projectId: z.string().min(1), client: clientCapabilitiesSchema,
      ownerId: z.string().min(1), title: z.string().trim().max(160).optional() }).strict(),
    output: panelStateSchema,
  },
  listWorkspaceMeetings: {
    input: z.object({ projectId: z.string().min(1) }).strict(), output: workspaceMeetingsResultSchema,
  },
  projectWorkspace: {
    input: z.object({ threadId: z.string().min(1).optional(), projectId: z.string().min(1).optional(), workspaceId: z.string().optional() }).strict(),
    output: z.object({ workspaceId: z.string().nullable() }).strict(),
  },
  readWorkspaceMeeting: {
    input: z.object({ threadId: z.string().min(1).optional(), projectId: z.string().min(1).optional(), sessionId: z.string().min(1).optional() }).strict(),
    output: workspaceMeetingResultSchema,
  },
  saveWorkspaceMemo: {
    input: z.object({ threadId: z.string().min(1).optional(), projectId: z.string().min(1).optional(), sessionId: z.string().min(1),
      expectedRevision: z.string().min(1), text: z.string().max(100_000),
    }).strict(), output: workspaceMeetingResultSchema,
  },
  renameWorkspaceMeeting: {
    input: z.object({ projectId: z.string().min(1), sessionId: z.string().min(1), title: z.string().trim().min(1).max(160) }).strict(),
    output: workspaceMeetingActionResultSchema,
  },
  archiveWorkspaceMeeting: {
    input: z.object({ projectId: z.string().min(1), sessionId: z.string().min(1), archived: z.boolean() }).strict(),
    output: workspaceMeetingActionResultSchema,
  },
  recordMeetingOrigin: {
    input: z.object({ projectId: z.string().min(1), sessionId: z.string().min(1) }).strict(),
    output: workspaceMeetingActionResultSchema,
  },
  discardWorkspaceMeeting: {
    input: z.object({ projectId: z.string().min(1), sessionId: z.string().min(1) }).strict(),
    output: workspaceMeetingActionResultSchema,
  },
  readWorkspaceTranscript: {
    input: z.object({ projectId: z.string().min(1), sessionId: z.string().min(1) }).strict(),
    output: workspaceTranscriptResultSchema,
  },
  captureAuthority: {
    input: z.object({ threadId: z.string().min(1).optional(), projectId: z.string().min(1).optional() }).strict(),
    output: z.discriminatedUnion("ok", [
      z.object({ ok: z.literal(true), instanceId: z.string().min(1), workspaceId: z.string().min(1) }).strict(),
      z.object({ ok: z.literal(false), error: hostErrorSchema }).strict(),
    ]),
  },
  issueMenuGrant: {
    input: z.object({ projectId: z.string().min(1), origin: z.string().url().max(300) }).strict(),
    output: z.object({ serviceUrl: z.string().url(), token: z.string().min(32), workspaceId: z.string().min(1),
      workspaceName: z.string().min(1), instanceId: z.string().min(1), expiresAt: z.number().int() }).strict(),
  },
  revokeMenuGrants: {
    input: z.object({ projectId: z.string().min(1) }).strict(),
    output: z.object({ revoked: z.boolean() }).strict(),
  },
  pinNativeSession: {
    input: z.object({ threadId: z.string().min(1).optional(), projectId: z.string().min(1).optional(), sessionId: z.string().min(1), instanceId: z.string().min(1), workspaceId: z.string().min(1) }).strict(),
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
    input: z.object({ threadId: z.string().min(1).optional(), projectId: z.string().min(1).optional(), sessionId: z.string().min(1) }).strict(),
    output: connectedNoteResultSchema,
  },
  transcribePinnedSession: {
    input: z.object({ threadId: z.string().min(1).optional(), projectId: z.string().min(1).optional(), sessionId: z.string().min(1) }).strict(),
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
export type WorkspaceMeeting = z.infer<typeof workspaceMeetingSchema>;
export type WorkspaceMeetingSummary = z.infer<typeof workspaceMeetingSummarySchema>;
