import { useEffect, useRef, useState } from "react";
import { definePluginApp, useBbContext, useBbNavigate, useRpc } from "@get-bb/plugin-sdk/app";
import { AlertCircle, Pause, Play, Square } from "lucide-react";
import type { marginsRpcContract } from "../server.js";
import { browserCaptureOwner } from "./browser-capture.js";
import { nativeBridgeOwner, nativeMicrophoneDurationMs, type CaptureAuthority, type NativeStatus } from "./native-bridge-client.js";
import { MeetingLevelDot, MeetingsAccessory, MeetingsPage, rememberStopAck } from "./meetings-page.js";
import { MarginsThreadTab } from "./thread-tab.js";

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
  const seconds = nativeLive ? Math.floor(nativeMicrophoneDurationMs(native!) / 1_000) : Math.floor(browserCaptureOwner.elapsedMs / 1_000);
  const elapsed = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  async function control(action: "pause" | "resume" | "stop") {
    setBusy(true);
    if (action === "stop") setStopping(true);
    try {
      let saved = Boolean(nativeLive);
      if (nativeLive) await nativeBridgeOwner.control(action);
      else {
        const next = await browserCaptureOwner[action]();
        if (next) browserCaptureOwner.acceptPanel(sessionId, next);
        if (next?.state === "needs_attention") setFailure(true);
        saved = next?.state === "saved";
      }
      if (action === "stop" && saved) rememberStopAck(sessionId, elapsed);
    } catch { setFailure(true); }
    finally { setBusy(false); setStopping(false); }
  }
  async function finishPending(incomplete: boolean) {
    setBusy(true);
    try {
      const next = incomplete ? await browserCaptureOwner.finishIncomplete() : await browserCaptureOwner.retryPendingStop();
      if (next) browserCaptureOwner.acceptPanel(sessionId, next);
      setFailure(next?.state === "needs_attention");
    } catch { setFailure(true); }
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
    {!nativeLive && status === "needs_attention" && browserCaptureOwner.hasPendingStop
      && (browserCaptureOwner.canFinishIncomplete || state?.primaryAction === "retry") && <div className="margins-native-actions">
      <span className="margins-overlay-error">{state?.error?.message || "Some browser audio was not saved."}</span>
      <button disabled={busy} onClick={() => void finishPending(browserCaptureOwner.canFinishIncomplete)}>
        {browserCaptureOwner.canFinishIncomplete ? "Finish with what was saved" : "Try again"}
      </button>
    </div>}
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
      {status?.state === "getting_ready" && <p>Starting the microphone and computer audio. Don't speak yet; recording begins when this says Recording.</p>}
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
  // Older bb hosts can still render Meetings and the thread panel even when
  // they do not provide the newer persistent overlay slot.
  const hasOverlaySlot = typeof app.slots.experimental_appOverlay === "function";
  if (hasOverlaySlot) {
    app.slots.experimental_appOverlay({ id: "recording-status", component: RecordingOverlay });
  }
  app.slots.navPanel({ id: "meetings", title: "Meetings", icon: "Mic", path: "meetings",
    component: (props) => <><MeetingsPage {...props} />{!hasOverlaySlot && <RecordingOverlay />}</>,
    experimental_sidebarAccessory: MeetingsAccessory });
  app.slots.settingsSection({ id: "recording", title: "Margins recording", component: MarginsSettings });
  app.slots.threadPanelAction({
    id: "live", title: "Margins", icon: "Mic", layout: "flush",
    component: ({ threadId }) => <MarginsThreadTab threadId={threadId} />,
  });
});
