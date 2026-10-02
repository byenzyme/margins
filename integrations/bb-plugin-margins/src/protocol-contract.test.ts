import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { CAPTURE_PROTOCOL_VERSION, hostCaptureSnapshotSchema } from "./contracts.js";
import {
  workspaceArtifactSchema, workspaceMemoSchema, workspaceNoteAssociationSchema,
  workspaceProcessingJobSchema, workspaceSessionPageSchema, workspaceSessionSummarySchema,
  workspaceTranscriptSchema,
} from "./project-server.js";

describe("hosted capture protocol", () => {
  it("matches the existing Margins project recording service", async () => {
    const rustPath = fileURLToPath(new URL("../../../crates/public/margins-server/src/lib.rs", import.meta.url));
    const source = await readFile(rustPath, "utf8");
    const rustVersion = source.match(/HOSTED_CAPTURE_PROTOCOL_VERSION:\s*u8\s*=\s*(\d+)/)?.[1];
    expect(rustVersion, "Rust capture protocol constant").toBe(String(CAPTURE_PROTOCOL_VERSION));
    expect(CAPTURE_PROTOCOL_VERSION).toBe(3);
  });

  it("accepts the Rust browser snapshot wire fixture exactly", async () => {
    const fixturePath = fileURLToPath(new URL(
      "../../../crates/public/margins-server/contracts/browser-snapshot.v3.json", import.meta.url,
    ));
    const fixture = JSON.parse(await readFile(fixturePath, "utf8")) as unknown;
    expect(hostCaptureSnapshotSchema.parse(fixture)).toEqual(fixture);
    expect(() => hostCaptureSnapshotSchema.parse({ ...(fixture as object), extra: true })).toThrow();
  });

  it("accepts every Rust Workspace response shape used by the plugin", async () => {
    const fixturePath = fileURLToPath(new URL(
      "../../../crates/public/margins-server/contracts/workspace-responses.v1.json", import.meta.url,
    ));
    const fixture = JSON.parse(await readFile(fixturePath, "utf8")) as Record<string, unknown>;
    const schemas = {
      session_summary: workspaceSessionSummarySchema,
      session_page: workspaceSessionPageSchema,
      transcript: workspaceTranscriptSchema,
      artifacts: workspaceArtifactSchema.array(),
      memo: workspaceMemoSchema,
      note_association: workspaceNoteAssociationSchema,
      note_association_missing: workspaceNoteAssociationSchema.nullable(),
      processing_job: workspaceProcessingJobSchema,
    };
    expect(Object.keys(fixture).sort()).toEqual(Object.keys(schemas).sort());
    for (const [key, schema] of Object.entries(schemas)) {
      const value = fixture[key];
      expect(schema.parse(value), key).toEqual(value);
      if (value && typeof value === "object" && !Array.isArray(value)) {
        expect(() => schema.parse({ ...(value as Record<string, unknown>), unexpected: true }), key).toThrow();
      }
    }
    expect((fixture.session_page as { sessions: unknown[] }).sessions[0]).toEqual(fixture.session_summary);
  });
});
