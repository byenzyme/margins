import { useCallback, useEffect, useRef, useState } from "react";
import { experimental_FileLink as FileLink, useBbContext, useBbNavigate, useRealtime, useRpc } from "@get-bb/plugin-sdk/app";
import { Pause } from "lucide-react";
import type { marginsRpcContract } from "../server.js";
import { browserCaptureOwner, detectClientCapabilities } from "./browser-capture.js";
import { nativeBridgeOwner } from "./native-bridge-client.js";
import type { PanelState, WorkspaceMeeting, WorkspaceMeetingSummary } from "./contracts.js";

const LAST_PROJECT_KEY = "margins.bb.meetings-project";
const HANDOFF_KEY = "margins.bb.note-draft";
const STOP_ACK_KEY = "margins.bb.stop-ack";
const STOP_ACK_EVENT = "margins:stop-saved";
type StopAck = { sessionId: string; elapsed: string; at: number };

function meetingTime(value: string) {
  return new Date(value).toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" });
}
function meetingListTitle(value: string) {
  const date = new Date(value);
  const today = date.toDateString() === new Date().toDateString();
  return `${today ? "Today" : date.toLocaleDateString("en-US", { month: "short", day: "numeric" })} ${meetingTime(value)}`;
}
function noteTitle(relativePath: string) {
  const title = relativePath.split(/[\\/]/).at(-1)?.replace(/\.md$/i, "")
    .replace(/^\d{4}-\d{2}-\d{2}[\s_-]+/, "").replace(/[-_]+/g, " ").trim();
  return title ? title[0]!.toUpperCase() + title.slice(1) : "Connected note";
}
function elapsedLabel(milliseconds: number) {
  const seconds = Math.max(0, Math.floor(milliseconds / 1_000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}
export function rememberStopAck(sessionId: string, elapsed: string) {
  const ack: StopAck = { sessionId, elapsed, at: Date.now() };
  try { sessionStorage.setItem(`${STOP_ACK_KEY}.${sessionId}`, JSON.stringify(ack)); } catch { /* private browser */ }
  window.dispatchEvent(new CustomEvent<StopAck>(STOP_ACK_EVENT, { detail: ack }));
}
function readStopAck(sessionId: string | null): StopAck | null {
  if (!sessionId) return null;
  try {
    const ack = JSON.parse(sessionStorage.getItem(`${STOP_ACK_KEY}.${sessionId}`) || "null") as StopAck | null;
    return ack?.sessionId === sessionId && Number.isFinite(ack.at) && Date.now() - ack.at < 4_000 ? ack : null;
  } catch { return null; }
}
function handoffKey(projectId: string, sessionId: string) { return `${HANDOFF_KEY}.${projectId}.${sessionId}`; }
function readHandoff(projectId: string, sessionId: string | null): string[] | null {
  if (!projectId || !sessionId) return null;
  try {
    const value = JSON.parse(sessionStorage.getItem(handoffKey(projectId, sessionId)) || "null");
    return Array.isArray(value) && value.every((item) => typeof item === "string") ? value : null;
  } catch { return null; }
}

export function MeetingLevelDot({ level, paused = false, accessory = false }: { level: number | null; paused?: boolean; accessory?: boolean }) {
  const amplitude = level === null ? null : Math.max(0, Math.min(1, level));
  return <span className={`margins-level-dot${paused ? " paused" : ""}`} aria-hidden={!accessory}
    aria-label={accessory ? amplitude === null ? "Recording indicator" : "Live audio level" : undefined}
    data-level={amplitude === null ? "static" : amplitude.toFixed(3)}
    style={paused || amplitude === null ? undefined : { opacity: .3 + amplitude * .7, transform: `scale(${.75 + amplitude * .65})` }} />;
}

function rememberedProject() {
  try { return sessionStorage.getItem(LAST_PROJECT_KEY) || ""; } catch { return ""; }
}

export function MeetingsAccessory() {
  const [browser, setBrowser] = useState(() => ({ active: browserCaptureOwner.active, level: browserCaptureOwner.level,
    state: browserCaptureOwner.panel()?.state }));
  const [native, setNative] = useState(() => nativeBridgeOwner.status);
  useEffect(() => {
    const unsubscribe = browserCaptureOwner.subscribe(() => setBrowser({ active: browserCaptureOwner.active,
      level: browserCaptureOwner.level, state: browserCaptureOwner.panel()?.state }));
    return () => { unsubscribe(); };
  }, []);
  useEffect(() => nativeBridgeOwner.subscribe(() => setNative(nativeBridgeOwner.status)), []);
  const paused = browser.active && browser.state === "paused" || native?.state === "paused";
  const recording = browser.active && browser.state === "recording" || native?.state === "recording";
  if (paused) return <Pause size={12} aria-label="Paused" />;
  if (!recording) return null;
  return <MeetingLevelDot level={native?.state === "recording" ? native.micPeak ?? null : browser.active ? browser.level : null} accessory />;
}

export function MeetingsPage({ subPath }: { subPath: string }) {
  const rpc = useRpc<typeof marginsRpcContract>();
  const navigate = useBbNavigate();
  const context = useBbContext();
  const [projects, setProjects] = useState<Array<{ id: string; name: string }>>([]);
  const [projectId, setProjectId] = useState(() => subPath.split("/")[0] || context.projectId || rememberedProject());
  const [workspaceChoice, setWorkspaceChoice] = useState("");
  const [workspaceOptions, setWorkspaceOptions] = useState<Array<{ id: string; name: string | null }>>([]);
  const [workspaceNotice, setWorkspaceNotice] = useState("");
  const [menuAvailable, setMenuAvailable] = useState(false);
  const [menuBusy, setMenuBusy] = useState(false);
  const [panel, setPanel] = useState<PanelState | null>(null);
  const [meetings, setMeetings] = useState<WorkspaceMeetingSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(() => subPath.split("/")[1] || null);
  const [meeting, setMeeting] = useState<WorkspaceMeeting | null>(null);
  const [draft, setDraft] = useState("");
  const [message, setMessage] = useState("");
  const [transcriptStatus, setTranscriptStatus] = useState<{ sessionId: string; state: "checking" | "ready" | "pending" | "not_ready" | "failed" } | null>(null);
  const [stopAck, setStopAck] = useState<StopAck | null>(null);
  const [handoffTick, setHandoffTick] = useState(0);
  const dirty = useRef(false);
  const latestDraft = useRef("");
  const revision = useRef("");
  const saveLoop = useRef<Promise<void> | null>(null);
  const memoRef = useRef<HTMLTextAreaElement | null>(null);
  const focusedSession = useRef("");
  const client = useRef(detectClientCapabilities()).current;
  const selected = meetings.find((item) => item.sessionId === selectedId);
  const handoff = readHandoff(projectId, selectedId);

  useEffect(() => {
    void rpc.call("availableProjects", {}).then(({ projects: available }) => {
      setProjects(available);
      setProjectId((current) => current || available[0]?.id || "");
    }).catch((error) => setMessage(String(error)));
  }, [rpc]);
  useEffect(() => { if (projectId) try { sessionStorage.setItem(LAST_PROJECT_KEY, projectId); } catch { /* private browser */ } }, [projectId]);
  useEffect(() => {
    if (client.platform !== "macos" || !navigator.permissions) return;
    let disposed = false;
    // Chromium prompts for loopback access. Only probe silently after the site
    // already has permission; Connect and Start provide a deliberate gesture.
    void navigator.permissions.query({ name: "loopback-network" as PermissionName }).then((permission) => {
      if (permission.state === "granted") return nativeBridgeOwner.probeMenu();
      return false;
    }).then((available) => { if (!disposed && available) setMenuAvailable(true); }).catch(() => undefined);
    return () => { disposed = true; };
  }, [client.platform]);

  const refresh = useCallback(async () => {
    if (!projectId) return;
    const options = await rpc.call("availableWorkspaces", { projectId });
    setWorkspaceOptions(options.workspaces);
    setWorkspaceChoice((current) => current && options.workspaces.some((item) => item.id === current)
      ? current : options.workspaces[0]?.id || "");
    if (options.autoSelected) setWorkspaceNotice(`Using ${options.workspaces[0]?.name || options.workspaces[0]?.id} as the machine default.`);
    const [nextPanel, listed] = await Promise.all([
      rpc.call("getProjectPanelState", { projectId, client }),
      rpc.call("listWorkspaceMeetings", { projectId }),
    ]);
    setPanel(nextPanel);
    if (!listed.ok) { setMessage(listed.error.message); return; }
    setMeetings(listed.meetings);
    setSelectedId((current) => {
      if (current && listed.meetings.some((item) => item.sessionId === current)) return current;
      return listed.meetings.find((item) => !item.inputFinalized)?.sessionId
        || listed.meetings.find((item) => item.inputFinalized && !item.notePath)?.sessionId
        || listed.meetings[0]?.sessionId || null;
    });
  }, [projectId, rpc, client.clientId]);
  useEffect(() => {
    void refresh().catch((error) => setMessage(String(error)));
    const timer = setInterval(() => void refresh().catch(() => undefined), 3_000);
    return () => clearInterval(timer);
  }, [refresh]);
  useRealtime("margins-recording", () => void refresh().catch(() => undefined));

  useEffect(() => {
    if (!projectId || !selectedId) { setMeeting(null); return; }
    let cancelled = false;
    void rpc.call("readWorkspaceMeeting", { projectId, sessionId: selectedId }).then((result) => {
      if (cancelled || !result.ok || !result.meeting) return;
      setMeeting(result.meeting);
      if (!dirty.current) {
        setDraft(result.meeting.notepad.text);
        latestDraft.current = result.meeting.notepad.text;
        revision.current = result.meeting.notepad.revision;
      }
    }).catch((error) => { if (!cancelled) setMessage(String(error)); });
    return () => { cancelled = true; };
  }, [projectId, selectedId, rpc]);

  useEffect(() => {
    if (!selected || selected.inputFinalized || meeting?.sessionId !== selected.sessionId || focusedSession.current === selected.sessionId) return;
    focusedSession.current = selected.sessionId;
    memoRef.current?.focus();
  }, [selected?.sessionId, selected?.inputFinalized, meeting?.sessionId]);
  useEffect(() => {
    if (message !== "Saved") return;
    const timer = setTimeout(() => setMessage((current) => current === "Saved" ? "" : current), 1_500);
    return () => clearTimeout(timer);
  }, [message]);
  useEffect(() => {
    if (!stopAck) return;
    const timer = setTimeout(() => setStopAck(null), Math.max(0, 4_000 - (Date.now() - stopAck.at)));
    return () => clearTimeout(timer);
  }, [stopAck]);
  useEffect(() => {
    setStopAck(readStopAck(selectedId));
    const onSaved = (event: Event) => {
      const ack = (event as CustomEvent<StopAck>).detail;
      if (ack.sessionId === selectedId) setStopAck(ack);
    };
    window.addEventListener(STOP_ACK_EVENT, onSaved);
    return () => window.removeEventListener(STOP_ACK_EVENT, onSaved);
  }, [selectedId]);
  useEffect(() => {
    if (!selectedId || !projectId || !selected?.inputFinalized) { setTranscriptStatus(null); return; }
    let disposed = false;
    setTranscriptStatus({ sessionId: selectedId, state: "checking" });
    const check = async () => {
      try {
        const result = await rpc.call("connectedNoteContext", { projectId, sessionId: selectedId });
        if (disposed) return;
        if (!result.ok) { setTranscriptStatus({ sessionId: selectedId, state: "failed" }); return; }
        setTranscriptStatus((current) => ({ sessionId: selectedId,
          state: result.context.transcript.available ? "ready" : current?.sessionId === selectedId && ["pending", "failed"].includes(current.state) ? current.state : "not_ready" }));
      } catch { if (!disposed) setTranscriptStatus({ sessionId: selectedId, state: "failed" }); }
    };
    void check();
    const timer = setInterval(() => void check(), 3_000);
    return () => { disposed = true; clearInterval(timer); };
  }, [projectId, selectedId, selected?.inputFinalized, rpc]);
  useEffect(() => {
    if (!selected || !handoff || !selected.threadIds?.some((id) => !handoff.includes(id))) return;
    try { sessionStorage.removeItem(handoffKey(projectId, selected.sessionId)); } catch { /* private browser */ }
    setHandoffTick((tick) => tick + 1);
  }, [projectId, selected?.sessionId, selected?.threadIds?.join(","), handoffTick]);

  async function saveMemo() {
    if (saveLoop.current) return saveLoop.current;
    if (!dirty.current || !meeting || !projectId || !revision.current) return;
    const sessionId = meeting.sessionId;
    const loop = (async () => {
      while (dirty.current) {
        const text = latestDraft.current;
        const result = await rpc.call("saveWorkspaceMemo", { projectId, sessionId,
          expectedRevision: revision.current, text });
        if (!result.ok || !result.meeting) throw new Error(result.ok ? "Meeting unavailable" : result.error.message);
        revision.current = result.meeting.notepad.revision;
        setMeeting(result.meeting);
        dirty.current = latestDraft.current !== text;
      }
      setMessage("Saved");
    })().finally(() => { saveLoop.current = null; });
    saveLoop.current = loop;
    return loop;
  }
  useEffect(() => {
    if (!dirty.current) return;
    const timer = setTimeout(() => void saveMemo().catch((error) => setMessage(String(error))), 800);
    return () => clearTimeout(timer);
  }, [draft]);

  async function choose(sessionId: string) {
    try { await saveMemo(); } catch (error) { setMessage(String(error)); return; }
    setMeeting(null);
    setSelectedId(sessionId);
    navigate.toPluginPanel("meetings", { subPath: `${projectId}/${sessionId}` });
  }
  async function chooseProject(nextProjectId: string) {
    try { await saveMemo(); } catch (error) { setMessage(String(error)); return; }
    dirty.current = false; latestDraft.current = ""; revision.current = "";
    setProjectId(nextProjectId); setSelectedId(null); setMeeting(null); setDraft(""); setMessage("");
    navigate.toPluginPanel("meetings", { subPath: nextProjectId });
  }
  async function control(action: "pause" | "resume" | "stop", sessionId = selectedId): Promise<boolean> {
    try {
      const native = nativeBridgeOwner.status;
      const elapsed = native?.sessionId === sessionId
        ? elapsedLabel((native.microphoneSamples || 0) / 16)
        : elapsedLabel(browserCaptureOwner.elapsedMs);
      if (native?.sessionId && native.sessionId === sessionId) await nativeBridgeOwner.control(action);
      else if (browserCaptureOwner.active || action === "stop" && browserCaptureOwner.hasPendingStop) {
        if (action === "stop" && browserCaptureOwner.hasPendingStop) await browserCaptureOwner.retryPendingStop();
        else await browserCaptureOwner[action]();
      } else throw new Error("Open the bb window with this recorder to control its microphone.");
      await refresh();
      if (action === "stop" && sessionId) {
        rememberStopAck(sessionId, elapsed);
        const latest = await rpc.call("readWorkspaceMeeting", { projectId, sessionId });
        if (latest.ok && latest.meeting && !dirty.current) {
          setMeeting(latest.meeting); setDraft(latest.meeting.notepad.text);
          latestDraft.current = latest.meeting.notepad.text;
          revision.current = latest.meeting.notepad.revision;
        }
      }
      return true;
    } catch (error) { setMessage(String(error)); return false; }
  }
  async function start() {
    if (!projectId) return;
    const live = meetings.find((item) => !item.inputFinalized);
    if (live && !window.confirm("Stop and save the current meeting, then start?")) return;
    if (live && !await control("stop", live.sessionId)) return;
    try {
      if (client.platform === "macos" || menuAvailable) {
        if (!nativeBridgeOwner.paired) await connectMenu();
        const authority = await rpc.call("captureAuthority", { projectId });
        if (!authority.ok) throw new Error(authority.error.message);
        await nativeBridgeOwner.verify(authority);
        try { sessionStorage.setItem(LAST_PROJECT_KEY, projectId); } catch { /* private browser */ }
        await nativeBridgeOwner.control("start");
        await refresh();
        return;
      }
      await startBrowserMicrophone();
    } catch (error) { setMessage(String(error)); }
  }
  async function startBrowserMicrophone() {
    if (!projectId) return;
    try {
      const fallbackKey = "margins.bb.browser-mic-confirmed";
      if (sessionStorage.getItem(fallbackKey) !== "yes") {
        if (!window.confirm("Record with this browser's microphone only?")) return;
        sessionStorage.setItem(fallbackKey, "yes");
      }
      const next = await browserCaptureOwner.startFromProject(projectId);
      if (next.error) throw new Error(next.error.message);
      await refresh();
      if (next.sessionId) setSelectedId(next.sessionId);
    } catch (error) { setMessage(String(error)); }
  }
  async function connectMenu() {
    if (!projectId) return;
    setMenuBusy(true);
    try {
      const grant = await rpc.call("issueMenuGrant", { projectId, origin: window.location.origin });
      try { await nativeBridgeOwner.connectMenu(grant); }
      catch (error) {
        if (error instanceof TypeError || error instanceof DOMException && error.name === "AbortError") {
          throw new Error("Can't reach Margins Menu. Open it and allow bb to access local devices in this browser, then click Connect again.");
        }
        throw error;
      }
      setMenuAvailable(true);
      setMessage("");
    } catch (error) { setMessage(error instanceof Error ? error.message : String(error)); throw error; }
    finally { setMenuBusy(false); }
  }
  async function chooseWorkspace() {
    if (!projectId) return;
    try {
      await rpc.call("projectWorkspace", { projectId, workspaceId: workspaceChoice });
      setMessage("");
      await refresh();
    } catch (error) { setMessage(String(error)); }
  }
  async function retryTranscription() {
    if (!selectedId || !projectId) return;
    setTranscriptStatus({ sessionId: selectedId, state: "pending" });
    try {
      const result = await rpc.call("transcribePinnedSession", { projectId, sessionId: selectedId });
      if (!result.ok) throw new Error(result.error.message);
      setTranscriptStatus({ sessionId: selectedId, state: result.status === "complete" ? "ready" : "pending" });
      setMessage("");
    } catch (error) { setTranscriptStatus({ sessionId: selectedId, state: "failed" }); setMessage(String(error)); }
  }
  async function distill() {
    if (!selected || !projectId) return;
    try {
      await saveMemo();
      const result = await rpc.call("connectedNoteContext", { projectId, sessionId: selected.sessionId });
      if (!result.ok) throw new Error(result.error.message);
      const { sessionId, workspaceId, memo } = result.context;
      let transcript: "ready" | "pending" = "ready";
      if (!result.context.transcript.available) {
        const requested = await rpc.call("transcribePinnedSession", { projectId, sessionId });
        if (!requested.ok) { setTranscriptStatus({ sessionId, state: "failed" }); throw new Error(requested.error.message); }
        transcript = requested.status === "complete" ? "ready" : "pending";
        setTranscriptStatus({ sessionId, state: transcript });
      }
      const date = new Date(meeting?.startedAt || selected.startedAt).toLocaleDateString("en-US", {
        month: "long", day: "numeric", year: "numeric",
      });
      const title = (selected.title || "").replace(/\s+/g, " ").trim().slice(0, 100);
      const label = title ? `${title}${/meeting$/i.test(title) ? "" : " meeting"} on ${date}` : `meeting on ${date}`;
      const action = result.context.noteAssociation ? "Update the connected note from" : "Make a connected note from";
      const contextBlock = JSON.stringify({ workspaceId, sessionId, memoRevision: memo.revision,
        bbProjectId: projectId, transcript, note: result.context.noteAssociation ? "update" : "create" });
      try { sessionStorage.setItem(handoffKey(projectId, sessionId), JSON.stringify(selected.threadIds || [])); } catch { /* private browser */ }
      setHandoffTick((tick) => tick + 1);
      navigate.toCompose({ initialPrompt: `${action} my ${label}.\n\n<margins-context-v1>\n${contextBlock}\n</margins-context-v1>`, focusPrompt: true });
    } catch (error) { setMessage(String(error)); }
  }

  const live = meetings.filter((item) => !item.inputFinalized);
  const ready = meetings.filter((item) => item.inputFinalized && !item.notePath);
  const distilled = meetings.filter((item) => item.inputFinalized && item.notePath);
  const pausedSession = (sessionId: string) => panel?.sessionId === sessionId && panel.state === "paused"
    || nativeBridgeOwner.status?.sessionId === sessionId && nativeBridgeOwner.status.state === "paused";
  const memoChangedSinceNote = Boolean(selected?.notePath && meeting &&
    (selected.distilledMemoRevision !== meeting.notepad.revision || dirty.current));
  const noteAction = selected?.inputFinalized && (!selected.notePath || memoChangedSinceNote);
  const threadLinks = selected?.threadLinks?.length ? selected.threadLinks
    : (selected?.threadIds || []).map((id) => ({ id, title: "Meeting note thread" }));
  const shownTranscript = transcriptStatus?.sessionId === selectedId ? transcriptStatus.state : "checking";
  const groups = [["Live", live], ["Ready to refine", ready], ["Distilled", distilled]] as const;
  return <main className="margins-meetings-page">
    <aside className="margins-meeting-list">
      <div className="margins-meetings-top"><strong>Meetings</strong>
        <button className={meetings.length === 0 || live.length > 0 ? "is-reserved" : ""} aria-hidden={meetings.length === 0 || live.length > 0}
          tabIndex={meetings.length === 0 || live.length > 0 ? -1 : undefined}
          onClick={() => void start()} disabled={panel?.state === "unavailable" || meetings.length === 0 || live.length > 0}>Start</button></div>
      <select aria-label="bb project for Meetings" value={projectId} disabled={live.length > 0} onChange={(event) => void chooseProject(event.target.value)}>{projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}</select>
      {groups.map(([label, items]) => items.length > 0 && <section key={label}>
        <h3>{label}</h3>{items.map((item) => <button key={item.sessionId} className={item.sessionId === selectedId ? "selected" : ""}
          onClick={() => void choose(item.sessionId)}><span>{item.title || meetingListTitle(item.startedAt)}</span>
          {!item.inputFinalized && pausedSession(item.sessionId) && <small>Paused</small>}</button>)}
      </section>)}
    </aside>
    <section className="margins-meeting-pad">
      {(client.platform === "macos" || menuAvailable) && live.length === 0 && <div className="margins-menu-connect">
        <span>{nativeBridgeOwner.paired ? "Margins Menu connected (mic + computer audio)" : "Record with Margins Menu (mic + computer audio)"}</span>
        {!nativeBridgeOwner.paired && <button disabled={menuBusy || !projectId} onClick={() => void connectMenu().catch(() => undefined)}>Connect</button>}
        {client.platform === "macos" && !nativeBridgeOwner.paired && <button disabled={menuBusy || !projectId} onClick={() => void startBrowserMicrophone()}>Browser mic only</button>}
      </div>}
      {panel?.state === "unavailable" && <div className="margins-meetings-empty">
        <h2>{workspaceOptions.length ? "Choose a Margins Workspace" : "Set up a Margins Workspace"}</h2>
        {workspaceOptions.length ? <div><select aria-label="Margins Workspace" value={workspaceChoice} onChange={(event) => setWorkspaceChoice(event.target.value)}>
          {workspaceOptions.map((item) => <option key={item.id} value={item.id}>{item.name || item.id}</option>)}
        </select><button onClick={() => void chooseWorkspace()} disabled={!workspaceChoice}>Use Workspace</button></div>
          : <p>Use the Margins workspace-setup skill to choose where meetings and notes live.</p>}
        {message && <p role="alert">{message}</p>}
      </div>}
      {panel?.state !== "unavailable" && selected && meeting ? <>
        {workspaceNotice && <p className="margins-workspace-notice">{workspaceNotice}</p>}
        <header><div><div className="margins-meeting-meta"><span className="margins-meeting-kicker">
          {!selected.inputFinalized ? <><i className={`margins-meeting-state-dot${pausedSession(selected.sessionId) ? " paused" : ""}`} aria-hidden="true" />{pausedSession(selected.sessionId) ? "Paused" : "Recording"}</>
            : stopAck?.sessionId === selected.sessionId ? `Saved · ${stopAck.elapsed} recorded`
              : memoChangedSinceNote ? "Memo updated since note" : selected.notePath ? "Note created" : "Ready"}</span>
          {selected.notePath && <nav className="margins-meeting-links" aria-label="Meeting links">
            {selected.noteFile ? <FileLink title="Open note preview" target={{ kind: "host", ...selected.noteFile }} location={null}>{noteTitle(selected.notePath)}</FileLink>
              : <span title="File preview is unavailable on this host">{noteTitle(selected.notePath)}</span>}
            {threadLinks.map(({ id, title }) => <button key={id} onClick={() => navigate.toThread(id)}>{title}</button>)}
          </nav>}
          {message && <span className={`margins-meeting-status${message === "Saved" ? " saved" : " error"}`} role="status">{message}</span>}</div>
          <h2>{meeting.title || `Meeting · ${meetingTime(meeting.startedAt)}`}</h2></div>
          {noteAction && <div className="margins-meeting-next"><button onClick={() => void distill()}>{selected.notePath ? "Update note" : "Make note"} →</button></div>}
        </header>
        {selected.inputFinalized && <div className="margins-meeting-trail"><span>{shownTranscript === "ready" ? "Transcript ready" : shownTranscript === "pending" ? "Transcribing…" : shownTranscript === "checking" ? "Checking transcript…" : shownTranscript === "failed" ? "Transcript unavailable" : "Transcript not ready"}</span>
          {(shownTranscript === "failed" || shownTranscript === "not_ready") && <button onClick={() => void retryTranscription()}>{shownTranscript === "failed" ? "Retry" : "Transcribe"}</button>}
          {handoff && <span>Note draft opened — press Enter to start</span>}</div>}
        <textarea ref={memoRef} aria-label="Meeting memo pad" placeholder="Write notes..." value={draft} onChange={(event) => { dirty.current = true; latestDraft.current = event.target.value; setDraft(event.target.value); setMessage(""); }} onBlur={() => void saveMemo().catch((error) => setMessage(String(error)))} />
        <footer>{memoChangedSinceNote && <span>Note uses an earlier memo revision</span>}</footer>
      </> : panel?.state !== "unavailable" && <div className="margins-meetings-empty"><h2>No meetings yet</h2>{workspaceNotice && <p>{workspaceNotice}</p>}<button onClick={() => void start()}>Start meeting</button>
        {message && <p role="alert">{message}</p>}</div>}
    </section>
  </main>;
}
