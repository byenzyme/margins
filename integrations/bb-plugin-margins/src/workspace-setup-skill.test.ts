import { readFile } from "node:fs/promises";
import { describe, expect, it } from "vitest";

const skill = () => readFile(new URL("../skills/workspace-setup/SKILL.md", import.meta.url), "utf8");

describe("workspace-setup skill", () => {
  it("requires showing the plan's consequences before apply, then no second confirmation", async () => {
    const text = (await skill()).replace(/\s+/g, " ");
    expect(text).toContain("Do not apply a workspace plan until the user has seen its plain-language consequences");
    expect(text).toContain("before running `workspace apply`");
    for (const consequence of ["preset readings kept", "preset folders skipped", "what it leaves out", "the folder new notes go to"]) {
      expect(text).toContain(consequence);
    }
    expect(text).toContain("Reporting them only after apply is not showing the plan.");
    expect(text).toContain("Once the user has seen them, the setup request authorizes applying the preset plan unchanged; do not ask for a second confirmation.");
  });
});
