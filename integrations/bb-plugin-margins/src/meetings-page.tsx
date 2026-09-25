import { useCallback, useEffect, useRef, useState } from "react";
import { useBbContext, useBbNavigate, useRealtime, useRpc } from "@get-bb/plugin-sdk/app";
import { Pause } from "lucide-react";
import type { marginsRpcContract } from "../server.js";
import { browserCaptureOwner, detectClientCapabilities } from "./browser-capture.js";
import { nativeBridgeOwner } from "./native-bridge-client.js";
import type { PanelState, WorkspaceMeeting, WorkspaceMeetingSummary } from "./contracts.js";

const LAST_PROJECT_KEY = "margins.bb.meetings-project";

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
  const level = browser.active ? browser.level : native && native.microphoneSamples > 0 ? .35 : 0;
  return <span className="margins-sidebar-level" aria-label="Live audio level" data-level={level.toFixed(3)}>
    {[.45, .8, 1, .65].map((factor, index) => <i key={index} style={{ height: `${Math.max(3, level * factor * 17)}px` }} />)}
  </span>;
}

export function MeetingsPage({ subPath }: { subPath: string }) {
  const rpc = useRpc<typeof marginsRpcContract>();
  const navigate = useBbNavigate();
  const context = useBbContext();
  const [projects, setProjects] = useState<Array<{ id: string; name: string }>>([]);
  const [projectId, setProjectId] = useState(() => subPath.split("/")[0] || context.projectId || rememberedProject());
  const [workspaceChoice, setWorkspaceChoice] = useState("");
  const [panel, setPanel] = useState<PanelState | null>(null);
  const [meetings, setMeetings] = useState<WorkspaceMeetingSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(() => subPath.split("/")[1] || null);
  const [meeting, setMeeting] = useState<WorkspaceMeeting | null>(null);
  const [draft, setDraft] = useState("");
  const [message, setMessage] = useState("");
  const dirty = useRef(false);
  const latestDraft = useRef("");
  const revision = useRef("");
  const saveLoop = useRef<Promise<void> | null>(null);
  const client = useRef(detectClientCapabilities()).current;

  useEffect(() => {
    void rpc.call("availableProjects", {}).then(({ projects: available }) => {
      setProjects(available);
      setProjectId((current) => current || available[0]?.id || "");
    }).catch((error) => setMessage(String(error)));
  }, [rpc]);
  useEffect(() => { if (projectId) try { sessionStorage.setItem(LAST_PROJECT_KEY, projectId); } catch { /* private browser */ } }, [projectId]);

  const refresh = useCallback(async () => {
    if (!projectId) return;
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
      if (native?.sessionId && native.sessionId === sessionId) await nativeBridgeOwner.control(action);
      else if (browserCaptureOwner.active || action === "stop" && browserCaptureOwner.hasPendingStop) {
        if (action === "stop" && browserCaptureOwner.hasPendingStop) await browserCaptureOwner.retryPendingStop();
        else await browserCaptureOwner[action]();
      } else throw new Error("Open the bb window with this recorder to control its microphone.");
      await refresh();
      if (action === "stop" && sessionId) {
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
      const next = await browserCaptureOwner.startFromProject(projectId);
      if (next.error) throw new Error(next.error.message);
      await refresh();
      if (next.sessionId) setSelectedId(next.sessionId);
    } catch (error) { setMessage(String(error)); }
  }
  async function chooseWorkspace() {
    if (!projectId) return;
    try {
      await rpc.call("projectWorkspace", { projectId, workspaceId: workspaceChoice });
      setMessage("");
      await refresh();
    } catch (error) { setMessage(String(error)); }
  }
  async function distill() {
    if (!selected || !projectId) return;
    try {
      await saveMemo();
      const result = await rpc.call("connectedNoteContext", { projectId, sessionId: selected.sessionId });
      if (!result.ok) throw new Error(result.error.message);
      const { sessionId, workspaceId, memo } = result.context;
      let transcriptRequest = "";
      if (!result.context.transcript.available) {
        const requested = await rpc.call("transcribePinnedSession", { projectId, sessionId });
        transcriptRequest = requested.ok ? "Transcription has been requested; wait for it before writing. "
          : `Transcription could not be requested (${requested.error.message}); arrange transcription before writing. `;
      }
      const noteTask = result.context.noteAssociation
        ? "Review the note already associated with this session and update it with the revised memo."
        : "Create a connected note for this session.";
      navigate.toCompose({ initialPrompt: `Work in bb project ${projectId}. Use the Margins distillation skill for Margins session ${sessionId} in Workspace ${workspaceId}. ${noteTask} Distill memo revision ${memo.revision}. Read that exact session's memo and transcript, then write the note through the Workspace Source. ${transcriptRequest}After writing, associate the note with the session and record this bb thread id and distilled memo revision on the session.`, focusPrompt: true });
    } catch (error) { setMessage(String(error)); }
  }

  const live = meetings.filter((item) => !item.inputFinalized);
  const ready = meetings.filter((item) => item.inputFinalized && !item.notePath);
  const distilled = meetings.filter((item) => item.inputFinalized && item.notePath);
  const selected = meetings.find((item) => item.sessionId === selectedId);
  const memoChangedSinceNote = Boolean(selected?.notePath && meeting &&
    (selected.distilledMemoRevision !== meeting.notepad.revision || dirty.current));
  const noteAction = selected?.inputFinalized && (!selected.notePath || memoChangedSinceNote);
  const groups = [["Live", live], ["Ready to refine", ready], ["Distilled", distilled]] as const;
  return <main className="margins-meetings-page">
    <aside className="margins-meeting-list">
      <div className="margins-meetings-top"><strong>Meetings</strong>{meetings.length > 0 && live.length === 0 &&
        <button onClick={() => void start()} disabled={panel?.state === "unavailable"}>Start</button>}</div>
      <select aria-label="bb project for Meetings" value={projectId} onChange={(event) => void chooseProject(event.target.value)}>{projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}</select>
      {groups.map(([label, items]) => items.length > 0 && <section key={label}>
        <h3>{label}</h3>{items.map((item) => <button key={item.sessionId} className={item.sessionId === selectedId ? "selected" : ""}
          onClick={() => void choose(item.sessionId)}><span>{item.title || new Date(item.startedAt).toLocaleString()}</span>
          {!item.inputFinalized && <small>Live</small>}</button>)}
      </section>)}
    </aside>
    <section className="margins-meeting-pad">
      {panel?.state === "unavailable" && <div className="margins-meetings-empty">
        <h2>Choose a Margins Workspace</h2><p>{panel.error?.message || "Set up a Workspace to store meetings and notes."}</p>
        <div><input aria-label="Workspace id" value={workspaceChoice} onChange={(event) => setWorkspaceChoice(event.target.value)} placeholder="Workspace id" />
          <button onClick={() => void chooseWorkspace()}>Use Workspace</button></div>
        <p>To create one, use the Margins workspace-setup skill.</p>
      </div>}
      {panel?.state !== "unavailable" && selected && meeting ? <>
        <header><div><span className="margins-meeting-kicker">{!selected.inputFinalized ? "Live" : memoChangedSinceNote ? "Memo updated since note" : selected.notePath ? "Note created" : "Ready to refine"}</span>
          <h2>{meeting.title || new Date(meeting.startedAt).toLocaleString()}</h2></div>
          {noteAction && <div className="margins-meeting-next"><button onClick={() => void distill()}>{selected.notePath ? "Update note in new thread" : "Make note in new thread"} →</button>
            <small>Memo stays editable.</small></div>}
        </header>
        <textarea aria-label="Meeting memo pad" placeholder="" value={draft} onChange={(event) => { dirty.current = true; latestDraft.current = event.target.value; setDraft(event.target.value); setMessage(""); }} onBlur={() => void saveMemo().catch((error) => setMessage(String(error)))} />
        <footer><span role="status">{message}</span>
          {selected.notePath && <span>Note: {selected.notePath}</span>}
          {(selected.threadIds || []).map((threadId) => <button key={threadId} onClick={() => navigate.toThread(threadId)}>Thread {threadId}</button>)}
          {memoChangedSinceNote && <span>Note uses an earlier memo revision</span>}</footer>
      </> : panel?.state !== "unavailable" && <div className="margins-meetings-empty"><h2>No meetings yet</h2><button onClick={() => void start()}>Start meeting</button></div>}
    </section>
  </main>;
}
