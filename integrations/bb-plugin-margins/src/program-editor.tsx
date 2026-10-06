import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import { experimental_Diff, experimental_useCodeTheme, useRpc } from "@get-bb/plugin-sdk/app";
import { AlertCircle, Check, X } from "lucide-react";
import type { marginsRpcContract } from "../server.js";
import { MAX_PROGRAM_BYTES, type ProgramError, type ProgramPlanResult, type WorkspaceProgram } from "./contracts.js";
import { offsetOf, tokenizeEnzyme, type EnzymeTokenClass } from "./enzyme-highlight.js";

const VALIDATE_DELAY_MS = 450;
type Plan = Extract<ProgramPlanResult, { ok: true }>;

/** TextMate scopes, most specific first, whose bb code-theme color paints each class. */
// Bare words (keywords) and punctuation stay a muted foreground: themes often
// paint keywords red, which made a valid program look like an error.
const THEME_SCOPES: Partial<Record<EnzymeTokenClass, string[]>> = {
  string: ["string.quoted", "string"],
  number: ["constant.numeric", "constant"],
  comment: ["comment.line", "comment"],
  kind: ["entity.name.type", "support.type", "storage.type"],
  profile: ["entity.name.function", "support.function", "variable"],
};

type CodeTheme = ReturnType<typeof experimental_useCodeTheme>;
// Older bb hosts predate the code-theme hook and the diff component; the
// editor then uses its own light/dark palette and a plain diff.
const useCodeTheme: () => CodeTheme | null = typeof experimental_useCodeTheme === "function" ? experimental_useCodeTheme : () => null;

function themeColor(theme: NonNullable<CodeTheme["theme"]>, candidates: string[]): string | undefined {
  for (const wanted of candidates) {
    let best: { length: number; color: string } | null = null;
    for (const rule of theme.tokenColors) {
      const color = rule.settings.foreground;
      if (!color || !rule.scope) continue;
      const scopes = (typeof rule.scope === "string" ? rule.scope.split(",") : rule.scope).map((item) => item.trim());
      for (const scope of scopes) {
        // A rule for `string` paints `string.quoted`; a rule for a descendant
        // selector or a narrower scope does not paint the broader one.
        if (scope.includes(" ")) continue;
        if ((wanted === scope || wanted.startsWith(`${scope}.`)) && (!best || scope.length > best.length)) best = { length: scope.length, color };
      }
    }
    if (best) return best.color;
  }
  return undefined;
}

/** CSS variables carrying the live bb code theme into the editor. */
export function codeThemeStyle(state: CodeTheme | null): CSSProperties {
  const theme = state?.theme;
  if (!theme) return {};
  const style: Record<string, string> = {
    "--enz-fg": theme.fg, "--enz-bg": theme.colors["editor.background"] || theme.bg,
    "--enz-gutter": theme.colors["editorLineNumber.foreground"] || "",
    "--enz-caret": theme.colors["editorCursor.foreground"] || theme.fg,
    "--enz-selection": theme.colors["editor.selectionBackground"] || "",
  };
  for (const [cls, scopes] of Object.entries(THEME_SCOPES)) {
    const color = scopes && themeColor(theme, scopes);
    if (color) style[`--enz-${cls}`] = color;
  }
  return Object.fromEntries(Object.entries(style).filter(([, value]) => value)) as CSSProperties;
}

/** A short label for the inline callout; the full message is under the editor. */
export function calloutText(message: string): string | null {
  const found = /; found "(.*)"$/.exec(message);
  if (found) return found[1] === "<end>" ? "Unexpected end of program" : `Unexpected "${found[1]}"`;
  return message.length <= 40 ? message : null;
}

export function EnzymeCodeEditor({ value, onChange, error, label, textareaRef, describedBy }: {
  value: string; onChange: (value: string) => void; label: string;
  error: { line: number; column: number; message: string } | null;
  textareaRef?: React.RefObject<HTMLTextAreaElement | null>;
  describedBy?: string;
}) {
  const codeTheme = useCodeTheme();
  const tokens = useMemo(() => tokenizeEnzyme(value), [value]);
  const lineCount = useMemo(() => value.split("\n").length, [value]);
  const ownArea = useRef<HTMLTextAreaElement | null>(null);
  const area = textareaRef || ownArea;
  const boxRef = useRef<HTMLDivElement | null>(null);
  const gutterRef = useRef<HTMLDivElement | null>(null);
  const measureRef = useRef<HTMLSpanElement | null>(null);
  // Escape, then Tab, leaves the editor (WCAG 2.1.2); Tab alone indents.
  const releaseTab = useRef(false);
  const errorOffset = error ? offsetOf(value, error) : -1;
  // The token the error points at gets the squiggle; on whitespace or a line
  // end, the next token does.
  const errorToken = error ? tokens.findIndex((token) => token.cls !== null && token.start + token.text.length > errorOffset) : -1;
  const callout = error ? calloutText(error.message) : null;

  /** The text layers are as large as the text and the outer box scrolls, so
   * keep the caret inside the visible part of that box ourselves. */
  const revealCaret = useCallback(() => {
    const textarea = area.current, box = boxRef.current, measure = measureRef.current, gutter = gutterRef.current;
    if (!textarea || !box || !measure || !gutter || document.activeElement !== textarea) return;
    const caret = textarea.selectionDirection === "backward" ? textarea.selectionStart : textarea.selectionEnd;
    const before = textarea.value.slice(0, caret);
    const line = before.split("\n").length - 1;
    const column = caret - (before.lastIndexOf("\n") + 1);
    const style = getComputedStyle(textarea);
    const lineHeight = parseFloat(style.lineHeight) || 20;
    const padTop = parseFloat(style.paddingTop) || 0;
    const padLeft = parseFloat(style.paddingLeft) || 0;
    const charWidth = measure.getBoundingClientRect().width / 10 || 7.5;
    const top = padTop + line * lineHeight;
    if (top < box.scrollTop) box.scrollTop = Math.max(0, top - padTop);
    else if (top + lineHeight + padTop > box.scrollTop + box.clientHeight) box.scrollTop = top + lineHeight + padTop - box.clientHeight;
    const x = padLeft + column * charWidth;
    const visible = box.clientWidth - gutter.offsetWidth;
    if (x - charWidth < box.scrollLeft) box.scrollLeft = Math.max(0, x - padLeft - 4 * charWidth);
    else if (x + 2 * charWidth > box.scrollLeft + visible) box.scrollLeft = x + 4 * charWidth - visible;
  }, [area]);
  useLayoutEffect(revealCaret, [value, revealCaret]);

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Escape") { releaseTab.current = true; return; }
    const release = releaseTab.current;
    releaseTab.current = false;
    if (event.key !== "Tab" || event.shiftKey || event.metaKey || event.ctrlKey || event.altKey || release) return;
    event.preventDefault();
    const target = event.currentTarget;
    // insertText keeps the browser's undo history; setting the value would erase it.
    if (typeof document.execCommand === "function" && document.execCommand("insertText", false, "  ")) return;
    const { selectionStart, selectionEnd } = target;
    onChange(`${value.slice(0, selectionStart)}  ${value.slice(selectionEnd)}`);
    requestAnimationFrame(() => target.setSelectionRange(selectionStart + 2, selectionStart + 2));
  }
  return <div ref={boxRef} className={`margins-code${codeTheme?.mode === "dark" ? " is-dark" : ""}`} style={codeThemeStyle(codeTheme)} data-theme-mode={codeTheme?.mode}>
    <div ref={gutterRef} className="margins-code-gutter" aria-hidden="true">
      {Array.from({ length: lineCount }, (_, index) => <span key={index}
        className={error?.line === index + 1 ? "is-error" : undefined}>{index + 1}</span>)}
    </div>
    <div className="margins-code-body">
      <span ref={measureRef} className="margins-code-measure" aria-hidden="true">0000000000</span>
      {error && <div className="margins-code-error-line" aria-hidden="true" style={{ "--enz-line": error.line - 1 } as CSSProperties} />}
      <pre aria-hidden="true">{tokens.map((token, index) => token.cls
        ? <span key={token.start} className={`enz-${token.cls}${index === errorToken ? " enz-error" : ""}`} data-token={token.cls}>{token.text}</span>
        : token.text)}{"\n"}</pre>
      <textarea ref={area} aria-label={label} spellCheck={false} autoCapitalize="off" autoCorrect="off" autoComplete="off"
        wrap="off" value={value} aria-invalid={error ? true : undefined} aria-describedby={describedBy}
        onChange={(event) => onChange(event.target.value)} onKeyDown={onKeyDown} onSelect={revealCaret}
        // The layer always fits its text; never let it scroll out of register with the highlighting.
        onScroll={(event) => { event.currentTarget.scrollTop = 0; event.currentTarget.scrollLeft = 0; }} />
      {error && callout && <div className="margins-code-error-callout" aria-hidden="true"
        style={{ "--enz-line": error.line - 1, "--enz-col": (value.split("\n")[error.line - 1]?.length ?? 0) + 3 } as CSSProperties}>
        <span>{callout}</span>
      </div>}
    </div>
  </div>;
}

interface Draft { text: string; baseRevision: string; setAside: { text: string; base: string } | null }
function draftKey(projectId: string, workspaceId: string) { return `margins.program-draft.${projectId}.${workspaceId}`; }
function readDraft(projectId: string, workspaceId: string): Draft | null {
  try {
    const parsed = JSON.parse(localStorage.getItem(draftKey(projectId, workspaceId)) || "null") as
      { text?: unknown; baseRevision?: unknown; setAside?: { text?: unknown; base?: unknown } | null } | null;
    if (!parsed || typeof parsed.text !== "string" || typeof parsed.baseRevision !== "string") return null;
    const aside = parsed.setAside;
    return { text: parsed.text, baseRevision: parsed.baseRevision,
      setAside: aside && typeof aside.text === "string" && typeof aside.base === "string" ? { text: aside.text, base: aside.base } : null };
  } catch { return null; }
}
function writeDraft(projectId: string, workspaceId: string, draft: Draft | null) {
  try {
    if (draft) localStorage.setItem(draftKey(projectId, workspaceId), JSON.stringify(draft));
    else localStorage.removeItem(draftKey(projectId, workspaceId));
  } catch { /* storage unavailable: the text stays in the editor */ }
}

/** Once the location is shown, the `invalid desired program: line:column:`
 * prefix only repeats it. */
function errorText(error: ProgramError) {
  return error.message.replace(/^(invalid desired program: )?\d+:\d+: /, "");
}

/** A plain unified diff with added and removed lines marked, for hosts
 * without bb's diff component. */
function PlainDiff({ patch }: { patch: string }) {
  return <pre>{patch.split("\n").map((line, index) => <span key={index} className={line.startsWith("+") && !line.startsWith("+++") ? "is-added"
    : line.startsWith("-") && !line.startsWith("---") ? "is-removed" : undefined}>{line}{"\n"}</span>)}</pre>;
}

type Validation =
  | { state: "idle" }
  | { state: "checking" }
  | { state: "valid"; plan: Plan }
  | { state: "invalid"; error: ProgramError };

/**
 * The selected Workspace's program, editable. Every pause in typing plans the
 * text through `margins workspace plan --desired` (no writes); Review shows
 * that plan and Apply commits exactly it through `margins workspace apply`.
 * The text is the user's: reloads and refusals never replace it without a way
 * back, and an unsaved draft survives closing the panel.
 */
export function ProgramEditor({ projectId, onClose }: { projectId: string; onClose?: () => void }) {
  const rpc = useRpc<typeof marginsRpcContract>();
  const [saved, setSaved] = useState<WorkspaceProgram | null>(null);
  const [loadError, setLoadError] = useState("");
  const [text, setText] = useState("");
  // The revision the user's text was written against. When the saved program
  // moves past it, applying would overwrite a change the user has not seen.
  const [base, setBase] = useState("");
  const [validation, setValidation] = useState<Validation>({ state: "idle" });
  const [review, setReview] = useState<Plan | null>(null);
  const [applying, setApplying] = useState(false);
  const [stale, setStale] = useState(false);
  const [notice, setNotice] = useState("");
  const [setAside, setSetAside] = useState<{ text: string; base: string } | null>(null);
  // Bumped to plan the same text again (an expired review).
  const [recheck, setRecheck] = useState(0);
  // Screen readers hear results, not every "Checking…" while typing.
  const [announcement, setAnnouncement] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const reviewHeading = useRef<HTMLHeadingElement | null>(null);
  const requestSeq = useRef(0);

  const load = useCallback(async () => {
    setLoadError("");
    try {
      const program = await rpc.call("workspaceProgram", { projectId });
      setSaved(program);
      return program;
    } catch (error) {
      setLoadError(error instanceof Error ? error.message : String(error));
      return null;
    }
  }, [rpc, projectId]);

  useEffect(() => {
    let disposed = false;
    void load().then((program) => {
      if (disposed || !program) return;
      const draft = readDraft(projectId, program.workspaceId);
      if (draft && draft.text !== program.program) {
        setText(draft.text); setBase(draft.baseRevision);
        setNotice("Restored your unsaved edits.");
      } else { setText(program.program); setBase(program.revision); }
      if (draft?.setAside) setSetAside(draft.setAside);
    });
    return () => { disposed = true; };
  }, [load, projectId]);

  const dirty = saved !== null && text !== saved.program;
  useEffect(() => {
    if (!saved) return;
    writeDraft(projectId, saved.workspaceId, dirty || setAside ? { text, baseRevision: base, setAside } : null);
  }, [projectId, saved, text, base, dirty, setAside]);

  // Validate as the user types: plan the text after a short pause. Only the
  // latest request may update the view.
  useEffect(() => {
    if (!saved) return;
    if (new TextEncoder().encode(text).length > MAX_PROGRAM_BYTES) {
      setValidation({ state: "invalid", error: { code: "too_large", message: "The program is too large to check here.", line: null, column: null } });
      return;
    }
    const seq = ++requestSeq.current;
    setValidation((current) => current.state === "idle" ? current : { state: "checking" });
    const timer = setTimeout(() => {
      setValidation({ state: "checking" });
      void rpc.call("planWorkspaceProgram", { projectId, workspaceId: saved.workspaceId, program: text }).then((result) => {
        if (seq !== requestSeq.current) return;
        if (result.ok && result.baseRevision !== base && text === saved.program) {
          // Nothing to keep: follow the program that changed elsewhere.
          void load().then((program) => {
            if (program && seq === requestSeq.current) { setText(program.program); setBase(program.revision); }
          });
        } else if (result.ok) {
          setValidation({ state: "valid", plan: result });
          setStale(result.baseRevision !== base);
        } else setValidation({ state: "invalid", error: result.error });
      }).catch((error) => {
        if (seq !== requestSeq.current) return;
        setValidation({ state: "invalid", error: { code: "unavailable", message: error instanceof Error ? error.message : String(error), line: null, column: null } });
      });
    }, VALIDATE_DELAY_MS);
    return () => clearTimeout(timer);
  }, [rpc, projectId, saved, text, base, recheck]);

  useEffect(() => {
    if (validation.state === "valid") {
      const count = validation.plan.actions.length;
      setAnnouncement(validation.plan.noop ? "No changes" : `Valid, ${count} change${count === 1 ? "" : "s"}`);
    } else if (validation.state === "invalid") {
      const { line, column } = validation.error;
      setAnnouncement(`Not valid${line && column ? ` at line ${line}, column ${column}` : ""}: ${errorText(validation.error)}`);
    }
  }, [validation]);

  useEffect(() => { if (review) reviewHeading.current?.focus(); }, [review]);

  function edit(next: string) {
    setText(next); setReview(null); setNotice("");
  }

  async function apply() {
    if (!saved || !review) return;
    setApplying(true); setNotice("");
    try {
      const result = await rpc.call("applyWorkspaceProgram", { projectId, workspaceId: saved.workspaceId, previewId: review.previewId });
      if (!result.ok) {
        setReview(null);
        if (result.error.code === "stale") setStale(true);
        else if (result.error.code === "expired") { setNotice(result.error.message); setRecheck((value) => value + 1); }
        else setValidation({ state: "invalid", error: result.error });
        return;
      }
      const program = await load();
      setReview(null); setSetAside(null);
      if (program) { setText(program.program); setBase(program.revision); }
      setNotice("Saved.");
    } catch (error) {
      setValidation({ state: "invalid", error: { code: "unavailable", message: error instanceof Error ? error.message : String(error), line: null, column: null } });
    } finally { setApplying(false); }
  }

  /** Replace the editor text with the saved program; the replaced text stays recoverable. */
  async function loadSaved(message: string) {
    const program = await load();
    if (!program) return;
    if (text !== program.program) setSetAside({ text, base });
    setText(program.program); setBase(program.revision); setReview(null); setStale(false);
    setNotice(message);
  }

  /** Keep the user's text and review it against the program as it is saved now. */
  async function rebase() {
    const program = await load();
    if (!program) return;
    setBase(program.revision); setReview(null); setStale(false);
    setNotice("Your edits are now compared with the saved program. Review the changes before saving.");
  }

  function goTo(error: ProgramError) {
    const area = textareaRef.current;
    if (!area || !error.line || !error.column) return;
    const offset = offsetOf(text, { line: error.line, column: error.column });
    area.focus();
    area.setSelectionRange(offset, offset);
  }

  if (!saved) {
    return <section className="margins-program" aria-label="Workspace program">
      <header className="margins-program-header"><h2>Workspace program</h2>
        {onClose && <button className="margins-quiet" onClick={onClose}>Close</button>}</header>
      {loadError ? <p className="margins-error" role="alert"><AlertCircle size={13} />{loadError}</p> : <p role="status">Loading the program…</p>}
      {loadError && <button onClick={() => void load()}>Try again</button>}
    </section>;
  }

  const error = validation.state === "invalid" ? validation.error : null;
  const located = error?.line && error.column ? { line: error.line, column: error.column, message: errorText(error) } : null;
  const plan = validation.state === "valid" ? validation.plan : null;
  const canReview = Boolean(plan && !plan.noop && !stale && !applying);
  const status = validation.state === "checking" || validation.state === "idle" ? "Checking…"
    : error ? located ? `Line ${located.line}, column ${located.column}` : "Not valid"
      : plan?.noop ? "No changes" : `Valid · ${plan?.actions.length || 0} change${plan?.actions.length === 1 ? "" : "s"}`;
  const DiffView = typeof experimental_Diff === "function" ? experimental_Diff : null;

  return <section className="margins-program" aria-label="Workspace program">
    <header className="margins-program-header">
      <div><h2>Workspace program</h2>
        <p className="margins-program-path" title={saved.programPath}>{saved.workspaceName || saved.workspaceId} Workspace</p></div>
      <div className="margins-program-actions">
        <button className="margins-quiet" disabled={!dirty || applying} onClick={() => void loadSaved("Reverted to the saved program.")}>Revert to saved</button>
        <button disabled={!canReview} onClick={() => setReview(plan)}>Review changes</button>
        {onClose && <button className="margins-quiet" aria-label="Close program editor" onClick={onClose}><X size={14} /></button>}
      </div>
    </header>
    {stale && <div className="margins-program-banner" role="alert">
      <p>The saved program changed outside this editor (for example with <code>margins workspace edit</code>) since you started editing. Saving now would overwrite that change, so Margins will not. Your edits are kept.</p>
      <div><button onClick={() => void rebase()}>Review my edits against the saved version</button>
        <button className="margins-quiet" onClick={() => void loadSaved("Loaded the saved program. Your edits are set aside.")}>Load the saved version</button></div>
    </div>}
    {setAside && <div className="margins-program-notice" role="status">Your previous text is set aside.
      <button className="margins-quiet" onClick={() => { setText(setAside.text); setBase(setAside.base); setSetAside(null); setReview(null); setNotice(""); }}>Restore it</button></div>}
    {notice && !setAside && <p className="margins-program-notice" role="status">{notice}</p>}
    <EnzymeCodeEditor value={text} onChange={edit} error={located} label="Workspace program" textareaRef={textareaRef}
      describedBy={`margins-program-keys${error ? " margins-program-error" : ""}`} />
    <footer className="margins-program-status">
      <div className="margins-program-status-row">
        <span className={`margins-program-state${error ? " is-error" : plan && !plan.noop ? " is-valid" : ""}`}>
          {error ? <AlertCircle size={13} /> : plan ? <Check size={13} /> : null}{status}</span>
        <small id="margins-program-keys">Tab indents · Esc, then Tab, leaves the editor</small>
      </div>
      <span className="margins-visually-hidden" role="status">{announcement}</span>
      {error && <p id="margins-program-error" className="margins-error">{located ? errorText(error) : error.message}
        {located && <button className="margins-quiet" onClick={() => goTo(error)}>Go to line {located.line}</button>}</p>}
    </footer>
    {review && <div className="margins-program-review" role="dialog" aria-label="Review program changes">
      <h3 ref={reviewHeading} tabIndex={-1}>Review changes</h3>
      <ul>{review.actions.map((action, index) => <li key={index}>{action.summary}</li>)}</ul>
      <div className="margins-program-diff">{DiffView
        ? <DiffView patch={review.diff} path={saved.programPath} />
        : <PlainDiff patch={review.diff} />}</div>
      <div className="margins-program-actions">
        <button disabled={applying} onClick={() => void apply()}>{applying ? "Saving…" : "Save program"}</button>
        <button className="margins-quiet" disabled={applying} onClick={() => setReview(null)}>Keep editing</button>
      </div>
    </div>}
  </section>;
}
