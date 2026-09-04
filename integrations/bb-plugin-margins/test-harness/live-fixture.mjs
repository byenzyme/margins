import { createServer } from "node:http";

const port = Number.parseInt(process.env.MARGINS_FIXTURE_PORT || "19093", 10);
const token = process.env.MARGINS_FIXTURE_TOKEN || "margins-ui-fixture-token";
let status = process.env.MARGINS_FIXTURE_INITIAL_STATUS || "idle";
let generation = 2;
let revision = 1;
let notes = [];
let nextStartError = null;
const startDelayMs = Number.parseInt(process.env.MARGINS_FIXTURE_START_DELAY_MS || "900", 10);

function snapshot() {
  return {
    protocol_version: 1,
    server_unix_ms: Date.now(),
    session:
      status === "idle"
        ? null
        : {
            session_id: "customer-call",
            status,
            elapsed_ms: 123_456,
            generation,
          },
    health: {
      capture_phase: status,
      tap_status: "ok",
      system_audio_expected: true,
      system_audio_observed: true,
      transcript_freshness: {
        decoded_until_ms: 121_000,
        committed_until_ms: 119_000,
        updated_at_unix_ms: Date.now(),
        age_ms: 0,
      },
    },
    rolling_transcript: [
      { at_ms: 118_000, text: "[01:58] Sam: We can send the revised pricing tomorrow." },
      { at_ms: 119_000, text: "[01:59] Me: I will follow up with the deck." },
    ],
    memo_lines:
      status === "idle"
        ? []
        : notes.map((text, index) => ({ index, at_ms: 12_000 + index * 10_000, text })),
    notepad_revision: `fixture-${revision}`,
  };
}

function json(response, statusCode, value) {
  response.writeHead(statusCode, { "content-type": "application/json" });
  response.end(JSON.stringify(value));
}

async function body(request) {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  return chunks.length ? JSON.parse(Buffer.concat(chunks).toString("utf8")) : {};
}

const server = createServer(async (request, response) => {
  if (request.method === "GET" && request.url === "/__fixture/state") {
    json(response, 200, { status, generation, revision, notes, nextStartError });
    return;
  }
  if (request.method === "POST" && request.url === "/__fixture/state") {
    const input = await body(request);
    if (typeof input.status === "string") status = input.status;
    if (Array.isArray(input.notes)) notes = input.notes.map(String);
    nextStartError = typeof input.next_start_error === "string" ? input.next_start_error : null;
    json(response, 200, { status, generation, revision, notes, nextStartError });
    return;
  }

  if (request.headers.authorization !== `Bearer ${token}`) {
    json(response, 401, { code: "unauthorized", message: "Unauthorized", retryable: false });
    return;
  }

  if (request.method === "GET" && request.url?.startsWith("/v1/live/snapshot")) {
    json(response, 200, snapshot());
    return;
  }

  const input = await body(request);
  if (request.method === "POST" && request.url === "/v1/live/notepad") {
    if (input.expected_notepad_revision !== `fixture-${revision}`) {
      json(response, 409, {
        code: "notepad_changed",
        message: "The notepad changed somewhere else.",
        retryable: true,
      });
      return;
    }
    notes = String(input.text || "")
      .split("\n")
      .filter((line) => line.trim());
    revision += 1;
  } else if (request.method === "POST" && request.url === "/v1/live/pause") {
    status = "paused";
    generation += 1;
  } else if (request.method === "POST" && request.url === "/v1/live/resume") {
    status = "recording";
    generation += 1;
  } else if (request.method === "POST" && request.url === "/v1/live/stop") {
    status = "idle";
    generation += 1;
  } else if (request.method === "POST" && request.url === "/v1/live/start") {
    await new Promise((resolve) => setTimeout(resolve, startDelayMs));
    if (nextStartError) {
      const code = nextStartError;
      nextStartError = null;
      json(response, 409, {
        code,
        message:
          code === "microphone_permission_denied"
            ? "Microphone permission was denied."
            : "Recording could not start.",
        retryable: true,
      });
      return;
    }
    status = "recording";
    generation += 1;
  } else {
    json(response, 404, { code: "not_found", message: "Not found", retryable: false });
    return;
  }

  json(response, 200, {
    protocol_version: 1,
    idempotent_replay: false,
    snapshot: snapshot(),
    ...(request.url === "/v1/live/stop" ? { stopped_session_id: "customer-call" } : {}),
  });
});

server.listen(port, "127.0.0.1", () => {
  console.log(`Margins live fixture listening on ${port}`);
});
