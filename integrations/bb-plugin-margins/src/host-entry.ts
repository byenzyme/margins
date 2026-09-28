import { experimental_defineHostEntry } from "@get-bb/plugin-sdk/host";
import { hostSignals, marginsHostContract, type HostResult } from "./contracts.js";
import { ProjectMarginsTransport, workspaceOptions } from "./project-server.js";

type Transport = ProjectMarginsTransport;

async function changed(
  context: { experimental_emitSignal(name: "changed", payload: { projectId: string; reason: "start" | "pause" | "resume" | "stop" | "notepad" }): Promise<void> },
  projectId: string,
  reason: "start" | "pause" | "resume" | "stop" | "notepad",
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
      workspaceOptions(_input, context) {
        retain(context);
        return workspaceOptions();
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
      sessionExists(input, context) {
        retain(context);
        return transport.sessionExists(input.target, context.experimental_paths.dataDir, input.recordingId);
      },
      captureAuthority(input, context) {
        retain(context);
        return transport.authority(input.target, context.experimental_paths.dataDir);
      },
      relayWorkspaceHttp(input, context) {
        retain(context);
        return transport.relayWorkspaceHttp(input.target, context.experimental_paths.dataDir, input);
      },
      async startBrowserCapture(input, context) {
        retain(context);
        const result = await transport.start(input.target, context.experimental_paths.dataDir, input.ownerId, input.name);
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
        const result = await transport.mutate(input.target, context.experimental_paths.dataDir, input.recordingId, input.ownerId, "pause_recording");
        await changed(context, input.target.projectId, "pause", result);
        return result;
      },
      async resume(input, context) {
        retain(context);
        const result = await transport.mutate(input.target, context.experimental_paths.dataDir, input.recordingId, input.ownerId, "resume_recording");
        await changed(context, input.target.projectId, "resume", result);
        return result;
      },
      async stop(input, context) {
        retain(context);
        const result = await transport.stop(input.target, context.experimental_paths.dataDir, input.recordingId, input.ownerId);
        await changed(context, input.target.projectId, "stop", result);
        return result;
      },
      async updateNotepad(input, context) {
        retain(context);
        const result = await transport.updateNotepad(input.target, context.experimental_paths.dataDir, input.recordingId, input.ownerId, input.expectedRevision, input.text);
        await changed(context, input.target.projectId, "notepad", result);
        return result;
      },
      uploadChunk(input, context) {
        retain(context);
        return transport.upload(input.target, context.experimental_paths.dataDir, input.recordingId, input.ownerId, input.sequence, input.bytesBase64);
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
