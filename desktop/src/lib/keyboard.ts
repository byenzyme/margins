import { onGlobalCaptureToggle } from "./tauri";

// ---------------------------------------------------------------------------
// Capture shortcut context
// ---------------------------------------------------------------------------

export interface CaptureShortcutContext {
  /** Returns true when a capture is active (is_recording or startup 'starting'). */
  isRecording: () => boolean;
  /** Starts a capture — calls window.__startDefaultMeeting(). */
  startCapture: () => void;
  /** Pauses or resumes the active capture. Finish remains a separate terminal action. */
  stopCapture: () => void;
  /** Focuses the live mark input (#memo-input-new). */
  focusMarkInput: () => void;
  /** Opens the Settings view (⌘,). Always available, even on the empty home. */
  openSettings: () => void;
}

// ---------------------------------------------------------------------------
// Discoverable hint strings (single source of truth)
// ---------------------------------------------------------------------------

export function captureShortcutHints(): { start: string; mark: string; stop: string } {
  return { start: "⌘N", mark: "⌘L", stop: "⌘." };
}

// ---------------------------------------------------------------------------
// Installer — call once; safe to call multiple times (guarded)
// ---------------------------------------------------------------------------

let installed = false;

export function installCaptureShortcuts(ctx: CaptureShortcutContext): void {
  if (installed) return;
  installed = true;

  // In-app keydown layer (works only when Margins WebView is focused).
  document.addEventListener(
    "keydown",
    (event: KeyboardEvent) => {
      if (!event.metaKey) return;

      if (event.code === "KeyN" && !event.shiftKey && !event.altKey && !event.ctrlKey) {
        // ⌘N — start capture when idle
        if (!ctx.isRecording()) {
          event.preventDefault();
          ctx.startCapture();
        }
        return;
      }

      if (event.code === "Period" && !event.shiftKey && !event.altKey && !event.ctrlKey) {
        // ⌘. — pause/resume capture when recording
        if (ctx.isRecording()) {
          event.preventDefault();
          ctx.stopCapture();
        }
        return;
      }

      if (event.code === "KeyL" && !event.shiftKey && !event.altKey && !event.ctrlKey) {
        // ⌘L — focus mark input (safe override: browser address bar focus is irrelevant inside Tauri WebView)
        event.preventDefault();
        ctx.focusMarkInput();
        return;
      }

      if (
        (event.code === "Comma" || event.key === ",")
        && !event.shiftKey && !event.altKey && !event.ctrlKey
      ) {
        // ⌘, — open Settings. Standard macOS Preferences shortcut; always works,
        // including on the clean first-run home view (there is no native app menu).
        // Match on `event.key` too, not just the physical `event.code`, so the
        // binding also fires for non-US layouts and for synthetic/AX-dispatched
        // KeyboardEvents that set `key` but not `code`.
        event.preventDefault();
        ctx.openSettings();
        return;
      }
    },
    true, // capture phase — intercept before bubbling handlers
  );

  // OS-global layer: ⌘⇧N fires even when another app is frontmost.
  // Backend emits "global-capture-toggle"; active captures pause/resume here.
  onGlobalCaptureToggle(() => {
    if (ctx.isRecording()) {
      ctx.stopCapture();
    } else {
      ctx.startCapture();
    }
  }).catch(() => {
    // Tauri event listener setup failure is non-fatal; in-app shortcuts still work.
  });
}
