# Margo assets

Margo has exactly two deliberate product roles. The live floating mark uses one
derived layer so the approved paper can stay pixel-identical while its face is
animated in the renderer.

| File | Role | Usage |
| --- | --- | --- |
| `welcome.png` | Welcome illustration | Home intro / brand empty state at 132px |
| `mark.png` | Approved canonical static mark | Capture-circle fallback and visual source of truth |
| `mark-paper.png` | Approved faceless runtime layer | Capture-circle paper beneath the inline SVG face |

`mark-paper.png` changes only the face pixels from `mark.png`. It preserves the
paper, red margin, blue rule, fold, proportions, and transparent silhouette.

## Policy

- Do not add pose variants (`margo-recording`, `margo-weaving`,
  `margo-note`, `margo-recoverable`, and similar).
- Recording, processing, first-note payoff, and error recovery in the main UI
  are text-only. Margo's runtime presence is the capture-circle plus welcome.
- Do not alter `assets/logo.png`, `desktop/public/logo.png`, or platform icons.
