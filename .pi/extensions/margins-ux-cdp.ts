import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { StringEnum } from "@earendil-works/pi-ai";

const Action = StringEnum(["run", "open", "screenshot"] as const);

export default function marginsUxCdp(pi: ExtensionAPI) {
  pi.registerTool({
    name: "margins_ux_cdp",
    label: "Margins UX CDP",
    description: "Drive the margins desktop browser harness through Chrome DevTools Protocol and capture scenario screenshots/text without Playwright.",
    promptSnippet: "Run margins desktop UX browser scenarios through raw Chrome DevTools Protocol.",
    promptGuidelines: [
      "Use margins_ux_cdp when iterating on margins desktop UX screenshots in the Vite/browser harness.",
      "Before using margins_ux_cdp, ensure `cd desktop && npm run dev` is running or start it in a background shell.",
    ],
    parameters: Type.Object({
      action: Action,
      scenarios: Type.Optional(Type.Array(Type.String(), { description: "Scenario names such as recording-healthy or recording-dead-tap." })),
      base: Type.Optional(Type.String({ description: "Vite base URL, default http://localhost:5173" })),
      out: Type.Optional(Type.String({ description: "Output directory for screenshots/reports, default desktop/ux-shots" })),
    }),
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {
      const args = [`${ctx.cwd}/desktop/scripts/ux-cdp.mjs`, params.action];
      if (params.scenarios?.length) {
        if (params.action === "run") args.push("--scenarios", params.scenarios.join(","));
        else args.push(params.scenarios[0]);
      }
      if (params.base) args.push("--base", params.base);
      if (params.out) args.push("--out", params.out);

      const result = await pi.exec("node", args, {
        signal,
        timeout: params.action === "run" ? 60_000 : 25_000,
      });
      const text = [result.stdout, result.stderr].filter(Boolean).join("\n");
      if (result.code !== 0) throw new Error(text || `margins_ux_cdp exited ${result.code}`);
      return {
        content: [{ type: "text", text: text || "margins_ux_cdp completed" }],
        details: { action: params.action, scenarios: params.scenarios, out: params.out, code: result.code },
      };
    },
  });
}
