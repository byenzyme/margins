/**
 * Model-provisioning notice: a fixed-position, non-blocking banner shown after
 * capture starts to let the user download or configure local speech models.
 *
 * Visible copy MUST NOT mention internal model names (FluidAudio, CoreML,
 * parakeet, polyvoice, diarization). Use "speech models", "local transcription",
 * "speaker labels" only.
 */

import { esc, js } from "../lib/html";

export type ModelNoticeState =
  | "A"           // transcription missing — offer download
  | "B"           // transcription ready, diarization in progress — indeterminate pill
  | "C"           // transcription found at unexpected path — offer reuse
  | "downloading" // download in flight
  | "warming"     // download done, loading/warming the model (indeterminate)
  | "done"        // brief success confirmation — auto-dismiss
  | "error"       // download failed
  | "hidden";

export interface ModelNoticeData {
  state: ModelNoticeState;
  downloadBytes: number | null;
  /** Path where transcription was found (state C). */
  foundPath: string | null;
  /** 0..1 fractional progress, or null = indeterminate. */
  progress: number | null;
  message: string;
  /** When state C, user is typing a custom path. */
  customPathInputVisible?: boolean;
}

function sizeLabel(bytes: number | null): string {
  if (!bytes) return "";
  const mb = Math.round(bytes / 1_000_000);
  return mb > 0 ? ` (~${mb} MB)` : "";
}

export function renderModelNotice(data: ModelNoticeData | null): string {
  if (!data || data.state === "hidden") return "";

  const { state } = data;

  if (state === "B") {
    // Compact indeterminate pill: "Adding speaker labels…". Dismissible, and
    // auto-cleared once diarization lands (re-probed on capture finish).
    return `
      <div class="model-notice model-notice-pill" role="status" aria-live="polite">
        <div class="model-notice-pill-inner">
          <div class="model-progress-track indeterminate"></div>
          <span>Adding speaker labels…</span>
        </div>
        <button class="subtle model-notice-pill-close" onclick="window.__modelNoticeDismiss()" aria-label="Dismiss">×</button>
      </div>
    `;
  }

  if (state === "done") {
    return `
      <div class="model-notice model-notice-done" role="status" aria-live="polite">
        <span>Local transcription is ready.</span>
        <button class="subtle" onclick="window.__modelNoticeDismiss()">×</button>
      </div>
    `;
  }

  if (state === "downloading") {
    const pct = data.progress == null ? null : Math.max(0, Math.min(100, Math.round(data.progress * 100)));
    const indeterminate = data.progress == null;
    return `
      <div class="model-notice" role="status" aria-live="polite">
        <div class="model-notice-top">
          <strong>Downloading speech models</strong>
          ${pct != null ? `<span class="model-notice-pct">${pct}%</span>` : ""}
        </div>
        <div class="model-notice-body">${esc(data.message || "Downloading…")}</div>
        <div class="model-progress-track${indeterminate ? " indeterminate" : ""}">
          ${!indeterminate ? `<div style="width: ${Math.max(4, pct ?? 4)}%"></div>` : ""}
        </div>
        <div class="model-notice-actions">
          <button class="subtle" onclick="window.__modelNoticeCancel()">Cancel</button>
        </div>
      </div>
    `;
  }

  if (state === "warming") {
    // Download is complete; the model is loading/warming. One line, no percent —
    // an indeterminate bar so it never looks like the download bar jumped back.
    return `
      <div class="model-notice" role="status" aria-live="polite">
        <div class="model-notice-top">
          <strong>Warming up…</strong>
        </div>
        <div class="model-progress-track indeterminate"></div>
      </div>
    `;
  }

  if (state === "error") {
    return `
      <div class="model-notice model-notice-error" role="alert">
        <div class="model-notice-top">
          <strong>Couldn't download speech models</strong>
          <button class="subtle model-notice-close" onclick="window.__modelNoticeDismiss()" aria-label="Dismiss">×</button>
        </div>
        <div class="model-notice-body">${data.message ? esc(data.message) : "Your recording is safe. Check your connection and try again."}</div>
        <div class="model-notice-actions">
          <button class="primary" onclick="window.__modelNoticeDownload()">Try again</button>
          <button class="subtle" onclick="window.__modelNoticeDismiss()">Not now</button>
        </div>
      </div>
    `;
  }

  if (state === "C") {
    const path = data.foundPath || "";
    const custom = data.customPathInputVisible;
    return `
      <div class="model-notice" role="dialog" aria-label="Speech models">
        <div class="model-notice-top">
          <strong>${custom ? "Choose speech models folder" : "Found speech models"}</strong>
          <button class="subtle model-notice-close" onclick="window.__modelNoticeDismiss()" aria-label="Dismiss">×</button>
        </div>
        <div class="model-notice-body">
          ${custom
            ? "Enter the folder that contains your speech models."
            : `Found speech models at <code class="model-notice-path">${esc(path)}</code>. Use these?`}
        </div>
        ${custom ? `
          <div class="model-notice-custom-path">
            <input
              id="model-notice-custom-path-input"
              class="model-notice-path-input"
              type="text"
              placeholder="Path to speech models folder"
              value="${esc(path)}"
              aria-label="Custom models path"
            />
            ${data.message ? `<div class="model-notice-inline-error" role="alert">${esc(data.message)}</div>` : ""}
          </div>
        ` : ""}
        <div class="model-notice-actions">
          ${custom
            ? `<button class="primary" onclick="window.__modelNoticeConfirmCustom()">Use this path</button>
               <button class="subtle" onclick="window.__modelNoticeDismiss()">Cancel</button>`
            : `<button class="primary" onclick="window.__modelNoticeUseFound(${js(path)})">Use these</button>
               <button class="subtle" onclick="window.__modelNoticeChooseCustom()">Choose different</button>`
          }
        </div>
      </div>
    `;
  }

  // State A: transcription missing
  const size = sizeLabel(data.downloadBytes);
  return `
    <div class="model-notice" role="dialog" aria-label="Set up local transcription">
      <div class="model-notice-top">
        <strong>Set up local transcription</strong>
        <button class="subtle model-notice-close" onclick="window.__modelNoticeDismiss()" aria-label="Dismiss">×</button>
      </div>
      <div class="model-notice-body">
        Recording now — transcription turns on once speech models${size} finish downloading.
      </div>
      <div class="model-notice-actions">
        <button class="primary" onclick="window.__modelNoticeDownload()">Download</button>
        <button class="subtle" onclick="window.__modelNoticeDismiss()">Not now</button>
        <button class="subtle model-notice-settings-link" onclick="window.__openAudioSetup()">Open settings</button>
      </div>
    </div>
  `;
}
