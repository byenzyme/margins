import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";
import path from "node:path";

import { DEV_SERVICE_WORKER_SOURCE, generateServiceWorker } from "./build/service-worker.ts";

const host = process.env.TAURI_DEV_HOST;
const httpTarget = process.env.MARGINS_DEV_HTTP_TARGET;
const httpToken = process.env.MARGINS_DEV_HTTP_TOKEN;
const desktopDir = fileURLToPath(new URL(".", import.meta.url));

export default defineConfig({
  clearScreen: false,
  plugins: [
    {
      name: "margins-dev-service-worker-cleanup",
      apply: "serve",
      configureServer(server) {
        server.middlewares.use((request, response, next) => {
          if (request.url?.split("?", 1)[0] !== "/sw.js") return next();
          response.statusCode = 200;
          response.setHeader("content-type", "application/javascript; charset=utf-8");
          response.setHeader("cache-control", "no-store");
          response.end(DEV_SERVICE_WORKER_SOURCE);
        });
      },
    },
    {
      name: "margins-service-worker-asset-graph",
      apply: "build",
      enforce: "post",
      async generateBundle(_options, bundle) {
        const worker = await generateServiceWorker(
          bundle,
          path.join(desktopDir, "public"),
          path.join(desktopDir, "service-worker.template.js"),
        );
        this.emitFile({
          type: "asset",
          fileName: "sw.js",
          source: worker.source,
        });
      },
    },
    {
      name: "margins-dev-http-token",
      transformIndexHtml(html) {
        if (!httpToken) return html;
        const injection = `<script>window.__MARGINS_TOKEN__=${JSON.stringify(httpToken)};</script>`;
        return html.replace("</head>", `${injection}</head>`);
      },
    },
  ],
  build: {
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        circle: fileURLToPath(new URL("./circle.html", import.meta.url)),
        pad: fileURLToPath(new URL("./pad.html", import.meta.url)),
      },
    },
  },
  server: {
    port: 5173,
    strictPort: true,
    host: host || false,
    proxy: httpTarget
      ? {
          "/api": {
            target: httpTarget,
            changeOrigin: true,
            rewrite: path => path === "/api/health" ? "/health" : path,
          },
          "/ws": {
            target: httpTarget,
            ws: true,
            changeOrigin: true,
          },
          "/health": {
            target: httpTarget,
            changeOrigin: true,
          },
        }
      : undefined,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 5174,
        }
      : undefined,
  },
});
