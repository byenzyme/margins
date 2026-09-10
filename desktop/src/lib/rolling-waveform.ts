/**
 * Rolling voice-memo-style waveform for the mobile capture hero. Canvas bars
 * scroll right-to-left as new level samples land, so the trail is visible
 * proof capture has been hearing you — unlike the fixed-shape signal lanes,
 * which only pulse with the instantaneous level.
 */

export interface RollingWaveformHandle {
  stop(): void;
  /** Current bar amplitudes, oldest first — pass as `seed` to a successor
   *  instance so a full re-render doesn't erase the visible trail. */
  bars(): number[];
}

const BAR_WIDTH = 3;
const BAR_GAP = 2;
const BAR_PITCH = BAR_WIDTH + BAR_GAP;
const SAMPLE_INTERVAL_MS = 33;
const NOISE_FLOOR = 0.006;
const AMPLITUDE_GAIN = 8;
const AMPLITUDE_GAMMA = 0.6;
const IDLE_AMPLITUDE = 0.06;

function levelToAmplitude(level: number | null): number {
  if (level == null || level <= 0) return IDLE_AMPLITUDE;
  const boosted = Math.max(0, level - NOISE_FLOOR) * AMPLITUDE_GAIN;
  return Math.max(IDLE_AMPLITUDE, Math.min(1, boosted ** AMPLITUDE_GAMMA));
}

/**
 * Starts drawing into `canvas`, sampling `getLevel` (RMS energy 0..1, or
 * null when no signal is available) every ~33ms. Stops itself once the canvas
 * leaves the document, so callers only need `stop()` for eager teardown.
 */
export function startRollingWaveform(
  canvas: HTMLCanvasElement,
  getLevel: () => number | null,
  seed?: number[],
): RollingWaveformHandle {
  const ctx = canvas.getContext("2d");
  if (!ctx) return { stop: () => {}, bars: () => [] };

  const reducedMotion = typeof window.matchMedia === "function"
    && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  let bars: number[] = seed ? [...seed] : [];
  let cssWidth = 0;
  let cssHeight = 0;
  let barCount = 1;
  let midY = 0;
  let maxHalf = 0;
  let edgeFade = 0;
  let stopped = false;
  let rafId = 0;
  let lastSampleAt = 0;

  const measure = () => {
    const dpr = window.devicePixelRatio || 1;
    const rect = canvas.getBoundingClientRect();
    cssWidth = rect.width;
    cssHeight = rect.height;
    canvas.width = Math.max(1, Math.round(cssWidth * dpr));
    canvas.height = Math.max(1, Math.round(cssHeight * dpr));
    // Resizing the canvas resets all 2D context state, so re-apply it here.
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.lineCap = "round";
    ctx.lineWidth = BAR_WIDTH;
    midY = cssHeight / 2;
    maxHalf = Math.max(0, (cssHeight * 0.95 - BAR_WIDTH) / 2);
    edgeFade = cssWidth * 0.15;
    barCount = Math.max(1, Math.floor(cssWidth / BAR_PITCH));
    if (bars.length > barCount) bars = bars.slice(bars.length - barCount);
  };

  const draw = () => {
    ctx.strokeStyle = getComputedStyle(canvas).color || "currentColor";
    ctx.clearRect(0, 0, cssWidth, cssHeight);
    for (let i = 0; i < bars.length; i++) {
      const amp = bars[bars.length - 1 - i];
      const cx = cssWidth - BAR_WIDTH / 2 - i * BAR_PITCH;
      if (cx + BAR_WIDTH < 0) break;
      const half = Math.max(BAR_WIDTH / 2, amp * maxHalf);
      ctx.globalAlpha = cx < edgeFade ? Math.max(0.15, cx / edgeFade) : 1;
      ctx.beginPath();
      ctx.moveTo(cx, midY - half);
      ctx.lineTo(cx, midY + half);
      ctx.stroke();
    }
    ctx.globalAlpha = 1;
  };

  const drawIdle = () => {
    bars = Array.from({ length: barCount }, () => IDLE_AMPLITUDE);
    draw();
  };

  measure();

  const observer = typeof ResizeObserver !== "undefined"
    ? new ResizeObserver(() => {
      measure();
      draw();
    })
    : null;
  observer?.observe(canvas);

  const stop = () => {
    if (stopped) return;
    stopped = true;
    cancelAnimationFrame(rafId);
    observer?.disconnect();
  };

  if (reducedMotion) {
    drawIdle();
    return { stop, bars: () => [...bars] };
  }

  const tick = (now: number) => {
    if (stopped) return;
    if (!canvas.isConnected) {
      // The recording view re-renders via innerHTML; a detached canvas means
      // this instance was replaced or the user navigated away.
      stop();
      return;
    }
    if (now - lastSampleAt >= SAMPLE_INTERVAL_MS) {
      lastSampleAt = now;
      bars.push(levelToAmplitude(getLevel()));
      if (bars.length > barCount) bars.shift();
      draw();
    }
    rafId = requestAnimationFrame(tick);
  };
  rafId = requestAnimationFrame(tick);

  return { stop, bars: () => [...bars] };
}
