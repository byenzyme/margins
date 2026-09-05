import { defineRpcContract } from "@get-bb/plugin-sdk";
import { z } from "zod";

const jsonValueSchema: z.ZodType<
  null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue }
> = z.lazy(() =>
  z.union([
    z.null(),
    z.boolean(),
    z.number().finite(),
    z.string(),
    z.array(jsonValueSchema),
    z.record(z.string(), jsonValueSchema),
  ]),
);

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

export const PANEL_STATE_SCHEMA = "margins.bb.live.panel.v1";
export const WATERMARK_CONTEXT_SCHEMA = "margins.watermark.context.v1";

export const pluginStateSchema = z.enum([
  "host_offline",
  "unsupported_platform",
  "runtime_error",
  "runtime_auth_error",
  "microphone_permission",
  "system_audio_permission",
  "ready",
  "preparing",
  "recording",
  "paused",
  "finalizing",
  "meeting_saved",
  "recoverable_error",
]);

export const primaryActionSchema = z.enum([
  "start",
  "pause",
  "resume",
  "refresh",
  "none",
]);

export const desktopLiveStatusSchema = z.enum([
  "idle",
  "starting",
  "recording",
  "paused",
  "finalizing",
  "needs_attention",
]);

export const desktopTranscriptFreshnessSchema = z
  .object({
    decoded_until_ms: z.number().int().nonnegative(),
    committed_until_ms: z.number().int().nonnegative(),
    updated_at_unix_ms: z.number().int().nonnegative(),
    age_ms: z.number().int().nonnegative(),
  })
  .strict();

export const liveSessionRefSchema = z
  .object({
    session_id: z.string().min(1),
    status: desktopLiveStatusSchema,
    elapsed_ms: z.number().int().nonnegative(),
    generation: z.number().int().nonnegative(),
  })
  .strict();

export const liveHealthSchema = z
  .object({
    capture_phase: z.string().min(1),
    tap_status: z.string().min(1),
    tap_warning: z.string().nullable().optional(),
    system_audio_expected: z.boolean(),
    system_audio_observed: z.boolean(),
    transcript_freshness: desktopTranscriptFreshnessSchema.nullable().optional(),
  })
  .strict();

export const liveTranscriptSchema = z
  .object({
    at_ms: z.number().int().nonnegative().nullable().optional(),
    text: z.string(),
  })
  .strict();

export const liveMemoLineSchema = z
  .object({
    index: z.number().int().nonnegative(),
    at_ms: z.number().int().nonnegative().nullable().optional(),
    text: z.string(),
  })
  .strict();

export const desktopLiveEndpointsSchema = z
  .object({
    snapshot: z.string().startsWith("/"),
    start: z.string().startsWith("/"),
    pause: z.string().startsWith("/"),
    resume: z.string().startsWith("/"),
    stop: z.string().startsWith("/"),
    update_notepad: z.string().startsWith("/"),
  })
  .strict();

export const desktopLiveDiscoverySchema = z
  .object({
    protocol_version: z.literal(1),
    runtime: z.enum(["margins_desktop", "margins_cli"]),
    profile: z.string().min(1),
    pid: z.number().int().nonnegative(),
    base_url: z.string().url(),
    token: z.string().min(1),
    permissions: z
      .object({
        loopback_only: z.literal(true),
        private_file: z.literal(true),
      })
      .strict(),
    endpoints: desktopLiveEndpointsSchema,
    generated_at_unix_ms: z.number().int().nonnegative(),
  })
  .strict();

export const liveSnapshotSchema = z
  .object({
    protocol_version: z.literal(1),
    server_unix_ms: z.number().int().nonnegative(),
    session: liveSessionRefSchema.nullable(),
    health: liveHealthSchema,
    rolling_transcript: z.array(liveTranscriptSchema).max(80).default([]),
    memo_lines: z.array(liveMemoLineSchema).max(1000).default([]),
    notepad_revision: z.string().min(1),
  })
  .strict();

export const desktopLiveStartRequestSchema = z
  .object({
    operation_id: z.string().min(1),
    name: z.string().trim().min(1),
    project_id: z.string().min(1).optional(),
  })
  .strict();

export const desktopLiveSessionRequestSchema = z
  .object({
    operation_id: z.string().min(1),
    session_id: z.string().min(1),
    expected_generation: z.number().int().nonnegative().optional(),
  })
  .strict();

export const desktopLiveNotepadRequestSchema = desktopLiveSessionRequestSchema
  .extend({
    expected_notepad_revision: z.string().min(1),
    text: z.string(),
  })
  .strict();

export const liveErrorSchema = z
  .object({
    code: z.string().min(1),
    message: z.string().min(1),
    retryable: z.boolean(),
    state: pluginStateSchema.optional(),
    installUrl: z.string().url().optional(),
    details: z.record(z.string(), jsonValueSchema).optional(),
  })
  .strict();

export const hostReadSnapshotInputSchema = z
  .object({
    sessionId: z.string().min(1),
  })
  .strict();

export const hostStartInputSchema = z
  .object({
    operationId: z.string().min(1),
    name: z.string().trim().min(1).max(160),
    projectId: z.string().trim().min(1).optional(),
  })
  .strict();

export const hostSessionMutationInputSchema = z
  .object({
    operationId: z.string().min(1),
    sessionId: z.string().min(1),
    expectedGeneration: z.number().int().nonnegative().nullable(),
  })
  .strict();

export const hostUpdateNotepadInputSchema = hostSessionMutationInputSchema
  .extend({
    expectedNotepadRevision: z.string().min(1),
    text: z.string().max(100_000),
  })
  .strict();

export const hostOperationResultSchema = z.discriminatedUnion("ok", [
  z
    .object({
      ok: z.literal(true),
      snapshot: liveSnapshotSchema,
      idempotent_replay: z.boolean().optional(),
      stopped_session_id: z.string().nullable().optional(),
    })
    .strict(),
  z
    .object({
      ok: z.literal(false),
      error: liveErrorSchema,
    })
    .strict(),
]);

export const hostSignals = {
  changed: {
    payload: z
      .object({
        reason: z.enum([
          "snapshot",
          "ensure_runtime",
          "start",
          "pause",
          "resume",
          "stop",
          "notepad",
        ]),
      })
      .strict(),
  },
};

export const marginsHostContract = defineRpcContract({
  readSnapshot: {
    input: hostReadSnapshotInputSchema,
    output: hostOperationResultSchema,
  },
  ensureRuntime: {
    input: z.object({}).strict(),
    output: hostOperationResultSchema,
  },
  start: {
    input: hostStartInputSchema,
    output: hostOperationResultSchema,
  },
  pause: {
    input: hostSessionMutationInputSchema,
    output: hostOperationResultSchema,
  },
  resume: {
    input: hostSessionMutationInputSchema,
    output: hostOperationResultSchema,
  },
  stop: {
    input: hostSessionMutationInputSchema,
    output: hostOperationResultSchema,
  },
  updateNotepad: {
    input: hostUpdateNotepadInputSchema,
    output: hostOperationResultSchema,
  },
});

export const attachmentSchema = z
  .object({
    threadId: z.string().min(1),
    hostId: z.string().min(1),
    meetingId: z.string().min(1),
    attachedAtUnixMs: z.number().int().nonnegative(),
    detachedAtUnixMs: z.null(),
    generation: z.number().int().nonnegative().nullable(),
  })
  .strict();

export const savedMeetingSchema = z
  .object({
    threadId: z.string().min(1),
    hostId: z.string().min(1),
    meetingId: z.string().min(1),
    savedAtUnixMs: z.number().int().nonnegative(),
  })
  .strict();

export const panelStateSchema = z
  .object({
    schema: z.literal(PANEL_STATE_SCHEMA),
    threadId: z.string().min(1),
    state: pluginStateSchema,
    title: z.string().min(1),
    detail: z.string().min(1),
    primaryAction: primaryActionSchema,
    primaryLabel: z.string().min(1),
    canEditNotepad: z.boolean(),
    canStop: z.boolean(),
    canDetach: z.boolean(),
    firstUse: z.boolean(),
    preparationNeeded: z.boolean(),
    installUrl: z.string().url().nullable(),
    host: z
      .object({
        id: z.string().min(1),
        name: z.string().min(1),
        status: z.string().min(1),
      })
      .strict()
      .nullable(),
    attachment: attachmentSchema.nullable(),
    savedMeeting: savedMeetingSchema.nullable(),
    snapshot: liveSnapshotSchema.nullable(),
    error: liveErrorSchema.nullable(),
    mention: z
      .object({
        available: z.boolean(),
        itemId: z.string().nullable(),
      })
      .strict(),
  })
  .strict();

export const watermarkPacketSchema = z
  .object({
    schema: z.literal(WATERMARK_CONTEXT_SCHEMA),
    meeting: z
      .object({
        id: z.string().min(1),
        live: z.boolean(),
        status: desktopLiveStatusSchema,
      })
      .strict(),
    freshness: z
      .object({
        decodedUntilMs: z.number().int().nonnegative().nullable(),
        committedUntilMs: z.number().int().nonnegative().nullable(),
        transcriptUpdatedAtUnixMs: z.number().int().nonnegative().nullable(),
        resolvedAtUnixMs: z.number().int().nonnegative(),
        stale: z.boolean(),
        staleReason: z.string().nullable(),
      })
      .strict(),
    captureHealth: z
      .object({
        phase: z.string().min(1),
        paused: z.boolean(),
        tapStatus: z.string().min(1),
        systemAudioExpected: z.boolean(),
        systemAudioObserved: z.boolean(),
        warnings: z.array(z.string()).max(16),
      })
      .strict(),
    bounds: z
      .object({
        maxTranscriptChars: z.number().int().positive(),
        includedTranscriptChars: z.number().int().nonnegative(),
        memoItemsIncluded: z.number().int().nonnegative(),
      })
      .strict(),
    context: z.string(),
  })
  .strict();

const threadInputSchema = z.object({ threadId: z.string().min(1) }).strict();
const titleInputSchema = threadInputSchema
  .extend({ title: z.string().trim().max(160).optional() })
  .strict();
const notepadInputSchema = threadInputSchema
  .extend({
    expectedNotepadRevision: z.string().min(1),
    text: z.string().max(100_000),
  })
  .strict();

export const marginsRpcContract = defineRpcContract({
  getPanelState: {
    input: threadInputSchema,
    output: panelStateSchema,
  },
  startMeeting: {
    input: titleInputSchema,
    output: panelStateSchema,
  },
  pauseMeeting: {
    input: threadInputSchema,
    output: panelStateSchema,
  },
  resumeMeeting: {
    input: threadInputSchema,
    output: panelStateSchema,
  },
  stopMeeting: {
    input: threadInputSchema,
    output: panelStateSchema,
  },
  updateNotepad: {
    input: notepadInputSchema,
    output: panelStateSchema,
  },
  dismissSavedMeeting: {
    input: threadInputSchema,
    output: panelStateSchema,
  },
  detachThread: {
    input: threadInputSchema,
    output: panelStateSchema,
  },
});

export type PluginState = z.infer<typeof pluginStateSchema>;
export type PrimaryAction = z.infer<typeof primaryActionSchema>;
export type LiveError = z.infer<typeof liveErrorSchema>;
export type LiveSnapshot = z.infer<typeof liveSnapshotSchema>;
export type LiveMemoLine = z.infer<typeof liveMemoLineSchema>;
export type HostOperationResult = z.infer<typeof hostOperationResultSchema>;
export type ThreadAttachment = z.infer<typeof attachmentSchema>;
export type SavedMeeting = z.infer<typeof savedMeetingSchema>;
export type PanelState = z.infer<typeof panelStateSchema>;
export type WatermarkPacket = z.infer<typeof watermarkPacketSchema>;
