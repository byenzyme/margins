export type MeetingMention = {
  projectId: string;
  workspaceId: string;
  sessionId: string;
  memoRevision: string;
  note: "create" | "update";
};

export function meetingMentionId(value: MeetingMention): string {
  return encodeURIComponent(JSON.stringify(value));
}

export function parseMeetingMentionId(id: string): MeetingMention {
  if (id.length > 2_048) throw new Error("Meeting reference is too long. Open Make note again.");
  let value: unknown;
  try { value = JSON.parse(decodeURIComponent(id)); }
  catch { throw new Error("Invalid meeting reference. Open Make note again."); }
  if (!value || typeof value !== "object") throw new Error("Invalid meeting reference. Open Make note again.");
  const item = value as Record<string, unknown>;
  if (["projectId", "workspaceId", "sessionId", "memoRevision"].some((key) =>
    typeof item[key] !== "string" || !item[key] || (item[key] as string).length > 300)
    || item.note !== "create" && item.note !== "update") {
    throw new Error("Invalid meeting reference. Open Make note again.");
  }
  return item as MeetingMention;
}
