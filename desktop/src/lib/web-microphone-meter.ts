export interface WebMicrophoneLevelSnapshot {
  level: number | null;
  /** RMS energy of the same window — smoother than peak, used by the waveform. */
  rms: number | null;
  audioFrameCount: number;
}

interface MeterAnalyser {
  fftSize: number;
  getFloatTimeDomainData(samples: Float32Array): void;
  disconnect(): void;
}

interface MeterSource {
  connect(destination: MeterAnalyser): unknown;
  disconnect(): void;
}

interface MeterAudioContext {
  state: string;
  createAnalyser(): MeterAnalyser;
  createMediaStreamSource(stream: MediaStream): MeterSource;
  resume(): Promise<void>;
  suspend(): Promise<void>;
  close(): Promise<void>;
}

export interface WebMicrophoneMeterDependencies {
  createAudioContext(): MeterAudioContext;
  requestFrame(callback: FrameRequestCallback): number;
  cancelFrame(handle: number): void;
}

function browserMeterDependencies(): WebMicrophoneMeterDependencies | null {
  if (typeof AudioContext === "undefined" || typeof window === "undefined") return null;
  return {
    createAudioContext: () => new AudioContext() as unknown as MeterAudioContext,
    requestFrame: callback => window.requestAnimationFrame(callback),
    cancelFrame: handle => window.cancelAnimationFrame(handle),
  };
}

export class WebMicrophoneMeter {
  private readonly context: MeterAudioContext;
  private readonly source: MeterSource;
  private readonly analyser: MeterAnalyser;
  private readonly dependencies: WebMicrophoneMeterDependencies;
  private readonly samples: Float32Array;
  private frameHandle: number | null = null;
  private level: number | null = null;
  private rms: number | null = null;
  private audioFrameCount = 0;
  private running = true;
  private closed = false;

  constructor(
    context: MeterAudioContext,
    source: MeterSource,
    analyser: MeterAnalyser,
    dependencies: WebMicrophoneMeterDependencies,
  ) {
    this.context = context;
    this.source = source;
    this.analyser = analyser;
    this.dependencies = dependencies;
    this.samples = new Float32Array(analyser.fftSize);
    this.scheduleSample();
  }

  snapshot(): WebMicrophoneLevelSnapshot {
    return { level: this.level, rms: this.rms, audioFrameCount: this.audioFrameCount };
  }

  async pause(): Promise<void> {
    if (this.closed || !this.running) return;
    this.running = false;
    this.cancelScheduledSample();
    this.level = null;
    this.rms = null;
    if (this.context.state !== "closed") await this.context.suspend().catch(() => undefined);
  }

  async resume(): Promise<void> {
    if (this.closed || this.running) return;
    this.level = null;
    this.rms = null;
    if (this.context.state !== "running") {
      await this.context.resume().catch(() => undefined);
    }
    if (this.context.state !== "running") return;
    this.running = true;
    this.scheduleSample();
  }

  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    this.running = false;
    this.cancelScheduledSample();
    this.level = null;
    this.rms = null;
    try { this.source.disconnect(); } catch { /* already disconnected */ }
    try { this.analyser.disconnect(); } catch { /* already disconnected */ }
    if (this.context.state !== "closed") await this.context.close().catch(() => undefined);
  }

  private readonly sample = () => {
    this.frameHandle = null;
    if (!this.running || this.closed) return;
    try {
      this.analyser.getFloatTimeDomainData(this.samples);
      let peak = 0;
      let sumSquares = 0;
      for (const sample of this.samples) {
        peak = Math.max(peak, Math.abs(sample));
        sumSquares += sample * sample;
      }
      this.level = peak;
      this.rms = Math.sqrt(sumSquares / this.samples.length);
      this.audioFrameCount += this.samples.length;
    } catch {
      this.level = null;
      this.rms = null;
    }
    this.scheduleSample();
  };

  private scheduleSample(): void {
    if (!this.running || this.closed || this.frameHandle !== null) return;
    this.frameHandle = this.dependencies.requestFrame(this.sample);
  }

  private cancelScheduledSample(): void {
    if (this.frameHandle === null) return;
    this.dependencies.cancelFrame(this.frameHandle);
    this.frameHandle = null;
  }
}

export async function createWebMicrophoneMeter(
  stream: MediaStream,
  dependencies: WebMicrophoneMeterDependencies | null = browserMeterDependencies(),
): Promise<WebMicrophoneMeter | null> {
  if (!dependencies) return null;

  let context: MeterAudioContext | null = null;
  let source: MeterSource | null = null;
  let analyser: MeterAnalyser | null = null;
  try {
    context = dependencies.createAudioContext();
    analyser = context.createAnalyser();
    analyser.fftSize = 1024;
    source = context.createMediaStreamSource(stream);
    source.connect(analyser);
    if (context.state !== "running") await context.resume();
    if (context.state !== "running") throw new Error("Web Audio meter did not start");
    return new WebMicrophoneMeter(context, source, analyser, dependencies);
  } catch {
    try { source?.disconnect(); } catch { /* partially initialized */ }
    try { analyser?.disconnect(); } catch { /* partially initialized */ }
    if (context?.state !== "closed") await context?.close().catch(() => undefined);
    return null;
  }
}
