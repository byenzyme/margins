import { experimental_defineHostEntry } from "@get-bb/plugin-sdk/host";
import { hostSignals, marginsHostContract, type HostResult } from "./contracts.js";
import { ProjectMarginsTransport, workspaceOptions, workspacePaths } from "./project-server.js";
import { applyWorkspaceSetup, previewWorkspaceSetup } from "./workspace-setup.js";

type Transport = ProjectMarginsTransport;

async function changed(
  context: { experimental_emitSignal(name: "changed", payload: { projectId: string; reason: "start" | "pause" | "resume" | "stop" }): Promise<void> },
  projectId: string,
  reason: "start" | "pause" | "resume" | "stop",
  result: HostResult,
) {
  if (result.ok) await context.experimental_emitSignal("changed", { projectId, reason });
}

export function createMarginsHostEntry(transport: Transport) {
  let workerLease: { dispose(): Promise<void> } | null = null;
  const retain = (context: { experimental_retainWorker(): { dispose(): Promise<void> } }) => {
    workerLease ??= context.experimental_retainWorker();
  };
  return experimental_defineHostEntry({
    contract: marginsHostContract,
    experimental_signals: hostSignals,
    handlers: {
      async workspaceOptions(_input, context) {
        retain(context);
        await transport.prepareCli(context.experimental_paths.dataDir);
        return workspaceOptions();
      },
      async workspacePaths(input, context) {
        retain(context);
        await transport.prepareCli(context.experimental_paths.dataDir);
        return workspacePaths(input.workspaceId);
      },
      async previewWorkspaceSetup(input, context) {
        retain(context);
        await transport.prepareCli(context.experimental_paths.dataDir);
        return previewWorkspaceSetup(input.target, context.experimental_paths.dataDir, input.homeRoot);
      },
      async applyWorkspaceSetup(input, context) {
        retain(context);
        await transport.prepareCli(context.experimental_paths.dataDir);
        return applyWorkspaceSetup(context.experimental_paths.dataDir, input.previewId);
      },
      listWorkspaceMeetings(input, context) {
        retain(context);
        return transport.listWorkspaceMeetings(input.target, context.experimental_paths.dataDir);
      },
      readWorkspaceMeeting(input, context) {
        retain(context);
        return transport.readWorkspaceMeeting(input.target, context.experimental_paths.dataDir, input.sessionId);
      },
      saveWorkspaceMemo(input, context) {
        retain(context);
        return transport.saveWorkspaceMemo(input.target, context.experimental_paths.dataDir,
          input.sessionId, input.expectedRevision, input.text);
      },
      renameWorkspaceMeeting(input, context) {
        retain(context);
        return transport.renameWorkspaceMeeting(input.target, context.experimental_paths.dataDir, input.sessionId, input.title);
      },
      discardWorkspaceMeeting(input, context) {
        retain(context);
        return transport.discardWorkspaceMeeting(input.target, context.experimental_paths.dataDir, input.sessionId);
      },
      readWorkspaceTranscript(input, context) {
        retain(context);
        return transport.readWorkspaceTranscript(input.target, context.experimental_paths.dataDir, input.sessionId);
      },
      noteDestination(input, context) {
        retain(context);
        return transport.noteDestination(input.target, context.experimental_paths.dataDir);
      },
      linkWorkspaceNote(input, context) {
        retain(context);
        return transport.linkWorkspaceNote(input.target, context.experimental_paths.dataDir, input);
      },
      sessionExists(input, context) {
        retain(context);
        return transport.sessionExists(input.target, context.experimental_paths.dataDir, input.recordingId);
      },
      captureAuthority(input, context) {
        retain(context);
        return transport.authority(input.target, context.experimental_paths.dataDir);
      },
      speechSetup(input, context) {
        retain(context);
        return transport.speechSetup(input.target, context.experimental_paths.dataDir);
      },
      retrySpeechSetup(input, context) {
        retain(context);
        return transport.speechSetup(input.target, context.experimental_paths.dataDir, true);
      },
      relayWorkspaceHttp(input, context) {
        retain(context);
        return transport.relayWorkspaceHttp(input.target, context.experimental_paths.dataDir, input);
      },
      async startBrowserCapture(input, context) {
        retain(context);
        const result = await transport.start(input.target, context.experimental_paths.dataDir, input.ownerId,
          input.name, input.startedAtUnixMs);
        await changed(context, input.target.projectId, "start", result);
        return result;
      },
      readCapture(input, context) {
        retain(context);
        return transport.read(input.target, context.experimental_paths.dataDir, input.recordingId, input.ownerId);
      },
      heartbeat(input, context) {
        retain(context);
        return transport.mutate(input.target, context.experimental_paths.dataDir, input.recordingId, input.ownerId, "heartbeat_web_recording");
      },
      async pause(input, context) {
        retain(context);
        const result = await transport.mutate(input.target, context.experimental_paths.dataDir, input.recordingId,
          input.ownerId, "pause_recording", input.expectedNextSequence, input.segmentEndedUnixMs, input.recoveredAfterReload);
        await changed(context, input.target.projectId, "pause", result);
        return result;
      },
      async resume(input, context) {
        retain(context);
        const result = await transport.mutate(input.target, context.experimental_paths.dataDir, input.recordingId,
          input.ownerId, "resume_recording", undefined, input.segmentStartedUnixMs);
        await changed(context, input.target.projectId, "resume", result);
        return result;
      },
      async stop(input, context) {
        retain(context);
        const result = await transport.stop(input.target, context.experimental_paths.dataDir, input.recordingId,
          input.ownerId, input.expectedNextSequence, input.segmentEndedUnixMs);
        await changed(context, input.target.projectId, "stop", result);
        return result;
      },
      async finishIncomplete(input, context) {
        retain(context);
        const result = await transport.finishIncomplete(input.target, context.experimental_paths.dataDir,
          input.recordingId, input.ownerId, input.expectedNextSequence);
        await changed(context, input.target.projectId, "stop", result);
        return result;
      },
      uploadChunk(input, context) {
        retain(context);
        return transport.upload(input.target, context.experimental_paths.dataDir, input.recordingId, input.ownerId,
          input.sequence, input.bytesBase64, input.capturedStartUnixMs, input.capturedEndUnixMs);
      },
      connectedNoteContext(input, context) {
        retain(context);
        return transport.connectedNoteContext(input.target, context.experimental_paths.dataDir, input.recordingId);
      },
      requestTranscription(input, context) {
        retain(context);
        return transport.requestTranscription(input.target, context.experimental_paths.dataDir, input.recordingId);
      },
    },
    async dispose() {
      await transport.dispose();
      await workerLease?.dispose();
      workerLease = null;
    },
  });
}

export default createMarginsHostEntry(new ProjectMarginsTransport());
