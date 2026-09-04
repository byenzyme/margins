import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import {
  definePluginApp,
  useComposer,
  useRealtime,
  useRealtimeConnectionState,
  useRpc,
  type JsonValue,
} from "@get-bb/plugin-sdk/app";
import { AlertCircle, Check, Mic, Pause, Play, RefreshCw, Square } from "lucide-react";
import type { marginsRpcContract } from "../server.js";
import type { PanelState } from "./contracts.js";

const WAVEFORM_BARS = [
  4, 7, 10, 15, 9, 20, 13, 25, 17, 11, 22, 28, 18, 24, 14, 20, 10, 16, 8, 12,
  6, 9, 4,
];

function primaryIcon(action: PanelState["primaryAction"]) {
  if (action === "start" || action === "resume") {
    return <Play size={15} />;
  }
  if (action === "pause") return <Pause size={15} />;
  return <RefreshCw size={15} />;
}

function readParams(params: JsonValue | null): { title?: string } {
  if (typeof params !== "object" || params === null || Array.isArray(params)) {
    return {};
  }
  return typeof params.title === "string" ? { title: params.title } : {};
}

function Waveform({ state }: { state: PanelState["state"] }) {
  return (
    <div className="margins-waveform" data-state={state} aria-hidden="true">
      {WAVEFORM_BARS.map((height, index) => (
        <span
          key={`${height}-${index}`}
          style={
            {
              "--margins-wave-height": `${height}px`,
            } as CSSProperties
          }
        />
      ))}
    </div>
  );
}

function snapshotNotepad(state: PanelState | null) {
  return state?.snapshot?.memo_lines.map((line) => line.text).join("\n") ?? "";
}

type NotepadSaveState = "idle" | "dirty" | "saving" | "saved" | "conflict";
type BusyAction =
  | "start"
  | "pause"
  | "resume"
  | "stop"
  | "mention"
  | "dismiss"
  | "refresh"
  | null;

function MarginsPanel({ threadId, params }: { threadId: string; params: JsonValue | null }) {
  const rpc = useRpc<typeof marginsRpcContract>();
  const composer = useComposer();
  const connection = useRealtimeConnectionState();
  const [state, setState] = useState<PanelState | null>(null);
  const [notepad, setNotepad] = useState("");
  const [notepadSaveState, setNotepadSaveState] = useState<NotepadSaveState>("idle");
  const [busyAction, setBusyAction] = useState<BusyAction>(null);
  const [error, setError] = useState<string | null>(null);
  const sawInitialConnection = useRef(false);
  const notepadRef = useRef("");
  const notepadRevisionRef = useRef<string | null>(null);
  const notepadSessionRef = useRef<string | null>(null);
  const notepadDirtyRef = useRef(false);
  const notepadSavingRef = useRef(false);
  const notepadConflictRef = useRef(false);
  const savePromiseRef = useRef<Promise<boolean> | null>(null);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const stateRef = useRef<PanelState | null>(null);
  const initialTitle = readParams(params).title;

  function setPanelState(next: PanelState, forceNotepad = false) {
    stateRef.current = next;
    setState(next);
    const sessionId = next.snapshot?.session?.session_id ?? null;
    const sessionChanged = sessionId !== notepadSessionRef.current;
    if (forceNotepad || sessionChanged || (!notepadDirtyRef.current && !notepadSavingRef.current)) {
      const text = snapshotNotepad(next);
      notepadRef.current = text;
      notepadRevisionRef.current = next.snapshot?.notepad_revision ?? null;
      notepadSessionRef.current = sessionId;
      notepadDirtyRef.current = false;
      notepadConflictRef.current = false;
      setNotepad(text);
      setNotepadSaveState(text ? "saved" : "idle");
    }
  }

  const refresh = useCallback(async () => {
    const next = await rpc.call("getPanelState", { threadId });
    setPanelState(next);
    setError(null);
  }, [rpc, threadId]);

  useEffect(() => {
    void refresh().catch((cause) => setError(String(cause)));
  }, [refresh]);

  useRealtime("margins-live", () => {
    void refresh().catch((cause) => setError(String(cause)));
  });

  useEffect(() => {
    if (connection !== "connected") return;
    if (!sawInitialConnection.current) {
      sawInitialConnection.current = true;
      return;
    }
    void refresh().catch((cause) => setError(String(cause)));
  }, [connection, refresh]);

  useEffect(() => {
    if (
      state?.state !== "recording" &&
      state?.state !== "paused" &&
      state?.state !== "finalizing" &&
      state?.state !== "preparing"
    ) {
      return;
    }
    const timer = setInterval(() => {
      void refresh().catch((cause) => setError(String(cause)));
    }, 3000);
    return () => clearInterval(timer);
  }, [refresh, state?.state]);

  async function run(
    actionName: Exclude<BusyAction, null>,
    action: () => Promise<PanelState>,
    flush = false,
  ) {
    setBusyAction(actionName);
    setError(null);
    try {
      if (flush && !(await saveNotepad())) return;
      setPanelState(await action());
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusyAction(null);
    }
  }

  async function runPrimary() {
    if (!state) return;
    if (state.primaryAction === "start") {
      await run("start", () =>
        rpc.call("startMeeting", {
          threadId,
          ...(initialTitle ? { title: initialTitle } : {}),
        }),
      );
      return;
    }
    if (state.primaryAction === "pause") {
      await run("pause", () => rpc.call("pauseMeeting", { threadId }), true);
      return;
    }
    if (state.primaryAction === "resume") {
      await run("resume", () => rpc.call("resumeMeeting", { threadId }));
      return;
    }
    await run("refresh", () => rpc.call("getPanelState", { threadId }));
  }

  function clearSaveTimer() {
    if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
    saveTimerRef.current = null;
  }

  function scheduleNotepadSave(delay = 500) {
    clearSaveTimer();
    saveTimerRef.current = setTimeout(() => void saveNotepad(), delay);
  }

  async function saveNotepad(): Promise<boolean> {
    clearSaveTimer();
    if (savePromiseRef.current) {
      const saved = await savePromiseRef.current;
      return saved && notepadDirtyRef.current ? saveNotepad() : saved;
    }
    const currentState = stateRef.current;
    const revision = notepadRevisionRef.current;
    if (
      !currentState?.canEditNotepad ||
      !currentState.snapshot?.session ||
      !revision ||
      !notepadDirtyRef.current ||
      notepadConflictRef.current
    ) {
      return !notepadDirtyRef.current;
    }

    const submittedText = notepadRef.current;
    const operation = (async () => {
      notepadSavingRef.current = true;
      setNotepadSaveState("saving");
      try {
        const next = await rpc.call("updateNotepad", {
          threadId,
          expectedNotepadRevision: revision,
          text: submittedText,
        });
        if (next.error?.code === "notepad_changed") {
          stateRef.current = next;
          setState(next);
          notepadConflictRef.current = true;
          setNotepadSaveState("conflict");
          setError("This notepad changed elsewhere. Your draft is still here.");
          return false;
        }
        if (next.error) throw new Error(next.error.message);

        stateRef.current = next;
        setState(next);
        notepadRevisionRef.current = next.snapshot?.notepad_revision ?? revision;
        notepadSessionRef.current = next.snapshot?.session?.session_id ?? null;
        if (notepadRef.current === submittedText) {
          const savedText = snapshotNotepad(next);
          notepadRef.current = savedText;
          notepadDirtyRef.current = false;
          setNotepad(savedText);
          setNotepadSaveState(savedText ? "saved" : "idle");
        } else {
          setNotepadSaveState("dirty");
        }
        setError(null);
        return true;
      } catch (cause) {
        setNotepadSaveState("dirty");
        setError(cause instanceof Error ? cause.message : String(cause));
        return false;
      } finally {
        notepadSavingRef.current = false;
      }
    })();
    savePromiseRef.current = operation;
    const saved = await operation;
    savePromiseRef.current = null;
    if (saved && notepadDirtyRef.current) return saveNotepad();
    return saved;
  }

  useEffect(() => () => clearSaveTimer(), []);

  if (!state) {
    return (
      <div className="margins-panel margins-panel-center">
        <span className="margins-status-dot" />
        <span>Checking Margins</span>
      </div>
    );
  }

  const canAsk = state.mention.available && state.mention.itemId !== null;
  const showDetail =
    state.state !== "recording" &&
    state.state !== "paused" &&
    state.state !== "meeting_saved";
  const iconOnlyPrimary =
    state.primaryAction === "pause" || state.primaryAction === "resume";
  const displayedTitle =
    busyAction === "start"
      ? "Preparing recording"
      : busyAction === "stop"
        ? "Saving meeting"
        : state.title;
  const displayedDetail =
    busyAction === "start"
      ? "Preparing the private recorder on this Mac."
      : busyAction === "stop"
        ? "Your notes are saved. Margins is finishing the recording."
        : state.detail;
  const savedMeeting = state.savedMeeting;

  function addConnectedNoteRequest() {
    const request = "Turn the Margins meeting I just recorded into a connected note.";
    composer.updateText((current) => {
      const trimmed = current.trimEnd();
      return trimmed ? `${trimmed}\n\n${request}` : request;
    });
    composer.focus();
  }

  return (
    <div className="margins-panel">
      {error ? (
        <div className="margins-alert">
          <AlertCircle size={15} />
          <span>{error}</span>
        </div>
      ) : null}

      <section className="margins-recorder" aria-label="Recording controls">
        <div className="margins-recorder-row">
          <div className="margins-listening-state">
            <span className="margins-status-dot" data-state={busyAction ? "busy" : state.state} />
            <strong>{displayedTitle}</strong>
          </div>

          <Waveform state={busyAction === "stop" ? "finalizing" : state.state} />

          {state.state !== "meeting_saved" && state.primaryAction !== "none" ? <div className="margins-recorder-actions">
            <button
              type="button"
              className={
                iconOnlyPrimary
                  ? "margins-primary margins-icon-control"
                  : "margins-primary margins-labeled-control"
              }
              aria-label={busyAction ? displayedTitle : state.primaryLabel}
              title={busyAction ? displayedTitle : state.primaryLabel}
              disabled={busyAction !== null}
              onClick={() => void runPrimary()}
            >
              {primaryIcon(state.primaryAction)}
              {iconOnlyPrimary ? null : (
                <span>{busyAction ? displayedTitle : state.primaryLabel}</span>
              )}
            </button>
            {state.canStop ? (
              <button
                type="button"
                className="margins-secondary margins-icon-control margins-stop"
                aria-label="Stop recording"
                title="Stop recording"
                disabled={busyAction !== null}
                onClick={() =>
                  void run("stop", () => rpc.call("stopMeeting", { threadId }), true)
                }
              >
                <Square size={14} />
              </button>
            ) : null}
          </div> : null}
        </div>

        {showDetail || busyAction === "start" || busyAction === "stop" ? (
          <p className="margins-state-detail">{displayedDetail}</p>
        ) : null}
        {[
          "host_offline",
          "unsupported_platform",
          "runtime_error",
          "runtime_auth_error",
          "microphone_permission",
          "system_audio_permission",
          "recoverable_error",
        ].includes(state.state) ? (
          <span className="margins-needs-attention">Needs attention</span>
        ) : null}
        {state.state === "ready" && state.firstUse && busyAction === null ? (
          <div className="margins-privacy-note">
            <span>Audio and transcript stay on this Mac.</span>
            <span>Meeting context enters the thread only when you add @Margins.</span>
            {state.preparationNeeded ? (
              <span>First use prepares a private recorder on this Mac.</span>
            ) : null}
          </div>
        ) : null}
      </section>

      {state.state === "meeting_saved" ? (
        <section className="margins-saved" aria-label="Meeting saved">
          <div className="margins-saved-mark" aria-hidden="true">
            <Check size={18} />
          </div>
          <p>{state.detail}</p>
          <button type="button" className="margins-primary" onClick={addConnectedNoteRequest}>
            Make connected note
          </button>
          <button
            type="button"
            className="margins-quiet-button"
            disabled={busyAction !== null}
            onClick={() =>
              void run("dismiss", () => rpc.call("dismissSavedMeeting", { threadId }))
            }
          >
            Not now
          </button>
          <span className="margins-saved-hint">
            This adds a request to your message. You choose when to send it.
          </span>
        </section>
      ) : (
      <section className="margins-notepad" aria-label="Notepad">
        <div className="margins-notepad-heading">
          <strong>Notepad</strong>
          <div className="margins-save-state">
            <span aria-live="polite">
              {notepadSaveState === "saving"
                ? "Saving…"
                : notepadSaveState === "dirty"
                  ? "Not saved"
                  : notepadSaveState === "conflict"
                    ? "Changed elsewhere"
                    : notepadSaveState === "saved"
                      ? "Saved"
                      : ""}
            </span>
            {notepadSaveState === "conflict" && state.snapshot ? (
              <button
                type="button"
                onClick={() => {
                  setPanelState(state, true);
                  notepadConflictRef.current = false;
                  setError(null);
                }}
              >
                Reload
              </button>
            ) : null}
          </div>
        </div>

        <textarea
          className="margins-notepad-editor"
          value={notepad}
          disabled={!state.canEditNotepad}
          maxLength={100_000}
          placeholder={
            state.canEditNotepad
              ? "Jot down what matters…"
              : "Start recording to use the notepad"
          }
          onBlur={() => {
            if (notepadDirtyRef.current) scheduleNotepadSave(0);
          }}
          onChange={(event) => {
            const text = event.currentTarget.value;
            notepadRef.current = text;
            notepadDirtyRef.current = true;
            setNotepad(text);
            setNotepadSaveState("dirty");
            setError(null);
            scheduleNotepadSave();
          }}
        />
      </section>
      )}

      {state.state !== "meeting_saved" ? <footer className="margins-footer">
        <button
          type="button"
          className="margins-context-button"
          aria-label="Add @Margins to message"
          title="Add @Margins to message"
          disabled={!canAsk || busyAction !== null}
          onClick={() => {
            if (!state.mention.itemId) return;
            void (async () => {
              setBusyAction("mention");
              try {
                if (!(await saveNotepad())) return;
                composer.insertMention({
                  provider: "margins",
                  id: state.mention.itemId!,
                  label: "@Margins",
                });
                composer.focus();
              } finally {
                setBusyAction(null);
              }
            })();
          }}
        >
          <Mic size={15} />
          <span>Add @Margins</span>
        </button>
        <span>Bring this meeting into your next message.</span>
      </footer> : null}
    </div>
  );
}

export default definePluginApp((app) => {
  app.slots.threadPanelAction({
    id: "margins-live",
    title: "Margins live",
    icon: "Mic",
    component: MarginsPanel,
    layout: "flush",
  });
});
