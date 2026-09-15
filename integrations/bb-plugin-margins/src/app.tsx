import { useCallback, useEffect, useRef, useState } from "react";
import { definePluginApp, useComposer, useRealtime, useRpc, type JsonValue } from "@get-bb/plugin-sdk/app";
import { AlertCircle, Check, Pause, Play, Square } from "lucide-react";
import type { marginsRpcContract } from "../server.js";
import { browserCaptureOwner, detectClientCapabilities } from "./browser-capture.js";
import type { PanelState } from "./contracts.js";

function paramsTitle(params: JsonValue | null) {
  return params && typeof params === "object" && !Array.isArray(params) && typeof params.title === "string" ? params.title : undefined;
}

function Signal({ state }: { state: PanelState["state"] }) {
  return <div className="margins-signal" data-state={state} aria-hidden="true">
    {[5, 9, 14, 8, 12, 6, 10].map((height, index) => <span key={index} style={{ height }} />)}
  </div>;
}

function MarginsPanel({ threadId, params }: { threadId: string; params: JsonValue | null }) {
  const rpc = useRpc<typeof marginsRpcContract>();
  const composer = useComposer();
  const client = detectClientCapabilities();
  const [state, setState] = useState<PanelState | null>(() => browserCaptureOwner.panel(threadId));
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const desiredDraft = useRef("");
  const savedDraft = useRef("");
  const revision = useRef<string | null>(null);
  const memoOperation = useRef<string | null>(null);
  const saveLoop = useRef<Promise<void> | null>(null);

  const accept = useCallback((next: PanelState) => {
    setState(next);
    if (next.notepad && desiredDraft.current === savedDraft.current) {
      const pending = next.recordingId ? browserCaptureOwner.draft(next.recordingId) : undefined;
      desiredDraft.current = pending ?? next.notepad.text;
      savedDraft.current = next.notepad.text;
      revision.current = next.notepad.revision;
      setDraft(desiredDraft.current);
      if (pending === next.notepad.text && next.recordingId) browserCaptureOwner.clearDraft(next.recordingId);
    }
    browserCaptureOwner.acceptPanel(threadId, next);
  }, [threadId]);
  const refresh = useCallback(async () => {
    const next = await browserCaptureOwner.refresh(threadId, () => rpc.call("getPanelState", { threadId, client }));
    if (browserCaptureOwner.panel(threadId) === next) accept(next);
  }, [accept, client.clientId, client.platform, rpc, threadId]);

  useEffect(() => { void refresh().catch((error) => setMessage(String(error))); }, [refresh]);
  useRealtime("margins-recording", () => void refresh().catch(() => undefined));
  useEffect(() => {
    const unsubscribe = browserCaptureOwner.subscribe(() => {
      const owned = browserCaptureOwner.panel(threadId);
      if (owned) setState(owned);
    });
    return () => { unsubscribe(); };
  }, [refresh]);
  const saveNotepad = useCallback(async () => {
    if (saveLoop.current) return saveLoop.current;
    const loop = (async () => {
      while (desiredDraft.current !== savedDraft.current) {
        const current = state;
        const expectedRevision = revision.current;
        const text = desiredDraft.current;
        if (!current?.recordingId || !expectedRevision) return;
        memoOperation.current ??= crypto.randomUUID?.() || `${Date.now()}-${Math.random().toString(36).slice(2)}`;
        const next = await rpc.call("updateNotepad", {
          threadId, client, recordingId: current.recordingId,
          operationId: memoOperation.current, expectedRevision, text,
        });
        if (next.error) throw new Error(next.error.message);
        memoOperation.current = null;
        savedDraft.current = text;
        browserCaptureOwner.clearDraft(current.recordingId);
        revision.current = next.notepad?.revision || expectedRevision;
        setState(next);
      }
    })().finally(() => { saveLoop.current = null; });
    saveLoop.current = loop;
    return loop;
  }, [client.clientId, rpc, state, threadId]);

  function edit(value: string) {
    setDraft(value); desiredDraft.current = value; setMessage(null);
    if (state?.recordingId) browserCaptureOwner.retainDraft(state.recordingId, value);
    void saveNotepad().catch((error) => setMessage(error instanceof Error ? error.message : String(error)));
  }

  async function action(name: string, run: () => Promise<PanelState | void>) {
    setBusy(name); setMessage(null);
    try {
      const next = await run(); if (next) accept(next);
    } catch (error) { setMessage(error instanceof Error ? error.message : String(error)); }
    finally { setBusy(null); }
  }

  async function primary() {
    if (!state) return;
    if (state.primaryAction === "start") {
      setState({ ...state, state: "getting_ready", title: "Getting recording ready", detail: "Waiting for microphone permission and this bb project." });
      await action("start", () => browserCaptureOwner.start(threadId, paramsTitle(params)));
    } else if (state.primaryAction === "pause") await action("pause", () => browserCaptureOwner.pause());
    else if (state.primaryAction === "resume") await action("resume", () => browserCaptureOwner.resume());
    else if (state.primaryAction === "retry") await action("retry", refresh);
  }

  async function stop() { await action("stop", () => browserCaptureOwner.stop()); }
  function connectedNote() {
    const request = "Turn the Margins meeting I just recorded into a connected note.";
    composer.updateText((current) => current.trim() ? `${current.trimEnd()}\n\n${request}` : request);
    composer.focus();
  }

  if (!state) return <section className="margins-panel"><div className="margins-empty">Checking recording…</div></section>;
  const disconnectedLocal = state.ownsRecording && ["recording", "paused", "recovering"].includes(state.state)
    && browserCaptureOwner.recordingId === state.recordingId
    && (browserCaptureOwner.recovering || !browserCaptureOwner.active);
  const visibleState = disconnectedLocal ? "recovering" : state.state;
  const title = disconnectedLocal ? "Reconnecting" : state.title;
  const detail = disconnectedLocal ? "Audio and notes already received are safe. Margins will stop and save if this bb window does not reconnect shortly." : state.detail;
  const disabled = busy !== null || disconnectedLocal;

  return <section className="margins-panel" data-state={visibleState}>
    <header className="margins-header">
      <div className="margins-heading">
        <Signal state={visibleState} />
        <div><h3>{title}</h3>{state.sourceLabel && <p>{state.sourceLabel}</p>}</div>
      </div>
      <div className="margins-controls">
        {state.primaryAction !== "none" && <button className={`margins-control primary${state.primaryAction === "start" ? " margins-start" : ""}`} onClick={() => void primary()} disabled={disabled} aria-label={state.primaryLabel} title={state.primaryLabel}>
          {state.primaryAction === "pause" ? <Pause size={14} /> : <Play size={14} />}
          {state.primaryAction === "start" && <span>{state.primaryLabel}</span>}
        </button>}
        {state.canStop && <button className="margins-control" onClick={() => void stop()} disabled={disabled} aria-label="Stop and save" title="Stop and save"><Square size={12} /></button>}
      </div>
    </header>
    <p className="margins-detail">{detail}</p>
    {state.storageLabel && <p className="margins-storage"><Check size={12} /> {state.storageLabel}</p>}
    {message && <p className="margins-error"><AlertCircle size={13} />{message}</p>}
    {state.canEditNotepad && <textarea
      className="margins-notepad" aria-label="Meeting notepad" placeholder="Type notes as the conversation unfolds…"
      value={draft} onChange={(event) => edit(event.target.value)} onBlur={() => void saveNotepad()}
      disabled={busy === "pause" || busy === "stop"}
    />}
    {state.state === "saved" && <div className="margins-saved-actions">
      <button className="margins-connected-note" onClick={connectedNote}>Make connected note</button>
      <button className="margins-quiet" onClick={() => void action("dismiss", () => rpc.call("dismissSavedMeeting", { threadId, client }))}>Not now</button>
    </div>}
    {state.state === "needs_setup" && <p className="margins-seam">Recording with computer audio isn’t available in this browser yet.</p>}
  </section>;
}

export default definePluginApp((app) => {
  app.contentScripts.register({ id: "recording-owner", mount: (context) => browserCaptureOwner.install(context) });
  app.slots.threadPanelAction({
    id: "live", title: "Margins", icon: "Mic", layout: "flush",
    component: ({ threadId, params }) => <MarginsPanel threadId={threadId} params={params} />,
  });
});
