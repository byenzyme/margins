import { useCallback, useEffect, useRef, useState } from "react";
import { experimental_FileLink as FileLink, useBbContext, useBbNavigate, useRealtime, useRpc } from "@get-bb/plugin-sdk/app";
import { Pause } from "lucide-react";
import type { marginsRpcContract } from "../server.js";
import { browserCaptureOwner, detectClientCapabilities } from "./browser-capture.js";
import { nativeBridgeOwner } from "./native-bridge-client.js";
import type { PanelState, WorkspaceMeeting, WorkspaceMeetingSummary } from "./contracts.js";
import type { WorkspaceSetupPreview } from "./workspace-setup.js";

const LAST_PROJECT_KEY = "margins.bb.meetings-project";
const STOP_ACK_KEY = "margins.bb.stop-ack";
const STOP_ACK_EVENT = "margins:stop-saved";
const openedMeetings = new Map<string, WorkspaceMeeting>();
const openedSummaries = new Map<string, WorkspaceMeetingSummary>();
function rememberMeeting(key: string, value: WorkspaceMeeting) {
  openedMeetings.delete(key);
  openedMeetings.set(key, value);
  if (openedMeetings.size > 50) openedMeetings.delete(openedMeetings.keys().next().value!);
}
function rememberSummary(key: string, value: WorkspaceMeetingSummary) {
  openedSummaries.delete(key);
  openedSummaries.set(key, value);
  if (openedSummaries.size > 50) openedSummaries.delete(openedSummaries.keys().next().value!);
}
type StopAck = { sessionId: string; elapsed: string; at: number };

function meetingTime(value: string) {
  return new Date(value).toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" });
}
function meetingListTitle(value: string) {
  const date = new Date(value);
  const today = date.toDateString() === new Date().toDateString();
  return `${today ? "Today" : date.toLocaleDateString("en-US", { month: "short", day: "numeric" })} ${meetingTime(value)}`;
}
/** `folder:People` reads as "People"; other entity refs stay as written. */
function readingLabel(reading: string) {
  return reading.replace(/^folder:/i, "");
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
  const navigateRef = useRef(navigate);
  navigateRef.current = navigate;
  const context = useBbContext();
  const [projects, setProjects] = useState<Array<{ id: string; name: string }>>([]);
  const [projectId, setProjectId] = useState(() => subPath.split("/")[0] || context.projectId || rememberedProject());
  const [workspaceChoice, setWorkspaceChoice] = useState("");
  const [workspaceOptions, setWorkspaceOptions] = useState<Array<{ id: string; name: string | null }>>([]);
  const [workspaceNotice, setWorkspaceNotice] = useState("");
  const [setupHome, setSetupHome] = useState("");
  const [setupPreview, setSetupPreview] = useState<WorkspaceSetupPreview | null>(null);
  const [setupBusy, setSetupBusy] = useState(false);
  const [resolvedWorkspaceName, setResolvedWorkspaceName] = useState("");
  const [menuAvailable, setMenuAvailable] = useState(false);
  const [menuBusy, setMenuBusy] = useState(false);
  const [starting, setStarting] = useState(false);
  const [audioStartingId, setAudioStartingId] = useState<string | null>(null);
  const [startError, setStartError] = useState("");
  const [pendingDraft, setPendingDraft] = useState("");
  const [nativeStatus, setNativeStatus] = useState(() => nativeBridgeOwner.status);
  const [nativeConnectionError, setNativeConnectionError] = useState(() => nativeBridgeOwner.connectionError);
  const [panel, setPanel] = useState<PanelState | null>(null);
  const [, setCaptureVersion] = useState(0);
  useEffect(() => {
    const unsubscribe = browserCaptureOwner.subscribe(() => setCaptureVersion((version) => version + 1));
    return () => { unsubscribe(); };
  }, []);
  const [meetings, setMeetings] = useState<WorkspaceMeetingSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(() => subPath.split("/")[1] || null);
  const [meeting, setMeeting] = useState<WorkspaceMeeting | null>(() => openedMeetings.get(`${projectId}/${selectedId}`) || null);
  const [draft, setDraft] = useState(() => openedMeetings.get(`${projectId}/${selectedId}`)?.notepad.text || "");
  const [message, setMessage] = useState("");
  const [transcriptStatus, setTranscriptStatus] = useState<{ sessionId: string; state: "checking" | "ready" | "pending" | "not_ready" | "failed" } | null>(null);
  const [speechSetup, setSpeechSetup] = useState<{ state: "preparing" | "ready" | "failed" | "unavailable"; message: string; progress: number | null } | null>(null);
  const [stopAck, setStopAck] = useState<StopAck | null>(null);
  const [noteBusy, setNoteBusy] = useState(false);
  const [showArchived, setShowArchived] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");
  const [transcriptOpen, setTranscriptOpen] = useState(false);
  const [transcriptBody, setTranscriptBody] = useState("");
  const [actionBusy, setActionBusy] = useState(false);
  const dirty = useRef(false);
  const latestDraft = useRef(draft);
  const revision = useRef(meeting?.notepad.revision || "");
  const saveLoop = useRef<Promise<void> | null>(null);
  const memoRef = useRef<HTMLTextAreaElement | null>(null);
  const focusedSession = useRef("");
  const pendingDraftText = useRef("");
  const pendingNativeStart = useRef<{ projectId: string; existing: Set<string>; startedAt: number; accepted: boolean } | null>(null);
  const observedLiveSession = useRef<string | null>(null);
  const refreshInFlight = useRef<Promise<void> | null>(null);
  const client = useRef(detectClientCapabilities()).current;
  const selected = meetings.find((item) => item.sessionId === selectedId)
    || (selectedId ? openedSummaries.get(`${projectId}/${selectedId}`) : undefined);

  useEffect(() => {
    void rpc.call("availableProjects", {}).then(({ projects: available }) => {
      setProjects(available);
      setProjectId((current) => current || available[0]?.id || "");
    }).catch((error) => setMessage(String(error)));
  }, [rpc]);
  useEffect(() => { if (projectId) try { sessionStorage.setItem(LAST_PROJECT_KEY, projectId); } catch { /* private browser */ } }, [projectId]);
  useEffect(() => nativeBridgeOwner.subscribe(() => {
    setNativeStatus(nativeBridgeOwner.status);
    setNativeConnectionError(nativeBridgeOwner.connectionError);
  }), []);
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

  const refreshNow = useCallback(async () => {
    if (!projectId) return;
    const options = await rpc.call("availableWorkspaces", { projectId });
    setWorkspaceOptions(options.workspaces);
    setResolvedWorkspaceName(options.workspaces.find((item) => item.id === options.resolvedWorkspaceId)?.name
      || options.resolvedWorkspaceId || "");
    setWorkspaceChoice((current) => current && options.workspaces.some((item) => item.id === current)
      ? current : options.workspaces[0]?.id || "");
    if (options.autoSelected) setWorkspaceNotice(`Using ${options.workspaces[0]?.name || options.workspaces[0]?.id} as the machine default.`);
    const [nextPanel, listed, speech] = await Promise.all([
      rpc.call("getProjectPanelState", { projectId, client }),
      rpc.call("listWorkspaceMeetings", { projectId }),
      rpc.call("speechSetup", { projectId }).catch(() => null),
    ]);
    setPanel(nextPanel);
    if (speech?.ok) setSpeechSetup({ state: speech.state, message: speech.message, progress: speech.progress });
    if (!listed.ok) { setMessage(listed.error.message); return; }
    for (const item of listed.meetings) rememberSummary(`${projectId}/${item.sessionId}`, item);
    setMeetings(listed.meetings);
    const liveSession = listed.meetings.find((item) => !item.inputFinalized)?.sessionId || null;
    const newlyObservedLiveSession = liveSession && liveSession !== observedLiveSession.current ? liveSession : null;
    if (newlyObservedLiveSession && !pendingNativeStart.current && newlyObservedLiveSession !== selectedId) {
      try { await saveMemo(); }
      catch (error) { setMessage(String(error)); return; }
      const cached = openedMeetings.get(`${projectId}/${newlyObservedLiveSession}`) || null;
      dirty.current = false;
      revision.current = cached?.notepad.revision || "";
      latestDraft.current = cached?.notepad.text || "";
      setMeeting(cached); setDraft(cached?.notepad.text || "");
      navigateRef.current.toPluginPanel("meetings", { subPath: `${projectId}/${newlyObservedLiveSession}` });
    }
    observedLiveSession.current = liveSession;
    const pending = pendingNativeStart.current;
    const recordingId = nativeBridgeOwner.status?.sessionId;
    const candidates = pending?.projectId === projectId && pending.accepted
      ? listed.meetings.filter((item) => !pending.existing.has(item.sessionId)
        && Date.parse(item.startedAt) >= pending.startedAt - 2_000) : [];
    const expectedId = recordingId && pending && !pending.existing.has(recordingId) ? recordingId : null;
    const newMeeting = expectedId ? candidates.find((item) => item.sessionId === expectedId)
      : candidates.length === 1 ? candidates[0] : null;
    if (newMeeting) {
      void rpc.call("recordMeetingOrigin", { projectId, sessionId: newMeeting.sessionId }).catch(() => undefined);
      pendingNativeStart.current = null;
      setStarting(false);
      setStartError("");
      setAudioStartingId(newMeeting.sessionId);
      const text = pendingDraftText.current;
      dirty.current = text.length > 0; latestDraft.current = text; revision.current = "";
      setMeeting(null); setDraft(text);
      navigateRef.current.toPluginPanel("meetings", { subPath: `${projectId}/${newMeeting.sessionId}` });
    }
    setSelectedId((current) => {
      if (newMeeting) return newMeeting.sessionId;
      if (pendingNativeStart.current) return current;
      if (newlyObservedLiveSession) return newlyObservedLiveSession;
      if (current && listed.meetings.some((item) => item.sessionId === current)) return current;
      const recent = listed.meetings.filter((item) => !item.archived);
      return recent.find((item) => !item.inputFinalized)?.sessionId
        || recent.find((item) => item.inputFinalized && !item.notePath)?.sessionId
        || recent[0]?.sessionId || null;
    });
  }, [projectId, selectedId, rpc, client.clientId]);
  const refresh = useCallback((): Promise<void> => {
    if (refreshInFlight.current) return refreshInFlight.current;
    const task = refreshNow();
    refreshInFlight.current = task;
    void task.finally(() => {
      if (refreshInFlight.current === task) refreshInFlight.current = null;
    }).catch(() => undefined);
    return task;
  }, [refreshNow]);
  useEffect(() => {
    void refresh().catch((error) => setMessage(String(error)));
    const timer = setInterval(() => void refresh().catch(() => undefined), 3_000);
    return () => clearInterval(timer);
  }, [refresh]);
  useRealtime("margins-recording", () => void refresh().catch(() => undefined));
  useEffect(() => {
    if (!starting || !pendingNativeStart.current || startError) return;
    let busy = false;
    const poll = async () => {
      if (busy) return;
      busy = true;
      try {
        try {
          const status = await nativeBridgeOwner.refresh();
          if (pendingNativeStart.current?.accepted && status.state === "needs_attention") {
            setStartError(status.error || "Margins Menu could not start recording.");
          }
        } catch { /* Regular bridge polling reports connection errors. */ }
        if (pendingNativeStart.current) await refresh().catch(() => undefined);
      }
      finally { busy = false; }
    };
    const timer = setInterval(() => void poll(), 500);
    return () => clearInterval(timer);
  }, [starting, startError, refresh]);

  useEffect(() => {
    if (!projectId || !selectedId) { setMeeting(null); return; }
    let cancelled = false;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
    const retry = () => { if (!cancelled) retryTimer = setTimeout(() => void read(), 1_500); };
    const read = async () => {
      try {
        const result = await rpc.call("readWorkspaceMeeting", { projectId, sessionId: selectedId });
        if (cancelled) return;
        if (!result.ok || !result.meeting) {
          setMessage(result.ok ? "Meeting memo is unavailable." : result.error.message);
          retry();
          return;
        }
        rememberMeeting(`${projectId}/${selectedId}`, result.meeting);
        setMeeting(result.meeting);
        revision.current = result.meeting.notepad.revision;
        if (!dirty.current) {
          setDraft(result.meeting.notepad.text);
          latestDraft.current = result.meeting.notepad.text;
        }
      } catch (error) {
        if (cancelled) return;
        setMessage(String(error));
        retry();
      }
    };
    void read();
    return () => { cancelled = true; if (retryTimer) clearTimeout(retryTimer); };
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
        rememberMeeting(`${projectId}/${sessionId}`, result.meeting);
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
  }, [draft, meeting?.sessionId]);

  async function choose(sessionId: string) {
    try { await saveMemo(); } catch (error) { setMessage(String(error)); return; }
    const cached = openedMeetings.get(`${projectId}/${sessionId}`) || null;
    dirty.current = false;
    revision.current = cached?.notepad.revision || "";
    latestDraft.current = cached?.notepad.text || "";
    setMeeting(cached);
    setDraft(cached?.notepad.text || "");
    setSelectedId(sessionId);
    setMoreOpen(false); setEditingTitle(false); setTranscriptOpen(false); setTranscriptBody("");
    navigate.toPluginPanel("meetings", { subPath: `${projectId}/${sessionId}` });
  }
  async function chooseProject(nextProjectId: string) {
    try { await saveMemo(); } catch (error) { setMessage(String(error)); return; }
    dirty.current = false; latestDraft.current = ""; revision.current = "";
    setProjectId(nextProjectId); setSelectedId(null); setMeeting(null); setDraft(""); setMessage("");
    setSetupPreview(null);
    setShowArchived(false); setMoreOpen(false); setEditingTitle(false); setTranscriptOpen(false);
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
          rememberMeeting(`${projectId}/${sessionId}`, latest.meeting);
          setMeeting(latest.meeting); setDraft(latest.meeting.notepad.text);
          latestDraft.current = latest.meeting.notepad.text;
          revision.current = latest.meeting.notepad.revision;
        }
      }
      return true;
    } catch (error) { setMessage(String(error)); return false; }
  }
  async function settleIncompleteCapture() {
    try {
      const result = browserCaptureOwner.canFinishIncomplete
        ? await browserCaptureOwner.finishIncomplete() : await browserCaptureOwner.retryPendingStop();
      if (result?.error) setMessage(result.error.message);
      else if (result?.state === "saved") { setMessage(""); await refresh(); }
    } catch (error) { setMessage(String(error)); }
  }
  async function start() {
    if (!projectId) return;
    if (nativeBridgeOwner.paired) {
      try {
        const native = await nativeBridgeOwner.refresh();
        if (["getting_ready", "recording", "paused"].includes(native.state)) {
          pendingNativeStart.current = null;
          setStarting(false);
          setStartError("");
          if (native.sessionId) {
            const joined = await rpc.call("readWorkspaceMeeting", { projectId, sessionId: native.sessionId });
            if (joined.ok && joined.meeting) {
              try { await saveMemo(); }
              catch (error) { setMessage(String(error)); return; }
              rememberMeeting(`${projectId}/${native.sessionId}`, joined.meeting);
              setMeeting(joined.meeting);
              const joinedDraft = joined.meeting.notepad.text || pendingDraftText.current;
              dirty.current = joinedDraft !== joined.meeting.notepad.text;
              setDraft(joinedDraft);
              latestDraft.current = joinedDraft;
              revision.current = joined.meeting.notepad.revision;
              setSelectedId(native.sessionId);
              navigate.toPluginPanel("meetings", { subPath: `${projectId}/${native.sessionId}` });
              return;
            }
          }
          setMessage("Margins Menu is already recording. Waiting for its meeting to appear here…");
          await refresh();
          return;
        }
      } catch { /* The normal Connect flow below reports an unavailable bridge. */ }
    }
    const live = meetings.find((item) => !item.inputFinalized);
    if (live && !window.confirm("Stop and save the current meeting, then start?")) return;
    if (live && !await control("stop", live.sessionId)) return;
    const nativeStart = client.platform === "macos" || menuAvailable;
    const retrying = starting;
    if (nativeStart) {
      pendingNativeStart.current = { projectId, existing: new Set(meetings.map((item) => item.sessionId)), startedAt: Date.now(), accepted: false };
      setStarting(true);
      setStartError("");
      if (!retrying) { pendingDraftText.current = ""; setPendingDraft(""); }
    }
    try {
      await saveMemo();
      setMessage("");
      if (nativeStart) {
        if (!nativeBridgeOwner.paired) await connectMenu();
        const authority = await rpc.call("captureAuthority", { projectId });
        if (!authority.ok) throw new Error(authority.error.message);
        try { await nativeBridgeOwner.verify(authority); }
        catch {
          // A Menu restart invalidates the bridge, while sessionStorage still holds its pairing.
          await connectMenu();
          await nativeBridgeOwner.verify(authority);
        }
        try { sessionStorage.setItem(LAST_PROJECT_KEY, projectId); } catch { /* private browser */ }
        await nativeBridgeOwner.control("start");
        if (pendingNativeStart.current) pendingNativeStart.current.accepted = true;
        await refresh();
        return;
      }
      await startBrowserMicrophone();
    } catch (error) {
      pendingNativeStart.current = null;
      if (nativeStart) {
        const reason = error instanceof Error ? error.message : String(error);
        if (reason.includes("capture_already_active")) {
          setStarting(false);
          setStartError("");
          setMessage("Margins Menu is already recording. Waiting for its meeting to appear here…");
          void refresh().catch(() => undefined);
        } else setStartError(reason);
      }
      else setMessage(error instanceof Error ? error.message : String(error));
    }
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
      if (next.sessionId) {
        setSelectedId(next.sessionId);
        navigate.toPluginPanel("meetings", { subPath: `${projectId}/${next.sessionId}` });
      }
      await refresh();
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
  async function previewSetup() {
    if (!projectId) return;
    setSetupBusy(true); setMessage("");
    try { setSetupPreview(await rpc.call("previewWorkspaceSetup", { projectId, homeRoot: setupHome })); }
    catch (error) { setMessage(error instanceof Error ? error.message : String(error)); }
    finally { setSetupBusy(false); }
  }
  async function applySetup() {
    if (!projectId || !setupPreview) return;
    setSetupBusy(true); setMessage("");
    try {
      await rpc.call("applyWorkspaceSetup", { projectId, previewId: setupPreview.previewId });
      setSetupPreview(null);
      await refresh();
    } catch (error) { setMessage(error instanceof Error ? error.message : String(error)); }
    finally { setSetupBusy(false); }
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
  async function viewTranscript() {
    if (!selectedId || !projectId) return;
    if (transcriptOpen) { setTranscriptOpen(false); return; }
    const result = await rpc.call("readWorkspaceTranscript", { projectId, sessionId: selectedId });
    if (!result.ok) { setMessage(result.error.message); return; }
    setTranscriptBody(result.body.trim());
    setTranscriptOpen(true);
  }
  async function renameMeeting() {
    if (!selectedId || !projectId || !titleDraft.trim()) return;
    setActionBusy(true);
    try {
      await saveMemo();
      const result = await rpc.call("renameWorkspaceMeeting", { projectId, sessionId: selectedId, title: titleDraft.trim() });
      if (!result.ok) throw new Error(result.error.message);
      setMeetings((current) => current.map((item) => item.sessionId === selectedId ? { ...item, title: titleDraft.trim() } : item));
      setMeeting((current) => current?.sessionId === selectedId ? { ...current, title: titleDraft.trim() } : current);
      setEditingTitle(false);
      await refresh();
    } catch (error) { setMessage(String(error)); }
    finally { setActionBusy(false); }
  }
  async function archiveMeeting(archived: boolean) {
    if (!selectedId || !projectId) return;
    setActionBusy(true);
    try {
      await saveMemo();
      const result = await rpc.call("archiveWorkspaceMeeting", { projectId, sessionId: selectedId, archived });
      if (!result.ok) throw new Error(result.error.message);
      setMoreOpen(false);
      setMeetings((current) => current.map((item) => item.sessionId === selectedId ? { ...item, archived } : item));
      if (archived) {
        const next = meetings.find((item) => item.sessionId !== selectedId && !item.archived)?.sessionId || null;
        setSelectedId(next);
        navigate.toPluginPanel("meetings", { subPath: next ? `${projectId}/${next}` : projectId });
      }
      await refresh();
    } catch (error) { setMessage(String(error)); }
    finally { setActionBusy(false); }
  }
  async function discardMeeting() {
    if (!selectedId || !selected?.inputFinalized || !projectId) return;
    const label = selected.title || `Meeting · ${meetingTime(selected.startedAt)}`;
    if (!window.confirm(`Permanently discard “${label}”? This removes its recording, transcript, and memo. Any linked note and bb thread remain, but their meeting source link will no longer work.`)) return;
    setActionBusy(true);
    try {
      await saveMemo();
      const result = await rpc.call("discardWorkspaceMeeting", { projectId, sessionId: selectedId });
      if (!result.ok) throw new Error(result.error.message);
      openedMeetings.delete(`${projectId}/${selectedId}`);
      openedSummaries.delete(`${projectId}/${selectedId}`);
      setMoreOpen(false);
      setMeeting(null); setDraft(""); dirty.current = false; revision.current = ""; latestDraft.current = "";
      const remaining = meetings.filter((item) => item.sessionId !== selectedId);
      setMeetings(remaining);
      const next = remaining.find((item) => !item.archived)?.sessionId || null;
      setSelectedId(next);
      navigate.toPluginPanel("meetings", { subPath: next ? `${projectId}/${next}` : projectId });
      await refresh();
    } catch (error) { setMessage(String(error)); }
    finally { setActionBusy(false); }
  }
  async function distill() {
    if (!selected || !projectId || noteBusy) return;
    setNoteBusy(true);
    try {
      await saveMemo();
      const noteProjectId = selected.originProjectId || projectId;
      const result = await rpc.call("connectedNoteContext", { projectId: noteProjectId, sessionId: selected.sessionId });
      if (!result.ok) throw new Error(result.error.message);
      const { sessionId } = result.context;
      if (!result.context.transcript.available) {
        const requested = await rpc.call("transcribePinnedSession", { projectId: noteProjectId, sessionId });
        if (!requested.ok) { setTranscriptStatus({ sessionId, state: "failed" }); throw new Error(requested.error.message); }
        setTranscriptStatus({ sessionId, state: requested.status === "complete" ? "ready" : "pending" });
      }
      const started = await rpc.call("startConnectedNoteThread", { projectId: noteProjectId, sessionId });
      navigate.toThread(started.threadId);
    } catch (error) { setMessage(String(error)); }
    finally { setNoteBusy(false); }
  }

  const live = meetings.filter((item) => !item.inputFinalized);
  const preparing = meetings.filter((item) => item.inputFinalized && !item.archived && !item.notePath && (item.threadIds?.length || 0) > 0);
  const ready = meetings.filter((item) => item.inputFinalized && !item.archived && !item.notePath && !item.threadIds?.length);
  const distilled = meetings.filter((item) => item.inputFinalized && !item.archived && item.notePath);
  const archived = meetings.filter((item) => item.archived);
  const pausedSession = (sessionId: string) => panel?.sessionId === sessionId && panel.state === "paused"
    || nativeBridgeOwner.status?.sessionId === sessionId && nativeBridgeOwner.status.state === "paused";
  const memoChangedSinceNote = Boolean(selected?.notePath && meeting &&
    (selected.distilledMemoRevision !== meeting.notepad.revision || dirty.current));
  const noteAction = selected?.inputFinalized && (!selected.notePath || memoChangedSinceNote);
  const threadLinks = selected?.threadLinks?.length ? selected.threadLinks
    : (selected?.threadIds || []).map((id) => ({ id, title: "Meeting note thread" }));
  const shownTranscript = transcriptStatus?.sessionId === selectedId ? transcriptStatus.state : "checking";
  const localCapturePanel = browserCaptureOwner.panel();
  const pendingBrowserStop = Boolean(selected && browserCaptureOwner.hasPendingStop
    && browserCaptureOwner.recordingId === selected.sessionId);
  const missingAudio = selected?.captureGaps?.map((gap) => gap.endExclusive > gap.startSequence
    ? `${gap.segmentId}: ${gap.startSequence}–${gap.endExclusive - 1}`
    : `${gap.segmentId}: capture interrupted`).join(", ");
  const groups = [["Live", live], ["Preparing note", preparing], ["Ready to refine", ready], ["Distilled", distilled], ["Archived", showArchived ? archived : []]] as const;
  return <main className="margins-meetings-page">
    <aside className="margins-meeting-list">
      <div className="margins-meetings-top"><strong>Meetings</strong>
        <button className={meetings.length === 0 || live.length > 0 || starting ? "is-reserved" : ""} aria-hidden={meetings.length === 0 || live.length > 0 || starting}
          tabIndex={meetings.length === 0 || live.length > 0 || starting ? -1 : undefined}
          onClick={() => void start()} disabled={panel?.state === "unavailable" || meetings.length === 0 || live.length > 0 || starting}>Start</button></div>
      <select aria-label="bb project for Meetings" value={projectId} disabled={live.length > 0 || starting} onChange={(event) => void chooseProject(event.target.value)}>{projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}</select>
      {groups.map(([label, items]) => items.length > 0 && <section key={label}>
        <h3>{label}</h3>{items.map((item) => <button key={item.sessionId} className={item.sessionId === selectedId ? "selected" : ""}
          onClick={() => void choose(item.sessionId)}><span>{item.title || meetingListTitle(item.startedAt)}</span>
          {!item.inputFinalized && pausedSession(item.sessionId) && <small>Paused</small>}
          {item.captureIncomplete && <small>Incomplete</small>}</button>)}
      </section>)}
      {archived.length > 0 && <button className="margins-archive-toggle" onClick={() => {
        if (showArchived && selected?.archived) {
          const next = meetings.find((item) => !item.archived)?.sessionId || null;
          setSelectedId(next);
          navigate.toPluginPanel("meetings", { subPath: next ? `${projectId}/${next}` : projectId });
        }
        setShowArchived(!showArchived);
      }}>{showArchived ? "Hide archived" : `Archived (${archived.length})`}</button>}
    </aside>
    <section className={`margins-meeting-pad${selected?.inputFinalized ? " finished" : ""}`}>
      {panel && panel.state !== "unavailable" && (client.platform === "macos" || menuAvailable) && live.length === 0 && <div className="margins-menu-connect">
        <span>{nativeBridgeOwner.paired && nativeConnectionError
          ? "Margins Menu disconnected"
          : nativeBridgeOwner.paired
          ? `Margins Menu connected · ${nativeStatus?.microphoneDeviceName || "Microphone"} + computer audio`
          : "Record with Margins Menu (mic + computer audio)"}</span>
        {(!nativeBridgeOwner.paired || nativeConnectionError) && <button disabled={menuBusy || !projectId} onClick={() => void connectMenu().catch(() => undefined)}>{nativeBridgeOwner.paired ? "Reconnect" : "Connect"}</button>}
        {client.platform === "macos" && !nativeBridgeOwner.paired && <button disabled={menuBusy || !projectId} onClick={() => void startBrowserMicrophone()}>Browser mic only</button>}
      </div>}
      {panel && panel.state !== "unavailable" && speechSetup && speechSetup.state !== "ready" && <div className="margins-workspace-notice" role="status">
        {speechSetup.state === "preparing" ? "Preparing transcription on this project's machine. You can record now; saved audio will be transcribed when it is ready."
          : speechSetup.state === "failed" ? `Transcription setup needs attention: ${speechSetup.message}`
            : speechSetup.message}
        {speechSetup.state === "preparing" && <span> {speechSetup.message}{speechSetup.progress !== null ? ` · ${Math.round(speechSetup.progress * 100)}%` : ""}</span>}
        {speechSetup.state === "failed" && <button onClick={() => {
          setSpeechSetup({ state: "preparing", message: "Retrying transcription setup", progress: null });
          void rpc.call("retrySpeechSetup", { projectId }).then((result) => {
            if (result.ok) setSpeechSetup({ state: result.state, message: result.message, progress: result.progress });
            else setSpeechSetup({ state: "failed", message: result.error.message, progress: null });
          }).catch((error) => setSpeechSetup({ state: "failed", message: String(error), progress: null }));
        }}>Retry</button>}
      </div>}
      {panel?.state === "unavailable" && <div className="margins-meetings-empty">
        <h2>{workspaceOptions.length ? "Choose a Margins Workspace" : "Set up a Margins Workspace"}</h2>
        {workspaceOptions.length ? <div><select aria-label="Margins Workspace" value={workspaceChoice} onChange={(event) => setWorkspaceChoice(event.target.value)}>
          {workspaceOptions.map((item) => <option key={item.id} value={item.id}>{item.name || item.id}</option>)}
        </select><button onClick={() => void chooseWorkspace()} disabled={!workspaceChoice}>Use Workspace</button></div>
          : <div className="margins-setup"><p>Choose your notes project above. Margins keeps recordings in its own store.</p>
            <p>Continue starts from the Margins meetings preset: meeting notes, people, and projects.</p>
            <label>Notes folder <input aria-label="Workspace notes folder" value={setupHome} placeholder="Use this project if it is an Obsidian vault" disabled={setupBusy}
              onChange={(event) => { setSetupHome(event.target.value); setSetupPreview(null); }} /></label>
            {!setupPreview ? <button disabled={setupBusy} onClick={() => void previewSetup()}>{setupBusy ? "Preparing…" : "Continue →"}</button>
              : <div className="margins-setup-preview"><p>Workspace: {setupPreview.workspaceId}</p><p>Notes will go to {setupPreview.destination}</p>
                <p>{setupPreview.readings.length ? `Learns from ${setupPreview.readings.map(readingLabel).join(" · ")}` : "Chooses what to learn from automatically"}</p>
                {setupPreview.skippedReadings.length > 0 && <p>Skipped, not in your notes: {setupPreview.skippedReadings.map(readingLabel).join(" · ")}</p>}
                <p>Settings: {setupPreview.programPath} · change them later with <code>margins workspace edit</code></p>
                <details><summary>Exact Workspace changes</summary><pre>{JSON.stringify(setupPreview.actions, null, 2)}</pre></details>
                <button disabled={setupBusy} onClick={() => void applySetup()}>{setupBusy ? "Saving…" : "Use this Workspace"}</button>
                <button disabled={setupBusy} onClick={() => setSetupPreview(null)}>Change</button></div>}</div>}
        {message && <p role="alert">{message}</p>}
      </div>}
      {panel?.state !== "unavailable" && starting ? <div className="margins-preparing-pad">
        <header><div><span className="margins-meeting-kicker" role="status">{startError ? "Recording could not start" : nativeStatus?.state === "recording" ? "Recording" : "Preparing audio…"}</span>
          <h2>New meeting</h2><p className="margins-meeting-details">{nativeBridgeOwner.paired ? `${nativeStatus?.microphoneDeviceName || "Microphone"} + computer audio` : "Browser microphone"}{resolvedWorkspaceName && ` · Workspace: ${resolvedWorkspaceName}`} · Started from {projects.find((item) => item.id === projectId)?.name || "this project"}</p></div>{startError && <button onClick={() => void start()}>Retry Start</button>}</header>
        {startError && <p role="alert">{startError}</p>}
        <textarea aria-label="Meeting memo pad" placeholder="Write notes..." autoFocus value={pendingDraft}
          onChange={(event) => { pendingDraftText.current = event.target.value; setPendingDraft(event.target.value); }} />
      </div>
        : panel?.state !== "unavailable" && selected && meeting?.sessionId === selected.sessionId ? <>
        {workspaceNotice && <p className="margins-workspace-notice">{workspaceNotice}</p>}
        <header><div>{(!selected.inputFinalized || memoChangedSinceNote || selected.notePath || message && message !== "Saved") && <div className="margins-meeting-meta">
          {(!selected.inputFinalized || memoChangedSinceNote) && <span className="margins-meeting-kicker">
          {!selected.inputFinalized ? nativeStatus?.sessionId === selected.sessionId && nativeStatus.state === "saving" ? "Saving recording…"
            : audioStartingId === selected.sessionId && nativeStatus?.state === "needs_attention" ? "Audio needs attention"
              : audioStartingId === selected.sessionId && !(nativeStatus?.sessionId === selected.sessionId && ["recording", "paused"].includes(nativeStatus.state)) ? "Preparing audio…"
                : <><i className={`margins-meeting-state-dot${pausedSession(selected.sessionId) ? " paused" : ""}`} aria-hidden="true" />{pausedSession(selected.sessionId) ? "Paused" : "Recording"}</>
            : "Memo updated since note"}</span>}
          {selected.notePath && <nav className="margins-meeting-links" aria-label="Meeting links">
            {selected.noteFile ? <FileLink title="Open note preview" target={{ kind: "host", ...selected.noteFile }} location={null}>{noteTitle(selected.notePath)}</FileLink>
              : <span title="File preview is unavailable on this host">{noteTitle(selected.notePath)}</span>}
            {threadLinks.map(({ id, title }) => <button key={id} onClick={() => navigate.toThread(id)}>{title}</button>)}
          </nav>}
          {message && (message !== "Saved" || !selected.inputFinalized) && <span className={`margins-meeting-status${message === "Saved" ? " saved" : " error"}`} role="status">{message}</span>}</div>}
          <div className="margins-meeting-title-row">
            {editingTitle ? <><input aria-label="Meeting title" maxLength={160} value={titleDraft} autoFocus
              onChange={(event) => setTitleDraft(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") void renameMeeting(); if (event.key === "Escape") setEditingTitle(false); }} />
              <button disabled={actionBusy || !titleDraft.trim()} onClick={() => void renameMeeting()}>Save</button>
              <button onClick={() => setEditingTitle(false)}>Cancel</button></>
              : <><h2>{meeting.title || `Meeting · ${meetingTime(meeting.startedAt)}`}</h2>
                {selected.inputFinalized && <button className="margins-inline-action" onClick={() => { setTitleDraft(meeting.title || `Meeting · ${meetingTime(meeting.startedAt)}`); setEditingTitle(true); }}>Rename</button>}</>}
          </div>
          <p className="margins-meeting-details">{selected.inputFinalized
            ? stopAck?.sessionId === selected.sessionId ? `Saved · ${stopAck.elapsed} recorded` : `Saved${selected.durationMs !== null && selected.durationMs !== undefined ? ` · ${elapsedLabel(selected.durationMs)}` : ""}`
            : "Recording"}
            {!selected.inputFinalized && selected.audioSource && ` · ${selected.audioSource}`}
            {selected.workspaceName && ` · ${selected.inputFinalized ? selected.workspaceName : `Workspace: ${selected.workspaceName}`}`}
            {selected.originProjectName && selected.inputFinalized && selected.originProjectName.toLowerCase() !== selected.workspaceName?.toLowerCase() && ` · from ${selected.originProjectName}`}
            {!selected.inputFinalized && (selected.originProjectName
              ? ` · Started from ${selected.originProjectName}`
              : nativeStatus?.sessionId === selected.sessionId && audioStartingId !== selected.sessionId
                ? " · Started in Margins Menu"
                : projects.find((item) => item.id === projectId)?.name
                  ? ` · Started from ${projects.find((item) => item.id === projectId)?.name}` : "")}
            {selected.inputFinalized && message === "Saved" && <span className="margins-meeting-status saved" role="status"> · Memo updated</span>}</p></div>
        </header>
        {!selected.inputFinalized && !selected.originProjectId && audioStartingId !== selected.sessionId
          && nativeStatus?.sessionId === selected.sessionId &&
          <p className="margins-workspace-notice">Taking notes on the meeting already recording in Margins Menu.</p>}
        {pendingBrowserStop && <div className="margins-workspace-notice" role="alert">
          <span>{localCapturePanel?.error?.message || "Some browser audio may be missing."}</span>
          {(browserCaptureOwner.canFinishIncomplete || localCapturePanel?.primaryAction === "retry") && <button onClick={() => void settleIncompleteCapture()}>
            {browserCaptureOwner.canFinishIncomplete ? "Finish with what was saved" : "Try again"}
          </button>}
        </div>}
        {selected.captureIncomplete && <p className="margins-workspace-notice" role="alert">
          Recording incomplete. {missingAudio ? `Audio gaps: ${missingAudio}.` : "Some audio could not be decoded or saved."}
        </p>}
        {audioStartingId === selected.sessionId && nativeStatus?.state === "needs_attention" &&
          <p role="alert">{nativeStatus.error || "Margins Menu could not start recording."}</p>}
        <textarea ref={memoRef} aria-label="Meeting memo pad" placeholder="Write notes..." value={draft} onChange={(event) => { dirty.current = true; latestDraft.current = event.target.value; setDraft(event.target.value); setMessage(""); }} onBlur={() => void saveMemo().catch((error) => setMessage(String(error)))} />
        <footer className={selected.inputFinalized ? "margins-meeting-actions" : undefined}>
          {selected.inputFinalized && <div className="margins-meeting-trail">{shownTranscript !== "ready" && <span>{shownTranscript === "pending" ? "Transcribing…" : shownTranscript === "checking" ? "Checking transcript…" : shownTranscript === "failed" ? "Transcript unavailable" : "Transcript not ready"}</span>}
            {shownTranscript === "ready" && <button onClick={() => void viewTranscript()}>{transcriptOpen ? "Hide transcript" : "View transcript"}</button>}
            {(shownTranscript === "failed" || shownTranscript === "not_ready") && <button disabled={speechSetup?.state === "preparing"} onClick={() => void retryTranscription()}>{shownTranscript === "failed" ? "Retry" : "Transcribe"}</button>}
          </div>}
          {transcriptOpen && <div className="margins-meeting-transcript" aria-label="Meeting transcript">
            {selected.captureIncomplete && <p>Incomplete transcript: some recorded audio is missing.</p>}
            {transcriptBody || "Transcript is empty."}
          </div>}
          {memoChangedSinceNote && <span>Note uses an earlier memo revision</span>}
          {selected.inputFinalized && <div className="margins-meeting-next">
            {noteAction && <button disabled={noteBusy} onClick={() => void distill()}>{noteBusy ? "Starting note thread…" : selected.notePath ? "Update note" : selected.threadIds?.length ? "Open note thread" : "Make note"} →</button>}
            <div className="margins-meeting-more"><button className="margins-inline-action" aria-expanded={moreOpen} onClick={() => setMoreOpen(!moreOpen)}>More</button>
              {moreOpen && <div className="margins-meeting-more-menu">
                <button disabled={actionBusy} onClick={() => void archiveMeeting(!selected.archived)}>{selected.archived ? "Restore to recent" : "Archive"}</button>
                <button className="destructive" disabled={actionBusy} onClick={() => void discardMeeting()}>Discard permanently…</button>
              </div>}
            </div>
          </div>}
        </footer>
      </> : selectedId && panel?.state !== "unavailable" ? <div className="margins-meetings-empty" role="status"><h2>{selected?.title || (selected ? `Meeting · ${meetingTime(selected.startedAt)}` : "Opening meeting…")}</h2><p>Opening memo…</p>{message && <p role="alert">{message}</p>}</div>
      : panel?.state !== "unavailable" && <div className="margins-meetings-empty"><h2>{archived.length ? "No recent meetings" : "No meetings yet"}</h2>{workspaceNotice && <p>{workspaceNotice}</p>}{resolvedWorkspaceName && <p>Save to {resolvedWorkspaceName} · Started from {projects.find((item) => item.id === projectId)?.name || "this project"}</p>}{meetings.length === 0 && <button onClick={() => void start()}>Start meeting</button>}
        {message && <p role="alert">{message}</p>}</div>}
    </section>
  </main>;
}
