#!/usr/bin/env node

import { readFileSync } from "node:fs";

const [baseUrl, token, sessionName, wavPath] = process.argv.slice(2);
if (!baseUrl || !token || !sessionName || !wavPath) {
  console.error("usage: stream-live-pcm.mjs <base-url> <token> <session> <mono-s16-wav>");
  process.exit(2);
}

const { sampleRate, samples } = readMonoS16Wav(wavPath);
const chunkMs = Number(process.env.MARGINS_PCM_CHUNK_MS || 1000);
const speed = Number(process.env.MARGINS_PCM_SPEED || 1);
const chunkSamples = Math.max(1, Math.round(sampleRate * chunkMs / 1000));
const startedUnixMs = Date.now();

console.log(JSON.stringify({
  kind: "live_pcm_stream_started",
  unix_ms: startedUnixMs,
  session: sessionName,
  sample_rate: sampleRate,
  duration_ms: Math.round(samples.length * 1000 / sampleRate),
  chunk_ms: chunkMs,
  speed,
}));

for (let offset = 0; offset < samples.length; offset += chunkSamples) {
  const chunk = samples.subarray(offset, Math.min(offset + chunkSamples, samples.length));
  const body = Buffer.allocUnsafe(chunk.length * 4);
  for (let index = 0; index < chunk.length; index += 1) {
    body.writeFloatLE(chunk[index], index * 4);
  }
  const mediaStartMs = Math.round(offset * 1000 / sampleRate);
  const requestStarted = Date.now();
  const response = await fetch(
    `${baseUrl}/api/live-audio/pcm?session=${encodeURIComponent(sessionName)}&channel=mic&sample_rate=${sampleRate}`,
    {
      method: "POST",
      headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/octet-stream" },
      body,
    },
  );
  const payload = await response.json();
  if (!response.ok || !payload.ok) {
    throw new Error(`PCM injection failed at ${mediaStartMs} ms: ${JSON.stringify(payload)}`);
  }
  console.log(JSON.stringify({
    kind: "live_pcm_chunk_sent",
    unix_ms: Date.now(),
    media_start_ms: mediaStartMs,
    media_end_ms: Math.round((offset + chunk.length) * 1000 / sampleRate),
    request_ms: Date.now() - requestStarted,
  }));
  if (offset + chunk.length < samples.length) {
    await new Promise(resolve => setTimeout(resolve, chunkMs / speed));
  }
}

console.log(JSON.stringify({
  kind: "live_pcm_stream_finished",
  unix_ms: Date.now(),
  wall_ms: Date.now() - startedUnixMs,
}));

function readMonoS16Wav(path) {
  const data = readFileSync(path);
  if (data.toString("ascii", 0, 4) !== "RIFF" || data.toString("ascii", 8, 12) !== "WAVE") {
    throw new Error(`not a WAV file: ${path}`);
  }
  let offset = 12;
  let sampleRate = 0;
  let channels = 0;
  let bits = 0;
  let pcm;
  while (offset + 8 <= data.length) {
    const id = data.toString("ascii", offset, offset + 4);
    const size = data.readUInt32LE(offset + 4);
    const start = offset + 8;
    if (id === "fmt ") {
      channels = data.readUInt16LE(start + 2);
      sampleRate = data.readUInt32LE(start + 4);
      bits = data.readUInt16LE(start + 14);
    } else if (id === "data") {
      pcm = data.subarray(start, start + size);
    }
    offset = start + size + (size % 2);
  }
  if (!pcm || channels !== 1 || bits !== 16 || sampleRate <= 0) {
    throw new Error(`expected mono signed-16 WAV: ${path}`);
  }
  const samples = new Float32Array(pcm.length / 2);
  for (let index = 0; index < samples.length; index += 1) {
    samples[index] = pcm.readInt16LE(index * 2) / 32768;
  }
  return { sampleRate, samples };
}
