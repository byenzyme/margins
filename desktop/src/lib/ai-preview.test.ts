// AI settings preview summary + quick-assists section routing. Run with:
//   node --test --experimental-strip-types src/lib/ai-preview.test.ts
//
// Covers the per-activity role summary for each mode / override state and the
// decision to show the separate-cue-key notice (never silently hidden).
import { test } from "node:test";
import assert from "node:assert/strict";

import { quickAssistsSectionKind, roleSummaryInner } from "./ai-preview.ts";
import type { PreviewRole, ResolutionPreview, Settings } from "./tauri.ts";

function role(
  roleName: string,
  modelId: string,
  modelLabel: string,
  changed = false,
  providerLabel = "Included",
): PreviewRole {
  return {
    role: roleName,
    provider_label: providerLabel,
    model_id: modelId,
    model_label: modelLabel,
    changed_by_override: changed,
  };
}

test("included default lists notes, quick assists, and note search", () => {
  const preview: ResolutionPreview = {
    mode: "included",
    roles: [
      role("distill", "anthropic/claude-sonnet-4.6", "Claude Sonnet"),
      role("cue", "anthropic/claude-haiku-4.5", "Claude Haiku"),
      role("reprocess", "anthropic/claude-haiku-4.5", "Claude Haiku"),
      role("indexing", "google/gemini-3.1-flash-lite", "Gemini Flash Lite"),
    ],
  };
  const html = roleSummaryInner(preview);
  assert.match(html, /Notes: Claude Sonnet/);
  assert.match(html, /Quick assists: Claude Haiku/);
  assert.match(html, /Note search: Gemini Flash Lite/);
  assert.doesNotMatch(html, /your choice/);
});

test("included with a cue override marks the moved role and keeps indexing pinned", () => {
  const preview: ResolutionPreview = {
    mode: "included",
    roles: [
      role("distill", "anthropic/claude-sonnet-4.6", "Claude Sonnet"),
      role("cue", "google/gemini-3.1-flash-lite", "Gemini Flash Lite", true),
      role("reprocess", "google/gemini-3.1-flash-lite", "Gemini Flash Lite", true),
      role("indexing", "google/gemini-3.1-flash-lite", "Gemini Flash Lite"),
    ],
  };
  const html = roleSummaryInner(preview);
  assert.match(html, /Quick assists: Gemini Flash Lite <em>\(your choice\)<\/em> — used for live cues and note re-runs/);
  assert.match(html, /Notes: Claude Sonnet <em>\(unchanged\)<\/em>/);
  assert.match(html, /Note search: Gemini Flash Lite/);
});

test("chatgpt collapses notes & quick assists into the subscription model", () => {
  const preview: ResolutionPreview = {
    mode: "chatgpt",
    roles: [
      role("distill", "gpt-5.5", "GPT-5.5", false, "ChatGPT subscription"),
      role("cue", "gpt-5.5", "GPT-5.5", false, "ChatGPT subscription"),
      role("reprocess", "gpt-5.5", "GPT-5.5", false, "ChatGPT subscription"),
    ],
  };
  const html = roleSummaryInner(preview);
  assert.match(html, /Notes &amp; quick assists: GPT-5.5 via your subscription/);
  assert.doesNotMatch(html, /Note search/);
});

test("api mode with one model collapses notes & quick assists", () => {
  const preview: ResolutionPreview = {
    mode: "api",
    roles: [
      role("distill", "my-model", "my-model", false, "OpenAI"),
      role("cue", "my-model", "my-model", false, "OpenAI"),
      role("reprocess", "my-model", "my-model", false, "OpenAI"),
    ],
  };
  assert.match(roleSummaryInner(preview), /Notes &amp; quick assists: my-model/);
});

test("null preview renders nothing", () => {
  assert.equal(roleSummaryInner(null), "");
});

function settings(fields: Partial<Settings>): Settings {
  return fields as Settings;
}

test("quick-assists section routing: chatgpt hides the tier control", () => {
  assert.equal(quickAssistsSectionKind("chatgpt", settings({})), "chatgpt");
  assert.equal(quickAssistsSectionKind("chatgpt", settings({ backchannel_api_key: "k" })), "chatgpt");
});

test("quick-assists section routing: a configured cue key shows the notice", () => {
  assert.equal(quickAssistsSectionKind("included", settings({ backchannel_api_key: "cue-key" })), "external-key");
  assert.equal(quickAssistsSectionKind("api", settings({ backchannel_api_key: "cue-key" })), "external-key");
});

test("quick-assists section routing: otherwise the tier control", () => {
  assert.equal(quickAssistsSectionKind("included", settings({})), "tier");
  assert.equal(quickAssistsSectionKind("api", settings({ backchannel_api_key: "   " })), "tier");
});
