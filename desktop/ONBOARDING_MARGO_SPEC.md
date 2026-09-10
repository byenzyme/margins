# Margo asset policy

Margo has two deliberate roles. Git history retains the older multi-pose onboarding specification.

## Assets

| File | Role | Usage |
| --- | --- | --- |
| `welcome.png` | Welcome illustration | Home intro / brand empty state at 132px |
| `mark.png` | Floating capture mark | Native capture-circle window |

Do not add pose variants or alter the application/platform icon sets.

## Surface policy

| Surface | Margo treatment |
| --- | --- |
| Welcome / brand intro | `welcome.png` |
| Floating active capture | `mark.png` |
| Recording canvas | Text and marks only |
| Processing | Status text only |
| First-note payoff | Text-only callout |
| Error recovery | Text-only reassurance |

The floating mark reflects capture state and toggles the active Pad. It never owns capture lifecycle. Its secondary controls may Pause/Resume, Finish, and Open Margins; it never offers Discard.

## Window mechanics

- The native `capture-circle` is the only floating-mark window.
- A dedicated grip is draggable; Margo, More, and expanded controls remain interactive.
- Transparent gaps pass pointer events through using DOM-rectangle hit testing in logical coordinates.
- Placement is remembered in logical points and clamped back onto reachable monitors.
- The mark is always above ordinary windows while capture is active.
- Reduced-motion mode removes the recording-ring pulse.

## Migration

| Deprecated | Outcome |
| --- | --- |
| `margo-base.png` | Renamed to `welcome.png` (same pixels) |
| `margo-recording.png` | Deleted |
| `margo-weaving.png` | Deleted |
| `margo-note.png` | Deleted |
| `margo-recoverable.png` | Deleted |
| `src/lib/memo-typing-state.ts` | Deleted; it existed only for pose dismissal |
