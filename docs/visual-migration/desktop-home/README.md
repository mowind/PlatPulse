# Desktop Home refinement — browser evidence

## Scope

Only the desktop toolbar/control layering, map total's explanatory label, and
Node data resource width changed. Map geometry/options, 2×2 summary, grid column
policy (`auto-fill`, not `auto-fit`), other metrics, Validator badge and mobile
layout are unchanged.

- Opaque surfaces are applied to the **individual desktop controls**, including
  the Sort label, not to the whole toolbar. Network tabs and Sort share a normal
  flex row and have matching vertical centers (32px visible surfaces; 44px Sort hit target).
- Browser hit testing found a real conflict: at 1280/1440/1920, Australia's
  marker was inside the select (around y=351). Merely making the control opaque
  would hide an interactive marker. `lg:pt-10` locally moves the toolbar down
  40px, also clearing the select’s extended hit target. The desktop wrapper is static so the transparent gap does not intercept
  map events. The canvas is neither resized nor made pointer-inert.
- The compact total now has an accessible name and native desktop title:
  **Peer records in scope**, not unique Peers or Node deployment locations.
  Mobile retains the badge's original pointer-inert behavior.
- `md:col-span-2` makes Node data full width only at the desktop breakpoint.
  The existing MetricRow still owns label/value/bar/detail and Unknown handling.

## Real captures, fixed UI-only data

`before-*.png` were captured **before source changes**; `after-*.png` were
captured from the rebuilt production bundle in Chromium. These are browser
screenshots, not mockups. Both use the same frozen Public DTO from
`../mobile-map/networks.json`, with explicit UI-only overrides: 1/4/8 Nodes,
a long name, alternating known/unknown resources, and 42 Peer records at the
Server-shaped Australian country representative point. This is not a claim
about production observations or Node locations. SSE is paused for repeatability.

| Case | Before | After |
| --- | --- | --- |
| 1440, 1 Node, light | [before](before-1440-1-light.png) | [after](after-1440-1-light.png) |
| 1440, 1 Node, dark | [before](before-1440-1-dark.png) | [after](after-1440-1-dark.png) |
| 1440, 4 Nodes | [before](before-1440-4-light.png) | [after](after-1440-4-light.png) |
| 1440, 8 Nodes | [before](before-1440-8-light.png) | [after](after-1440-8-light.png) |
| 1280, 1 Node | [before](before-1280-1-light.png) | [after](after-1280-1-light.png) |
| 1920, 1 Node | [before](before-1920-1-light.png) | [after](after-1920-1-light.png) |
| Australia hover | [before: blocked](before-tooltip-light.png) | [after: tooltip](after-tooltip-light.png) |
| Sort interaction | [before](before-sort-light.png) | [after](after-sort-light.png) |
| Mobile 390, light | [before](before-390-1-light.png) | [after](after-390-1-light.png) |
| Mobile 390, dark | [before](before-390-1-dark.png) | [after](after-390-1-dark.png) |

The Sort capture is taken after `Alt+ArrowDown` on the focused native select;
native OS popup rendering is browser/platform dependent. The test additionally
clicks, dismisses, selects Name using the keyboard and verifies the selected
value. We did not replace the native menu with a decorative custom menu.

## Acceptance

Run from `platpulse-web` against `e2e/start-server.sh` on port 4173:

```sh
npm run build
node scripts/desktop-home-evidence.mjs after
```

The runner serves the current `dist/index.html` via a page route because the
long-running test Server caches its entry HTML; assets and authentication still
use the real Server. No production code or DTO contract is changed by the runner.

`before-metrics.json` / `after-metrics.json` record 42 cases each:
360, 390, 768, 1024, 1280, 1440, 1920 × 1/4/8 Nodes × light/dark.
The after runner asserts:

- no horizontal overflow; Sort receives real pointer hits;
- desktop CPU/Memory share two equal columns; Node data matches content width,
  left label/detail, right value and full-width progress;
- toolbar controls have equal vertical centers;
- a single card does not fill the desktop row;
- unknown Node data has no progressbar;
- business metric rows remain vertically ordered;
- eight desktop Australia hover cases open the tooltip via actual painted-pixel
  pointer input; selecting the network still updates map scope;
- native Sort mouse/keyboard interaction still selects Name.

The two 390px before/after screenshots (light and dark) are byte-identical.
The 1440px card remains 303px wide and 385.15625px tall; Node data expands from
129.5px to 271px, with unchanged metric content/typography and card height.

Additional checks on the final build:

- `npm run lint`, `npm run typecheck`, `npm test`: passed (26 files, 304 tests).
- `npm run build`: passed; existing large-chunk advisory remains.
- `home-geo-map.spec.ts`, desktop-1280 + desktop-1440: 14 passed, 4 deliberately
  skipped by the suite's project-specific guards. Includes real map/tooltip,
  filter scope, projection exception states, transparent surface and 44px hit
  target checks.
- `mobile-map-responsive.spec.ts`, phone-390-touch: passed, including
  320/360/390/430 widths, rotation, painted-marker/region taps, tooltip dismissal
  and unknown/stale/disabled/error states.

Those existing Playwright suites used a temporary local Vite preview on 4174
with the real 4173 API proxied, to avoid the Server's cached entry HTML; the
preview/config were removed after the run. No security or runtime configuration
in the repository was changed.
