import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  desktopLiveDiscoverySchema,
  desktopLiveNotepadRequestSchema,
  desktopLiveSessionRequestSchema,
  desktopLiveStartRequestSchema,
  liveSnapshotSchema,
} from "./contracts.js";

const fixture = JSON.parse(
  readFileSync(
    new URL(
      "../../../crates/public/margins-meeting-protocol/tests/fixtures/desktop-live-v1.json",
      import.meta.url,
    ),
    "utf8",
  ),
) as Record<string, unknown>;

describe("Rust live contract fixture", () => {
  it("is accepted by every bb-facing V1 schema", () => {
    expect(desktopLiveDiscoverySchema.parse(fixture.discovery).runtime).toBe("margins_cli");
    expect(liveSnapshotSchema.parse(fixture.recording_snapshot).session?.status).toBe("recording");
    expect(liveSnapshotSchema.parse(fixture.idle_snapshot).session).toBeNull();
    expect(desktopLiveStartRequestSchema.parse(fixture.start_request).operation_id).toBe(
      "op_start_1",
    );
    expect(desktopLiveSessionRequestSchema.parse(fixture.session_request).session_id).toBe(
      "customer-call",
    );
    expect(desktopLiveNotepadRequestSchema.parse(fixture.notepad_request).text).toContain(
      "Send details",
    );
  });
});
