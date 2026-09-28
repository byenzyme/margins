import { useCallback, useEffect, useRef, useState } from "react";
import { definePluginApp, useBbContext, useBbNavigate, useComposer, useRealtime, useRpc, type JsonValue } from "@get-bb/plugin-sdk/app";
import { AlertCircle, Check, Pause, Play, Square } from "lucide-react";
import type { marginsRpcContract } from "../server.js";
import { browserCaptureOwner, detectClientCapabilities } from "./browser-capture.js";
import { nativeBridgeOwner, type CaptureAuthority, type NativeStatus } from "./native-bridge-client.js";
import type { PanelState, WorkspaceMeeting } from "./contracts.js";
import { MeetingLevelDot, MeetingsAccessory, MeetingsPage, rememberStopAck } from "./meetings-page.js";
import { MarginsThreadTab } from "./thread-tab.js";

function paramsTitle(params: JsonValue | null) {
  return params && typeof params === "object" && !Array.isArray(params) && typeof params.title === "string" ? params.title : undefined;
}

function Signal({ state }: { state: PanelState["state"] }) {
  return <div className="margins-signal" data-state={state} aria-hidden="true">
    {[5, 9, 14, 8, 12, 6, 10].map((height, index) => <span key={index} style={{ height }} />)}
  </div>;
}

function ProjectWorkspaceSetting({ projectId, onSaved }: { projectId: string; onSaved: () => void }) {
  const rpc = useRpc<typeof marginsRpcContract>();
  const [value, setValue] = useState("");
  const [workspaces, setWorkspaces] = useState<Array<{ id: string; name: string | null }>>([]);
  const [defaultWorkspaceId, setDefaultWorkspaceId] = useState<string | null>(null);
  const [paths, setPaths] = useState<{ notes: string | null; recordings: string | null } | null>(null);
  const [pathsError, setPathsError] = useState(false);
  const [pathsVersion, setPathsVersion] = useState(0);
  const [message, setMessage] = useState("");
  useEffect(() => {
    void Promise.all([rpc.call("projectWorkspace", { projectId }), rpc.call("availableWorkspaces", { projectId })])
      .then(([selected, options]) => {
        setValue(selected.workspaceId || ""); setWorkspaces(options.workspaces);
        setDefaultWorkspaceId(options.defaultWorkspaceId);
        if (options.autoSelected) setMessage(`Using ${options.workspaces[0]?.name || options.workspaces[0]?.id} as the machine default.`);
      }).catch(() => setMessage("Workspace setting unavailable"));
  }, [rpc, projectId]);
  useEffect(() => {
    setPaths(null); setPathsError(false);
    void rpc.call("workspacePaths", { projectId })
      .then(({ notes, recordings }) => setPaths({ notes, recordings }))
      .catch(() => setPathsError(true));
  }, [rpc, projectId, pathsVersion]);
  return <div className="margins-workspace-setting">
    <label htmlFor="margins-project-workspace">Margins Workspace for this project</label>
    <select id="margins-project-workspace" aria-label="Margins Workspace for this project" value={value}
      onChange={(event) => setValue(event.target.value)}>
      <option value="">Machine default ({workspaces.find((item) => item.id === defaultWorkspaceId)?.name || defaultWorkspaceId || "not set"})</option>
      {workspaces.map((item) => <option key={item.id} value={item.id}>{item.name || item.id}</option>)}
    </select>
    <button onClick={() => void rpc.call("projectWorkspace", { projectId, workspaceId: value }).then(() => { setMessage("Workspace preference saved"); setPathsVersion((version) => version + 1); onSaved(); }).catch((error) => setMessage(String(error)))}>Save</button>
    {message && <span role="status">{message}</span>}
    {paths && <div className="margins-workspace-paths">
      {paths.recordings && <div>Meeting recordings <code>{paths.recordings}</code></div>}
      {paths.notes && <div>Connected notes <code>{paths.notes}</code></div>}
      <small>These locations come from the selected Margins Workspace.</small>
    </div>}
    {pathsError && <span role="status">Workspace locations unavailable</span>}
  </div>;
}

function MenuAccessSetting({ projectId }: { projectId: string }) {
  const rpc = useRpc<typeof marginsRpcContract>();
  const [message, setMessage] = useState("");
  return <div className="margins-workspace-setting">
    <span>Margins Menu access to this Workspace</span>
    <button onClick={() => void rpc.call("revokeMenuGrants", { projectId })
      .then(() => setMessage("Mac capture access revoked. Reconnect from Meetings to record again."))
      .catch((error) => setMessage(String(error)))}>Revoke access</button>
    {message && <span role="status">{message}</span>}
  </div>;
}

function RecordingOverlay() {
  const navigate = useBbNavigate();
  const context = useBbContext();
  const [tick, setTick] = useState(0);
  const [busy, setBusy] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [failure, setFailure] = useState(false);
  useEffect(() => {
    const unsubscribe = browserCaptureOwner.subscribe(() => setTick((value) => value + 1));
    return () => { unsubscribe(); };
  }, []);
  useEffect(() => nativeBridgeOwner.subscribe(() => setTick((value) => value + 1)), []);
  useEffect(() => {
    const timer = setInterval(() => setTick((value) => value + 1), 1_000);
    return () => clearInterval(timer);
  }, []);
  void tick;
  const native = nativeBridgeOwner.status;
  const nativeLive = native?.sessionId && ["recording", "paused", "saving", "needs_attention"].includes(native.state);
  const state = browserCaptureOwner.panel();
  const browserLive = browserCaptureOwner.recordingId && state && ["recording", "paused", "saving", "recovering", "needs_attention"].includes(state.state);
  const currentSessionId = nativeLive ? native!.sessionId : browserLive ? state!.sessionId || browserCaptureOwner.recordingId : null;
  useEffect(() => setStopping(false), [currentSessionId]);
  if (!nativeLive && !browserLive) return null;
  const sessionId = nativeLive ? native!.sessionId! : state!.sessionId || browserCaptureOwner.recordingId!;
  const status = nativeLive ? native!.state : state!.state;
  const paused = status === "paused";
  const recording = status === "recording";
  const noAudio = recording && (nativeLive ? nativeBridgeOwner.noAudioWarning : browserCaptureOwner.noAudioWarning);
  const seconds = nativeLive ? Math.floor((native!.microphoneSamples || 0) / 16_000) : Math.floor(browserCaptureOwner.elapsedMs / 1_000);
  const elapsed = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  async function control(action: "pause" | "resume" | "stop") {
    setBusy(true);
    if (action === "stop") setStopping(true);
    try {
      if (nativeLive) await nativeBridgeOwner.control(action);
      else {
        const next = await browserCaptureOwner[action]();
        if (next) browserCaptureOwner.acceptPanel(sessionId, next);
      }
      if (action === "stop") rememberStopAck(sessionId, elapsed);
      setFailure(false);
    } catch { setFailure(true); setStopping(false); }
    finally { setBusy(false); }
  }
  return <aside className="margins-overlay" role="status" aria-label="Margins recording">
    <button className="margins-overlay-open" onClick={() => {
      const projectId = (() => { try { return sessionStorage.getItem("margins.bb.meetings-project") || context.projectId || ""; } catch { return context.projectId || ""; } })();
      navigate.toPluginPanel("meetings", { subPath: projectId ? `${projectId}/${sessionId}` : "" });
    }}><MeetingLevelDot level={recording ? nativeLive ? native!.micPeak ?? null : browserCaptureOwner.level : null} paused={paused} />
      <span>{failure || status === "needs_attention" ? "Needs attention" : stopping ? "Saving recording…" : noAudio ? "No audio — check microphone" : paused ? `Paused · ${elapsed}`
        : recording ? nativeLive ? `Recording · ${elapsed}` : `Microphone only · ${elapsed}` : "Saving"}</span></button>
    {noAudio && <span className="margins-overlay-error">{nativeLive
      ? native?.microphoneDeviceName ? `Check ${native.microphoneDeviceName} in Margins Menu` : "Check Margins Menu microphone permission"
      : "Check browser microphone permission"}</span>}
    {(recording || paused) && !stopping && <>
      <button onClick={() => void control(paused ? "resume" : "pause")} disabled={busy} aria-label={paused ? "Resume recording" : "Pause recording"}>{paused ? <Play size={13} /> : <Pause size={13} />}</button>
      <button onClick={() => void control("stop")} disabled={busy} aria-label="Stop and save recording"><Square size={12} /></button>
    </>}
  </aside>;
}

function NativeCapturePanel({ projectId, title }: { projectId: string; title?: string }) {
  const rpc = useRpc<typeof marginsRpcContract>();
  const [status, setStatus] = useState<NativeStatus | null>(() => nativeBridgeOwner.status);
  const [paired, setPaired] = useState(() => nativeBridgeOwner.paired);
  const [connectionError, setConnectionError] = useState(() => nativeBridgeOwner.connectionError);
  const [code, setCode] = useState("");
  const [port, setPort] = useState("18765");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pinRetry, setPinRetry] = useState(0);
  const pinned = useRef<string | null>(null);
  useEffect(() => nativeBridgeOwner.subscribe(() => {
    setStatus(nativeBridgeOwner.status);
    setPaired(nativeBridgeOwner.paired);
    setConnectionError(nativeBridgeOwner.connectionError);
  }), []);
  const saved = paired && status?.state === "saved" && status.sessionId;
  useEffect(() => {
    if (!saved || pinned.current === status.sessionId) return;
    let disposed = false;
    let succeeded = false;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
    pinned.current = status.sessionId;
    void rpc.call("pinNativeSession", {
      projectId, sessionId: status.sessionId!, instanceId: status.instanceId, workspaceId: status.workspaceId,
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
  }, [rpc, saved, status?.sessionId, projectId, pinRetry]);
  async function authority(): Promise<CaptureAuthority> {
    const result = await rpc.call("captureAuthority", { projectId });
    if (!result.ok) throw new Error(result.error.message);
    return result;
  }
  async function act(run: () => Promise<void>) {
    setBusy(true); setError(null);
    try { await run(); } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusy(false); }
  }
  const controls = <>
    {!paired && <>
        <p>For a manually started recorder, run <code>margins native-bridge --remote &lt;Workspace URL or SSH alias&gt; --workspace &lt;Workspace ID&gt; --origin {window.location.origin} --port {port || "18765"}</code> on this Mac, then enter its pairing code.</p>
        <div className="margins-native-pair"><input aria-label="Mac recorder port" type="number" min="1" max="65535" value={port} onChange={(event) => setPort(event.target.value)} /><input aria-label="Mac recorder pairing code" autoComplete="off" value={code} onChange={(event) => setCode(event.target.value)} placeholder="Pairing code" /><button disabled={busy || !code.trim()} onClick={() => void act(async () => { await nativeBridgeOwner.pair(code.trim(), await authority(), Number(port)); setCode(""); })}>Connect</button></div>
    </>}
    {paired && <>
      {status?.state === "ready" && <button disabled={busy} onClick={() => void act(async () => { if (browserCaptureOwner.active) throw new Error("Stop the microphone-only recording before starting Mac audio."); await nativeBridgeOwner.verify(await authority()); try { sessionStorage.setItem("margins.bb.meetings-project", projectId); } catch { /* private browser */ } await nativeBridgeOwner.control("start", title); })}>Start Mac recording</button>}
      {status?.state === "recording" && <div className="margins-native-actions"><button disabled={busy} onClick={() => void act(() => nativeBridgeOwner.control("pause"))}>Pause</button><button disabled={busy} onClick={() => void act(() => nativeBridgeOwner.control("stop"))}>Stop and save</button></div>}
      {status?.state === "paused" && <div className="margins-native-actions"><button disabled={busy} onClick={() => void act(() => nativeBridgeOwner.control("resume"))}>Resume</button><button disabled={busy} onClick={() => void act(() => nativeBridgeOwner.control("stop"))}>Stop and save</button></div>}
      {status?.state === "getting_ready" && <p>Getting microphone and computer audio ready…</p>}
      {status?.microphoneDeviceName && <p>Microphone: {status.microphoneDeviceName} · Change it in Margins Menu before recording.</p>}
      {status?.state === "saving" && <p>Saving the meeting…</p>}
      {status?.state === "saved" && <p>Mac recording saved. The connected note action will appear in this thread once BB confirms the session.</p>}
      {status?.state === "needs_attention" && <p>{status.error || "Mac recording needs attention. The local transfer spool may still need delivery."}</p>}
      {connectionError && <p className="margins-error"><AlertCircle size={13} />Cannot reach the Mac recorder: {connectionError}</p>}
      {status && ["recording", "paused", "saving", "saved"].includes(status.state) && <details><summary>Recording details</summary><p>Microphone samples: {status.microphoneSamples.toLocaleString()} · Computer audio samples: {status.systemSamples.toLocaleString()} · Computer audio frames: {status.systemFrames.toLocaleString()}</p></details>}
      {(!status || ["ready", "saved", "needs_attention"].includes(status.state)) && <button className="margins-quiet" disabled={busy} onClick={() => void act(async () => nativeBridgeOwner.forget())}>Disconnect Mac recorder</button>}
    </>}
    {error && <p className="margins-error"><AlertCircle size={13} />{error}</p>}
  </>;
  return paired
    ? <div className="margins-native"><div className="margins-native-heading"><strong>Mac microphone + computer audio</strong><span>{status?.state?.replaceAll("_", " ") || "Connected"}</span></div>{controls}</div>
    : <details className="margins-native margins-native-manual"><summary>Connect a Mac recorder manually</summary>{controls}</details>;
}

function WorkspaceMeetingNotes({ threadId, onMeetingChange }: { threadId: string; onMeetingChange: (active: boolean) => void }) {
  const rpc = useRpc<typeof marginsRpcContract>();
  const composer = useComposer();
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
    onMeetingChange(!result.meeting.inputFinalized);
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

  async function makeConnectedNote() {
    if (!meeting?.inputFinalized) return;
    setMessage(null);
    try {
      const authority = await rpc.call("captureAuthority", { threadId });
      if (!authority.ok) throw new Error(authority.error.message);
      const pin = await rpc.call("pinNativeSession", {
        threadId, sessionId: meeting.sessionId,
        instanceId: authority.instanceId, workspaceId: authority.workspaceId,
      });
      if (!pin.ok) throw new Error(pin.error.message);
      const result = await rpc.call("connectedNoteContext", { threadId, sessionId: meeting.sessionId });
      if (!result.ok) throw new Error(result.error.message);
      if (!result.context.transcript.available) {
        const requested = await rpc.call("transcribePinnedSession", { threadId, sessionId: meeting.sessionId });
        if (!requested.ok) throw new Error(requested.error.message);
        setMessage("Transcription is finishing. Make the connected note when it is ready.");
        return;
      }
      const request = `Create a connected note for this exact Margins meeting. Resolve transcript, memo, artifacts, and declared Sources before writing. Read and write note files only through the project's native filesystem Source; never proxy note bytes through Margins.\n\n${JSON.stringify(result.context)}`;
      composer.updateText((current) => current.trim() ? `${current.trimEnd()}\n\n${request}` : request);
      composer.focus();
    } catch (error) { setMessage(error instanceof Error ? error.message : String(error)); }
  }

  if (!meeting) return message ? <p className="margins-detail">{message}</p> : null;
  return <div className="margins-workspace-meeting">
    <p className="margins-detail">{meeting.inputFinalized ? "Latest saved meeting" : "Recording in Margins Menu"} · {meeting.title || meeting.sessionId}</p>
    {meeting.inputFinalized && <p className="margins-detail"><time dateTime={meeting.startedAt}>{new Date(meeting.startedAt).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}</time></p>}
    <textarea className="margins-notepad" aria-label="Workspace meeting notepad"
      placeholder="Take notes while Margins records…" value={draft}
      onChange={(event) => { dirty.current = true; latestDraft.current = event.target.value; setDraft(event.target.value); setMessage(null); }}
      onBlur={() => void save()} />
    <button className="margins-quiet" onClick={() => void save()} disabled={!dirty.current || saving}>Save note</button>
    {meeting.inputFinalized && <button className="margins-connected-note" onClick={() => void makeConnectedNote()}>Make connected note</button>}
    {message && <p className="margins-detail">{message}</p>}
  </div>;
}

export function MarginsPanel({ threadId, params }: { threadId: string; params: JsonValue | null }) {
  const rpc = useRpc<typeof marginsRpcContract>();
  const context = useBbContext();
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
          sessionId: current.recordingId, client,
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
      if (nativeBridgeOwner.paired && nativeBridgeOwner.status && ["getting_ready", "recording", "paused", "saving"].includes(nativeBridgeOwner.status.state)) {
        setMessage("Stop the Mac microphone + computer audio recording before starting a microphone-only recording.");
        return;
      }
      setState({ ...state, state: "getting_ready", title: "Getting recording ready", detail: "Waiting for microphone permission and this bb project." });
      if (context.projectId) try { sessionStorage.setItem("margins.bb.meetings-project", context.projectId); } catch { /* private browser */ }
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
    {state.state === "saved" && state.lastSessionId && <div className="margins-saved-actions">
      <button className="margins-connected-note" onClick={() => void connectedNote()} disabled={busy !== null}>Make connected note</button>
    </div>}
    {state.state === "needs_setup" && <p className="margins-seam">Recording with computer audio isn’t available in this browser yet.</p>}
    </>}
  </section>;
}

function MarginsSettings() {
  const rpc = useRpc<typeof marginsRpcContract>();
  const context = useBbContext();
  const [projects, setProjects] = useState<Array<{ id: string; name: string }>>([]);
  const [projectId, setProjectId] = useState(context.projectId || "");
  useEffect(() => {
    void rpc.call("availableProjects", {}).then(({ projects: available }) => {
      setProjects(available);
      setProjectId((current) => current || available[0]?.id || "");
    });
  }, [rpc]);
  return <div className="margins-settings">
    <label>bb project <select aria-label="bb project for Margins settings" value={projectId} onChange={(event) => setProjectId(event.target.value)}>
      {projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}
    </select></label>
    {projectId && <>
      <ProjectWorkspaceSetting key={projectId} projectId={projectId} onSaved={() => undefined} />
      <MenuAccessSetting projectId={projectId} />
      <NativeCapturePanel key={`native-${projectId}`} projectId={projectId} />
    </>}
  </div>;
}

export default definePluginApp((app) => {
  app.contentScripts.register({ id: "recording-owner", mount: (context) => browserCaptureOwner.install(context) });
  app.slots.experimental_appOverlay({ id: "recording-status", component: RecordingOverlay });
  app.slots.navPanel({ id: "meetings", title: "Meetings", icon: "Mic", path: "meetings",
    component: MeetingsPage, experimental_sidebarAccessory: MeetingsAccessory });
  app.slots.settingsSection({ id: "recording", title: "Margins recording", component: MarginsSettings });
  app.slots.threadPanelAction({
    id: "live", title: "Margins", icon: "Mic", layout: "flush",
    component: ({ threadId }) => <MarginsThreadTab threadId={threadId} />,
  });
});
