import { experimental_defineHostEntry } from "@get-bb/plugin-sdk/host";
import { hostSignals, marginsHostContract, type HostOperationResult } from "./contracts.js";
import { createHttpMarginsLiveTransport, type MarginsLiveTransport } from "./live-transport.js";

interface MarginsHostEntryDeps {
  transport: MarginsLiveTransport;
}

async function emitChanged(
  context: {
    experimental_emitSignal(
      signal: "changed",
      payload: {
        reason:
          | "snapshot"
          | "ensure_runtime"
          | "start"
          | "pause"
          | "resume"
          | "stop"
          | "notepad";
      },
    ): Promise<void>;
  },
  reason:
    | "snapshot"
    | "ensure_runtime"
    | "start"
    | "pause"
    | "resume"
    | "stop"
    | "notepad",
  result: HostOperationResult,
) {
  if (result.ok) {
    await context.experimental_emitSignal("changed", { reason });
  }
}

export function createMarginsHostEntry({ transport }: MarginsHostEntryDeps) {
  return experimental_defineHostEntry({
    contract: marginsHostContract,
    experimental_signals: hostSignals,
    handlers: {
      async readSnapshot(input, context) {
        const result = await transport.readSnapshot({
          ...input,
          signal: context.signal,
        });
        return result;
      },
      async ensureRuntime(_input, context) {
        const result = await transport.ensureRuntime({
          dataDir: context.experimental_paths.dataDir,
          signal: context.signal,
        });
        await emitChanged(context, "ensure_runtime", result);
        return result;
      },
      async start(input, context) {
        const result = await transport.start({
          ...input,
          signal: context.signal,
        });
        await emitChanged(context, "start", result);
        return result;
      },
      async pause(input, context) {
        const result = await transport.pause({
          ...input,
          signal: context.signal,
        });
        await emitChanged(context, "pause", result);
        return result;
      },
      async resume(input, context) {
        const result = await transport.resume({
          ...input,
          signal: context.signal,
        });
        await emitChanged(context, "resume", result);
        return result;
      },
      async stop(input, context) {
        const result = await transport.stop({
          ...input,
          signal: context.signal,
        });
        await emitChanged(context, "stop", result);
        return result;
      },
      async updateNotepad(input, context) {
        const result = await transport.updateNotepad({
          ...input,
          signal: context.signal,
        });
        await emitChanged(context, "notepad", result);
        return result;
      },
    },
  });
}

export default createMarginsHostEntry({
  transport: createHttpMarginsLiveTransport(),
});
