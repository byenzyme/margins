import { useEffect, useRef, useState } from "react";
import { useBbContext, useBbNavigate, useRpc } from "@get-bb/plugin-sdk/app";
import type { marginsRpcContract } from "../server.js";
import type { WorkspaceMeeting, WorkspaceMeetingSummary } from "./contracts.js";

export function MarginsThreadTab({ threadId }: { threadId: string }) {
  const rpc = useRpc<typeof marginsRpcContract>();
  const navigate = useBbNavigate();
  const { projectId } = useBbContext();
  const [summary, setSummary] = useState<WorkspaceMeetingSummary | null>(null);
  const [meeting, setMeeting] = useState<WorkspaceMeeting | null>(null);
  const [draft, setDraft] = useState("");
  const [message, setMessage] = useState("");
  const revision = useRef("");
  const dirty = useRef(false);
  useEffect(() => {
    if (!projectId) return;
    let cancelled = false;
    void rpc.call("listWorkspaceMeetings", { projectId }).then(async (result) => {
      if (!result.ok) throw new Error(result.error.message);
      const linked = result.meetings.find((item) => item.threadIds?.includes(threadId)) || null;
      if (cancelled) return;
      setSummary(linked);
      if (!linked) return;
      const read = await rpc.call("readWorkspaceMeeting", { projectId, sessionId: linked.sessionId });
      if (cancelled || !read.ok || !read.meeting) return;
      setMeeting(read.meeting);
      setDraft(read.meeting.notepad.text);
      revision.current = read.meeting.notepad.revision;
    }).catch((error) => { if (!cancelled) setMessage(String(error)); });
    return () => { cancelled = true; };
  }, [projectId, rpc, threadId]);
  async function save() {
    if (!dirty.current || !meeting || !projectId) return;
    try {
      const result = await rpc.call("saveWorkspaceMemo", { projectId, sessionId: meeting.sessionId,
        expectedRevision: revision.current, text: draft });
      if (!result.ok || !result.meeting) throw new Error(result.ok ? "Meeting unavailable" : result.error.message);
      revision.current = result.meeting.notepad.revision;
      setMeeting(result.meeting);
      dirty.current = false;
      setMessage("Saved");
    } catch (error) { setMessage(String(error)); }
  }
  useEffect(() => {
    if (!dirty.current) return;
    const timer = setTimeout(() => void save(), 800);
    return () => clearTimeout(timer);
  }, [draft]);
  const open = (sessionId?: string) => navigate.toPluginPanel("meetings", { subPath: projectId ? `${projectId}/${sessionId || ""}` : "" });
  if (!summary || !meeting) return <section className="margins-thread-tab">
    <button onClick={() => open()}>Open Meetings</button><p>{message || "Open the live meeting memo pad in Meetings."}</p>
  </section>;
  return <section className="margins-thread-tab">
    <header><h3>{meeting.title || "Meeting memo"}</h3><button onClick={() => open(meeting.sessionId)}>Open in Meetings</button></header>
    <textarea aria-label="Source meeting memo pad" value={draft} onChange={(event) => { dirty.current = true; setDraft(event.target.value); }} onBlur={() => void save()} />
    {summary.notePath && <p>Note: {summary.notePath}</p>}
    {summary.distilledMemoRevision && summary.distilledMemoRevision !== meeting.notepad.revision && <p>Edited since distillation</p>}
    <p role="status">{message}</p>
  </section>;
}
