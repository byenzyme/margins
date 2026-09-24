import { useCallback, useEffect, useRef, useState } from "react";
import { definePluginApp, useBbContext, useBbNavigate, useComposer, useRealtime, useRpc, type JsonValue } from "@get-bb/plugin-sdk/app";
import { AlertCircle, Check, Pause, Play, Square } from "lucide-react";
import type { marginsRpcContract } from "../server.js";
import { browserCaptureOwner, detectClientCapabilities } from "./browser-capture.js";
import { nativeBridgeOwner, type CaptureAuthority, type NativeStatus } from "./native-bridge-client.js";
import type { PanelState, WorkspaceMeeting } from "./contracts.js";

function paramsTitle(params: JsonValue | null) {
  return params && typeof params === "object" && !Array.isArray(params) && typeof params.title === "string" ? params.title : undefined;
}

function Signal({ state }: { state: PanelState["state"] }) {
  return <div className="margins-signal" data-state={state} aria-hidden="true">
    {[5, 9, 14, 8, 12, 6, 10].map((height, index) => <span key={index} style={{ height }} />)}
  </div>;
}

function RecordingOverlay() {
  const navigate = useBbNavigate();
  const context = useBbContext();
  const [threadId, setThreadId] = useState(() => browserCaptureOwner.threadId);
  const [state, setState] = useState<PanelState | null>(() => threadId ? browserCaptureOwner.panel(threadId) : null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [openOnArrival, setOpenOnArrival] = useState(false);
  const [nativeThreadId, setNativeThreadId] = useState(() => nativeBridgeOwner.threadId);
  const [nativeStatus, setNativeStatus] = useState(() => nativeBridgeOwner.status);
  const [nativeError, setNativeError] = useState(() => nativeBridgeOwner.connectionError);
  useEffect(() => {
    const unsubscribe = browserCaptureOwner.subscribe(() => {
      const owner = browserCaptureOwner.threadId;
      setThreadId(owner);
      setState(owner ? browserCaptureOwner.panel(owner) : null);
    });
    return () => { unsubscribe(); };
  }, []);
  useEffect(() => nativeBridgeOwner.subscribe(() => {
    setNativeThreadId(nativeBridgeOwner.threadId);
    setNativeStatus(nativeBridgeOwner.status);
    setNativeError(nativeBridgeOwner.connectionError);
  }), []);
  useEffect(() => {
    if (openOnArrival && (threadId || nativeThreadId) && context.threadId === (threadId || nativeThreadId)) {
      navigate.openThreadPanel({ actionId: "live" });
      setOpenOnArrival(false);
    }
  }, [context.threadId, navigate, openOnArrival, threadId, nativeThreadId]);
  async function nativeControl(operation: "pause" | "resume" | "stop") {
    setBusy(true); setError(null);
    try { await nativeBridgeOwner.control(operation); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusy(false); }
  }
  if (nativeThreadId && nativeStatus && ["getting_ready", "recording", "paused", "saving", "needs_attention"].includes(nativeStatus.state)) {
    const nativeBusy = ["getting_ready", "saving"].includes(nativeStatus.state);
    return <aside className="margins-overlay" role="status" aria-label="Margins Mac recording">
      <button className="margins-overlay-open" onClick={() => {
        if (context.threadId === nativeThreadId) navigate.openThreadPanel({ actionId: "live" });
        else { setOpenOnArrival(true); navigate.toThread(nativeThreadId); }
      }}><Signal state={nativeStatus.state === "paused" ? "paused" : nativeBusy ? "recovering" : "recording"} /><span>{nativeStatus.state === "paused" ? "Mac recording paused" : nativeStatus.state === "saving" ? "Saving Mac recording" : nativeStatus.state === "needs_attention" ? "Mac recording needs attention" : "Mac recording"}</span></button>
      {["recording", "paused"].includes(nativeStatus.state) && <>
        <button disabled={busy} onClick={() => void nativeControl(nativeStatus.state === "paused" ? "resume" : "pause")} aria-label={nativeStatus.state === "paused" ? "Resume Mac recording" : "Pause Mac recording"}>{nativeStatus.state === "paused" ? <Play size={13} /> : <Pause size={13} />}</button>
        <button disabled={busy} onClick={() => void nativeControl("stop")} aria-label="Stop and save Mac recording"><Square size={12} /></button>
      </>}
      {(error || nativeError) && <span className="margins-overlay-error">{error || nativeError}</span>}
    </aside>;
  }
  if (!threadId || !state) return null;
  const recording = state.state === "recording" && browserCaptureOwner.active && !browserCaptureOwner.recovering;
  const paused = state.state === "paused" && browserCaptureOwner.active && !browserCaptureOwner.recovering;
  const pending = ["recovering", "saving", "needs_attention"].includes(state.state) || browserCaptureOwner.recovering;
  if (!recording && !paused && !pending) return null;
  async function control(operation: "pause" | "resume" | "stop") {
    setBusy(true);
    setError(null);
    try {
      const next = await browserCaptureOwner[operation]();
      if (next && threadId) browserCaptureOwner.acceptPanel(threadId, next);
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusy(false); }
  }
  return <aside className="margins-overlay" role="status" aria-label="Margins recording">
    <button className="margins-overlay-open" onClick={() => {
      if (context.threadId === threadId) navigate.openThreadPanel({ actionId: "live" });
      else { setOpenOnArrival(true); navigate.toThread(threadId); }
    }}>
      <Signal state={paused ? "paused" : pending ? "recovering" : "recording"} />
      <span>{paused ? "Recording paused" : pending ? "Recording needs attention" : "Recording"}</span>
    </button>
    {(recording || paused) && <>
      <button onClick={() => void control(paused ? "resume" : "pause")} disabled={busy} aria-label={paused ? "Resume recording" : "Pause recording"}>{paused ? <Play size={13} /> : <Pause size={13} />}</button>
      <button onClick={() => void control("stop")} disabled={busy} aria-label="Stop and save recording"><Square size={12} /></button>
    </>}
    {error && <span className="margins-overlay-error">{error}</span>}
  </aside>;
}

function NativeCapturePanel({ threadId, title }: { threadId: string; title?: string }) {
  const rpc = useRpc<typeof marginsRpcContract>();
  const [status, setStatus] = useState<NativeStatus | null>(() => nativeBridgeOwner.status);
  const [pairedThread, setPairedThread] = useState(() => nativeBridgeOwner.threadId);
  const [connectionError, setConnectionError] = useState(() => nativeBridgeOwner.connectionError);
  const [code, setCode] = useState("");
  const [port, setPort] = useState("18765");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pinRetry, setPinRetry] = useState(0);
  const pinned = useRef<string | null>(null);
  useEffect(() => nativeBridgeOwner.subscribe(() => {
    setStatus(nativeBridgeOwner.status);
    setPairedThread(nativeBridgeOwner.threadId);
    setConnectionError(nativeBridgeOwner.connectionError);
  }), []);
  const pairedHere = pairedThread === threadId;
  const saved = pairedHere && status?.state === "saved" && status.sessionId;
  useEffect(() => {
    if (!saved || pinned.current === status.sessionId) return;
    let disposed = false;
    let succeeded = false;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
    pinned.current = status.sessionId;
    void rpc.call("pinNativeSession", {
      threadId, sessionId: status.sessionId!, instanceId: status.instanceId, workspaceId: status.workspaceId,
    }).then((result) => {
      if (disposed) return;
      if (result.ok) succeeded = true;
      else {
        pinned.current = null; setError(result.error.message);
        if (result.error.retryable) retryTimer = setTimeout(() => setPinRetry((value) => value + 1), 3_000);
      }
    }).catch((cause) => {
      if (disposed) return;
      pinned.current = null; setError(String(cause));
      retryTimer = setTimeout(() => setPinRetry((value) => value + 1), 3_000);
    });
    return () => {
      disposed = true;
      if (!succeeded && pinned.current === status.sessionId) pinned.current = null;
      if (retryTimer) clearTimeout(retryTimer);
    };
  }, [rpc, saved, status?.sessionId, threadId, pinRetry]);
  async function authority(): Promise<CaptureAuthority> {
    const result = await rpc.call("captureAuthority", { threadId });
    if (!result.ok) throw new Error(result.error.message);
    return result;
  }
  async function act(run: () => Promise<void>) {
    setBusy(true); setError(null);
    try { await run(); } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusy(false); }
  }
  return <div className="margins-native">
    <div className="margins-native-heading"><strong>Mac microphone + computer audio</strong><span>{pairedHere ? status?.state?.replaceAll("_", " ") || "Connecting" : "Connect Mac recorder"}</span></div>
    {!pairedHere && <>
      {pairedThread && <p>A Mac recording is paired to another bb thread. Open that thread to control it.</p>}
      {!pairedThread && <>
        <p>Run <code>margins native-bridge --remote &lt;Linux Margins URL or SSH alias&gt; --workspace &lt;Workspace ID&gt; --origin {window.location.origin} --port {port || "18765"}</code> on this Mac. Enter its pairing code here.</p>
        <div className="margins-native-pair"><input aria-label="Mac recorder port" type="number" min="1" max="65535" value={port} onChange={(event) => setPort(event.target.value)} /><input aria-label="Mac recorder pairing code" autoComplete="off" value={code} onChange={(event) => setCode(event.target.value)} placeholder="Pairing code" /><button disabled={busy || !code.trim()} onClick={() => void act(async () => { await nativeBridgeOwner.pair(threadId, code.trim(), await authority(), Number(port)); setCode(""); })}>Connect</button></div>
      </>}
    </>}
    {pairedHere && <>
      {status?.state === "ready" && <button disabled={busy} onClick={() => void act(async () => { if (browserCaptureOwner.active) throw new Error("Stop the microphone-only recording before starting Mac audio."); await nativeBridgeOwner.verify(await authority()); await nativeBridgeOwner.control("start", title); })}>Start Mac recording</button>}
      {status?.state === "recording" && <div className="margins-native-actions"><button disabled={busy} onClick={() => void act(() => nativeBridgeOwner.control("pause"))}>Pause</button><button disabled={busy} onClick={() => void act(() => nativeBridgeOwner.control("stop"))}>Stop and save</button></div>}
      {status?.state === "paused" && <div className="margins-native-actions"><button disabled={busy} onClick={() => void act(() => nativeBridgeOwner.control("resume"))}>Resume</button><button disabled={busy} onClick={() => void act(() => nativeBridgeOwner.control("stop"))}>Stop and save</button></div>}
      {status?.state === "getting_ready" && <p>Getting Mac audio and the Linux Workspace ready…</p>}
      {status?.state === "saving" && <p>Saving both audio lanes to Linux…</p>}
      {status?.state === "saved" && <p>Mac recording saved. The connected note action will appear in this thread once BB confirms the session.</p>}
      {status?.state === "needs_attention" && <p>{status.error || "Mac recording needs attention. The local transfer spool may still need delivery."}</p>}
      {connectionError && <p className="margins-error"><AlertCircle size={13} />Cannot reach the Mac recorder: {connectionError}</p>}
      {status && ["recording", "paused", "saving", "saved"].includes(status.state) && <p>Microphone samples: {status.microphoneSamples.toLocaleString()} · Computer audio samples: {status.systemSamples.toLocaleString()} · Computer audio frames: {status.systemFrames.toLocaleString()}</p>}
      {(!status || ["ready", "saved", "needs_attention"].includes(status.state)) && <button className="margins-quiet" disabled={busy} onClick={() => void act(async () => nativeBridgeOwner.forget())}>Disconnect Mac recorder</button>}
    </>}
    {error && <p className="margins-error"><AlertCircle size={13} />{error}</p>}
  </div>;
}

function WorkspaceMeetingNotes({ threadId, onMeetingChange }: { threadId: string; onMeetingChange: (active: boolean) => void }) {
  const rpc = useRpc<typeof marginsRpcContract>();
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [meeting, setMeeting] = useState<WorkspaceMeeting | null>(null);
  const [draft, setDraft] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const revision = useRef<string | null>(null);
  const dirty = useRef(false);
  const latestDraft = useRef("");

  const refresh = useCallback(async () => {
    const result = await rpc.call("readWorkspaceMeeting", { threadId, ...(sessionId ? { sessionId } : {}) });
    if (!result.ok) { setMessage(result.error.message); return; }
    if (!result.meeting) {
      onMeetingChange(result.candidates.length > 0);
      if (result.candidates.length > 1) setMessage("Several Workspace meetings are active. Finish one before joining from this panel.");
      return;
    }
    onMeetingChange(true);
    if (!dirty.current || result.meeting.sessionId !== sessionId) {
      setDraft(result.meeting.notepad.text);
      latestDraft.current = result.meeting.notepad.text;
      revision.current = result.meeting.notepad.revision;
      dirty.current = false;
    }
    setSessionId(result.meeting.sessionId);
    setMeeting(result.meeting);
  }, [onMeetingChange, rpc, sessionId, threadId]);

  useEffect(() => {
    void refresh().catch((error) => setMessage(String(error)));
    const timer = setInterval(() => void refresh().catch((error) => setMessage(String(error))), 3000);
    return () => clearInterval(timer);
  }, [refresh]);

  async function save() {
    if (!meeting || !dirty.current || !revision.current || saving) return;
    setSaving(true); setMessage(null);
    try {
      const result = await rpc.call("saveWorkspaceMemo", {
        threadId, sessionId: meeting.sessionId, expectedRevision: revision.current, text: draft,
      });
      if (!result.ok) throw new Error(result.error.message);
      if (!result.meeting) throw new Error("Meeting was unavailable after saving");
      revision.current = result.meeting.notepad.revision;
      dirty.current = latestDraft.current !== draft;
      setMeeting(result.meeting);
      setMessage("Note saved");
    } catch (error) { setMessage(error instanceof Error ? error.message : String(error)); }
    finally { setSaving(false); }
  }

  if (!meeting) return message ? <p className="margins-detail">{message}</p> : null;
  return <div className="margins-workspace-meeting">
    <p className="margins-detail">{meeting.inputFinalized ? "Saved meeting" : "Recording in Margins Menu"} · {meeting.title || meeting.sessionId}</p>
    <textarea className="margins-notepad" aria-label="Workspace meeting notepad"
      placeholder="Take notes while Margins records…" value={draft}
      onChange={(event) => { dirty.current = true; latestDraft.current = event.target.value; setDraft(event.target.value); setMessage(null); }}
      onBlur={() => void save()} />
    <button className="margins-quiet" onClick={() => void save()} disabled={!dirty.current || saving}>Save note</button>
    {message && <p className="margins-detail">{message}</p>}
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
  const [externalMeeting, setExternalMeeting] = useState(false);
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
    return next;
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
      if (nativeBridgeOwner.threadId && nativeBridgeOwner.status && ["getting_ready", "recording", "paused", "saving"].includes(nativeBridgeOwner.status.state)) {
        setMessage("Stop the Mac microphone + computer audio recording before starting a microphone-only recording.");
        return;
      }
      setState({ ...state, state: "getting_ready", title: "Getting recording ready", detail: "Waiting for microphone permission and this bb project." });
      await action("start", () => browserCaptureOwner.start(threadId, paramsTitle(params)));
    } else if (state.primaryAction === "pause") await action("pause", () => browserCaptureOwner.pause());
    else if (state.primaryAction === "resume") await action("resume", () => browserCaptureOwner.resume());
    else if (state.primaryAction === "retry") await action("retry", () => (
      browserCaptureOwner.hasPendingStop ? browserCaptureOwner.retryPendingStop() : refresh()
    ));
  }

  async function stop() { await action("stop", () => browserCaptureOwner.stop()); }
  async function connectedNote() {
    if (!state?.lastSessionId) return;
    await action("connected-note", async () => {
      const result = await rpc.call("connectedNoteContext", { threadId, sessionId: state.lastSessionId! });
      if (!result.ok) throw new Error(result.error.message);
      if (!result.context.transcript.available) {
        const requested = await rpc.call("transcribePinnedSession", { threadId, sessionId: state.lastSessionId! });
        if (!requested.ok) throw new Error(requested.error.message);
        setMessage(requested.status === "complete"
          ? "Transcription is ready. Make the connected note again."
          : "Transcribing this meeting on the project machine. Make the connected note when it finishes.");
        return;
      }
      const context = JSON.stringify(result.context);
      const request = `Create a connected note for the pinned Margins session below. Resolve transcript/artifact/recall data for this exact session before writing. Read and write note files only through the project's native filesystem Source; never proxy note bytes through Margins.\n\n${context}`;
      composer.updateText((current) => current.trim() ? `${current.trimEnd()}\n\n${request}` : request);
      composer.focus();
    });
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
    {client.platform === "macos" && <NativeCapturePanel threadId={threadId} title={paramsTitle(params)} />}
    {!state.ownsRecording && <WorkspaceMeetingNotes threadId={threadId} onMeetingChange={setExternalMeeting} />}
    {!externalMeeting && <>
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
    {state.lastSessionId && <div className="margins-saved-actions">
      <button className="margins-connected-note" onClick={() => void connectedNote()} disabled={busy !== null}>Make connected note</button>
      <button className="margins-quiet" onClick={() => void action("dismiss", refresh)}>Not now</button>
    </div>}
    {state.state === "needs_setup" && <p className="margins-seam">Recording with computer audio isn’t available in this browser yet.</p>}
    </>}
  </section>;
}

export default definePluginApp((app) => {
  app.contentScripts.register({ id: "recording-owner", mount: (context) => browserCaptureOwner.install(context) });
  app.slots.experimental_appOverlay({ id: "recording-status", component: RecordingOverlay });
  app.slots.threadPanelAction({
    id: "live", title: "Margins", icon: "Mic", layout: "flush",
    component: ({ threadId, params }) => <MarginsPanel threadId={threadId} params={params} />,
  });
});
