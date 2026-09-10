//! Renderer for the existing floating capture-circle window. Rust remains the
//! lifecycle authority; this shell paints Margo, reflects phase, selectively
//! passes transparent-window clicks through, and routes the existing controls.

import {
  blinkDelayMs,
  createStateChangeGate,
  globalPhysicalToLocalLogical,
  normalizedGaze,
  pointInInteractiveRects,
  pointerPollCadenceMs,
  type Rect,
} from "./lib/circle-pointer";
import {
  circleAccessibleTitle,
  circleControls,
  circleStatusLabel,
  parseCirclePhase,
  type CirclePhase,
} from "./lib/circle-state";
import {
  COLLAPSED_CIRCLE_SIZE,
  EXPANDED_CIRCLE_WIDTH,
  expandedCircleHeight,
  panelTransitionDurationMs,
  shouldStartWindowDrag,
} from "./lib/circle-interaction";

const isNative = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const INTERACTIVE_SELECTOR = ".mark, .more, .grip, #circle-root.controls-open .controls";

interface NativeCircleWindow {
  outerPosition(): Promise<{ x: number; y: number }>;
  scaleFactor(): Promise<number>;
  setIgnoreCursorEvents(ignore: boolean): Promise<void>;
  startDragging(): Promise<void>;
}

async function cmd(command: string, args?: Record<string, unknown>): Promise<unknown> {
  if (!isNative) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke(command, args);
}

async function resizeWindow(width: number, height: number): Promise<void> {
  if (!isNative) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const { LogicalSize } = await import("@tauri-apps/api/dpi");
  await getCurrentWindow().setSize(new LogicalSize(width, height));
}

async function showMainWindow(): Promise<void> {
  if (!isNative) return;
  const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
  const main = await WebviewWindow.getByLabel("main");
  if (main) {
    await main.show();
    await main.setFocus();
  }
}

const root = document.getElementById("circle-root")!;
const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

function initialPhase(): CirclePhase {
  const fromQuery = new URLSearchParams(window.location.search).get("phase");
  const fromAttr = root.getAttribute("data-phase");
  return parseCirclePhase(fromQuery ?? fromAttr);
}

let phase: CirclePhase = initialPhase();
let expanded = false;
let nativeWindow: NativeCircleWindow | null = null;
let pointerTimer: number | undefined;
let pointerSamplerStopped = false;
let blinkTimer: number | undefined;
let blinkEndTimer: number | undefined;
let panelTransition = 0;
const ignoreCursorGate = createStateChangeGate<boolean | null>(null);

function controlCount(): number {
  const controls = circleControls(phase);
  return [controls.pause, controls.resume, controls.finish, controls.openMargins]
    .filter(Boolean).length;
}

function waitForPanelTransition(): Promise<void> {
  const delay = panelTransitionDurationMs(reducedMotion.matches);
  return delay === 0
    ? Promise.resolve()
    : new Promise(resolve => window.setTimeout(resolve, delay));
}

function waitForPaint(): Promise<void> {
  if (reducedMotion.matches) return Promise.resolve();
  return new Promise(resolve => window.requestAnimationFrame(() => resolve()));
}

async function setExpanded(next: boolean): Promise<void> {
  const transition = ++panelTransition;
  expanded = next;
  const more = root.querySelector<HTMLElement>(".more");
  const controls = root.querySelector<HTMLElement>(".controls");
  more?.setAttribute("aria-expanded", String(next));
  if (!next) more?.focus();
  controls?.setAttribute("aria-hidden", String(!next));

  if (next) {
    // Give the native transparent window room before revealing the panel, so
    // its open transition is never clipped by the old collapsed bounds.
    root.classList.add("expanded");
    await resizeWindow(EXPANDED_CIRCLE_WIDTH, expandedCircleHeight(controlCount()));
    await waitForPaint();
    if (transition !== panelTransition || !expanded) return;
    root.classList.add("controls-open");
    controls?.querySelector<HTMLElement>("button")?.focus();
  } else {
    // Fade/settle the panel inside the expanded native bounds, then shrink.
    // Transparent gaps become click-through immediately because hit testing
    // keys off `controls-open`, not the temporary expanded layout class.
    root.classList.remove("controls-open");
    await waitForPanelTransition();
    if (transition !== panelTransition || expanded) return;
    await resizeWindow(COLLAPSED_CIRCLE_SIZE.width, COLLAPSED_CIRCLE_SIZE.height);
    root.classList.remove("expanded");
  }
}

function createMargoArt(): HTMLElement {
  const viewport = document.createElement("span");
  viewport.className = "mark-viewport";
  viewport.setAttribute("aria-hidden", "true");
  viewport.innerHTML = `
    <span class="margo-art">
      <img class="margo-paper" src="/margo/mark-paper.png" alt="" />
      <svg class="margo-face" viewBox="0 0 1254 1254" focusable="false">
        <defs>
          <clipPath id="circle-eye-left"><path d="M392 701c0-39 25-64 61-62 38 2 59 30 55 68-4 36-27 58-60 54-34-3-55-25-56-60Z" /></clipPath>
          <clipPath id="circle-eye-right"><path d="M759 702c-1-37 24-62 59-61 37 1 61 28 58 63-3 36-26 58-59 58-36-1-58-24-58-60Z" /></clipPath>
        </defs>
        <g class="eyes">
          <g class="eye-unit">
            <path class="eye" d="M392 701c0-39 25-64 61-62 38 2 59 30 55 68-4 36-27 58-60 54-34-3-55-25-56-60Z" />
            <ellipse class="eye-fleck" cx="430" cy="672" rx="7" ry="5" />
            <g class="lower-lid" clip-path="url(#circle-eye-left)">
              <path class="lower-lid-cover" d="M320 772 Q450 744 580 772 L580 930 L320 930Z" />
              <path class="lower-lid-rim" d="M330 770 Q450 747 570 770" />
            </g>
          </g>
          <g class="eye-unit">
            <path class="eye" d="M759 702c-1-37 24-62 59-61 37 1 61 28 58 63-3 36-26 58-59 58-36-1-58-24-58-60Z" />
            <ellipse class="eye-fleck" cx="795" cy="674" rx="7" ry="5" />
            <g class="lower-lid" clip-path="url(#circle-eye-right)">
              <path class="lower-lid-cover" d="M690 772 Q817 744 945 772 L945 930 L690 930Z" />
              <path class="lower-lid-rim" d="M700 770 Q817 747 935 770" />
            </g>
          </g>
        </g>
        <path class="mouth mouth-neutral" d="M602 780 Q629 782 656 780" />
        <path class="mouth mouth-soft" d="M602 779 Q629 787 656 779" />
      </svg>
    </span>`;

  // The static canonical mark is the fallback if the derived live layer fails.
  const paper = viewport.querySelector<HTMLImageElement>(".margo-paper")!;
  paper.addEventListener("error", () => {
    if (paper.src.endsWith("/margo/mark.png")) return;
    paper.src = "/margo/mark.png";
    viewport.querySelector<SVGElement>(".margo-face")?.setAttribute("hidden", "");
  });
  return viewport;
}

function render(): void {
  root.dataset.phase = phase;
  const controls = circleControls(phase);
  root.innerHTML = "";

  const grip = document.createElement("div");
  grip.className = "grip";
  grip.setAttribute("aria-hidden", "true");
  grip.addEventListener("pointerdown", event => {
    if (!shouldStartWindowDrag(event.button, event.isPrimary)) return;
    event.preventDefault();
    void startWindowDrag();
  });
  root.appendChild(grip);

  const markRow = document.createElement("div");
  markRow.className = "mark-row";

  const mark = document.createElement("button");
  mark.className = "mark";
  mark.type = "button";
  mark.title = circleAccessibleTitle(phase);
  mark.setAttribute("aria-label", circleAccessibleTitle(phase));
  mark.appendChild(createMargoArt());
  const status = document.createElement("span");
  status.className = "visually-hidden";
  status.textContent = circleStatusLabel(phase);
  mark.appendChild(status);
  mark.addEventListener("click", () => void cmd("toggle_active_pad"));
  markRow.appendChild(mark);

  const more = document.createElement("button");
  more.className = "more";
  more.type = "button";
  more.textContent = "⋯";
  more.title = "Capture controls";
  more.setAttribute("aria-label", "Capture controls");
  more.setAttribute("aria-haspopup", "menu");
  more.setAttribute("aria-expanded", String(expanded));
  more.addEventListener("click", () => void setExpanded(!expanded));
  markRow.appendChild(more);
  root.appendChild(markRow);

  const surface = document.createElement("div");
  surface.className = "controls";
  surface.setAttribute("role", "menu");
  surface.setAttribute("aria-label", "Capture controls");
  surface.setAttribute("aria-hidden", String(!expanded));

  const addButton = (label: string, handler: () => void) => {
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = label;
    button.setAttribute("role", "menuitem");
    button.addEventListener("click", () => {
      void setExpanded(false);
      handler();
    });
    surface.appendChild(button);
  };

  if (controls.pause) addButton("Pause", () => void cmd("pause_recording"));
  if (controls.resume) addButton("Resume", () => void cmd("resume_recording"));
  if (controls.finish) addButton("Finish", () => void cmd("stop_recording"));
  if (controls.openMargins) addButton("Open Margins", () => void showMainWindow());
  root.appendChild(surface);
  root.setAttribute("aria-label", circleAccessibleTitle(phase));
}

function setPhase(next: CirclePhase): void {
  if (next === phase) return;
  phase = next;
  if (expanded) void setExpanded(false).then(render);
  else render();
}

async function subscribe(): Promise<void> {
  if (!isNative) return;
  const { listen } = await import("@tauri-apps/api/event");
  await listen<string>("circle-phase", event => setPhase(parseCirclePhase(event.payload)));
}

function interactiveRects(): Rect[] {
  return [...document.querySelectorAll<HTMLElement>(INTERACTIVE_SELECTOR)]
    .map(element => ({ element, rect: element.getBoundingClientRect() }))
    .filter(({ element, rect }) => {
      const style = getComputedStyle(element);
      return rect.width > 0 && rect.height > 0
        && style.display !== "none" && style.visibility !== "hidden";
    })
    .map(({ rect }) => ({
      left: rect.left,
      top: rect.top,
      right: rect.right,
      bottom: rect.bottom,
    }));
}

function applyGaze(gaze: { nearby: boolean; x: number; y: number }): void {
  const travelX = reducedMotion.matches ? 1.15 : 2.6;
  const travelY = reducedMotion.matches ? 0.8 : 1.8;
  root.style.setProperty("--look-x", `${gaze.x * travelX}px`);
  root.style.setProperty("--look-y", `${gaze.y * travelY}px`);
  root.classList.toggle("pointer-near", gaze.nearby);
}

async function applyIgnoreCursorEvents(ignore: boolean): Promise<void> {
  if (!nativeWindow || !ignoreCursorGate.shouldApply(ignore)) return;
  try {
    await nativeWindow.setIgnoreCursorEvents(ignore);
  } catch (error) {
    ignoreCursorGate.shouldApply(!ignore);
    console.warn("Margins capture mark could not update click-through state", error);
  }
}

async function currentNativeWindow(): Promise<NativeCircleWindow | null> {
  if (!isNative) return null;
  if (nativeWindow) return nativeWindow;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  nativeWindow = getCurrentWindow() as unknown as NativeCircleWindow;
  return nativeWindow;
}

async function startWindowDrag(): Promise<void> {
  const windowToDrag = await currentNativeWindow();
  if (!windowToDrag) return;
  root.classList.add("dragging");
  try {
    // A real pointerdown on the grip proves this surface is interactive. Keep
    // the native window accepting the in-flight drag until AppKit releases it.
    await applyIgnoreCursorEvents(false);
    await windowToDrag.startDragging();
  } catch (error) {
    console.warn("Margins capture mark could not start dragging", error);
  } finally {
    root.classList.remove("dragging");
    schedulePointerSample(0);
  }
}

function clearPointerTimer(): void {
  if (pointerTimer !== undefined) window.clearTimeout(pointerTimer);
  pointerTimer = undefined;
}

function schedulePointerSample(delay: number): void {
  clearPointerTimer();
  if (pointerSamplerStopped || document.hidden) return;
  pointerTimer = window.setTimeout(() => void samplePointer(), delay);
}

async function samplePointer(): Promise<void> {
  if (pointerSamplerStopped || document.hidden || !nativeWindow) return;
  let nearby = false;
  let interactive = false;
  try {
    const { cursorPosition } = await import("@tauri-apps/api/window");
    const [cursor, origin, scale] = await Promise.all([
      cursorPosition(),
      nativeWindow.outerPosition(),
      nativeWindow.scaleFactor(),
    ]);
    const local = globalPhysicalToLocalLogical(cursor, origin, scale);
    const markRect = root.querySelector<HTMLElement>(".mark")?.getBoundingClientRect();
    const gaze = markRect
      ? normalizedGaze(local, markRect)
      : { nearby: false, x: 0, y: 0 };
    nearby = gaze.nearby;
    applyGaze(gaze);
    interactive = pointInInteractiveRects(local, interactiveRects());
    await applyIgnoreCursorEvents(!interactive);
  } catch (error) {
    applyGaze({ nearby: false, x: 0, y: 0 });
    await applyIgnoreCursorEvents(true);
    console.warn("Margins capture mark pointer sample failed", error);
  }
  schedulePointerSample(pointerPollCadenceMs(nearby || interactive || expanded));
}

async function startPointerSampler(): Promise<void> {
  if (!isNative) return;
  await currentNativeWindow();
  // Start in the safe pass-through posture; the first DOM sample selectively
  // re-enables only a real interactive surface.
  await applyIgnoreCursorEvents(true);
  schedulePointerSample(0);
}

function clearBlinkTimers(): void {
  if (blinkTimer !== undefined) window.clearTimeout(blinkTimer);
  if (blinkEndTimer !== undefined) window.clearTimeout(blinkEndTimer);
  blinkTimer = undefined;
  blinkEndTimer = undefined;
}

function scheduleBlink(): void {
  clearBlinkTimers();
  if (reducedMotion.matches || document.hidden || pointerSamplerStopped) return;
  blinkTimer = window.setTimeout(() => {
    const eyes = root.querySelector<SVGGElement>(".eyes");
    eyes?.classList.add("blinking");
    blinkEndTimer = window.setTimeout(() => {
      eyes?.classList.remove("blinking");
      scheduleBlink();
    }, 210);
  }, blinkDelayMs(Math.random()));
}

document.addEventListener("keydown", event => {
  if (event.key === "Escape" && expanded) {
    event.preventDefault();
    void setExpanded(false);
  }
});

document.addEventListener("visibilitychange", () => {
  if (document.hidden) {
    clearPointerTimer();
    clearBlinkTimers();
    applyGaze({ nearby: false, x: 0, y: 0 });
    void applyIgnoreCursorEvents(true);
  } else {
    schedulePointerSample(0);
    scheduleBlink();
  }
});

reducedMotion.addEventListener("change", () => {
  root.querySelector(".eyes")?.classList.remove("blinking");
  scheduleBlink();
});

window.addEventListener("beforeunload", () => {
  pointerSamplerStopped = true;
  clearPointerTimer();
  clearBlinkTimers();
});

render();
void resizeWindow(COLLAPSED_CIRCLE_SIZE.width, COLLAPSED_CIRCLE_SIZE.height);
void subscribe();
void startPointerSampler();
scheduleBlink();
