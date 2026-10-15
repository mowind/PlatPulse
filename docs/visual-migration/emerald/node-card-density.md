# Node card density verification

> Historical equal-height measurements are retained for comparison, not as the
> current acceptance contract. Issue #236 supersedes cross-grid equality, six
> Home metrics and reserved empty-state height. See the final record below.

## Scope and method

Baseline: `7872545234bc2fa7f0fe48550b16fbf77326a0df`. Implementation: `eb06cb3`.

Measurements use headless Chromium, browser zoom 100%, the unchanged application
font, root font size 16px, viewport height 900px, and production assets served by
the isolated existing Playwright harness on port 4173. They are **controlled
fixtures, not measurements of the operator's authenticated production Nodes**.
Hydra, Chimera and Satyrs each have six populated Validator metrics; Sync and Sync
LEB have authoritative empty metrics. Widths and values are held constant for the
before/after comparison. Independent rank freshness is explicitly pinned to fresh
in the final recorder to avoid the harness clock making it stale between runs.

The reproducible recorder is [measure-node-density.mjs](../../../platpulse-web/scripts/measure-node-density.mjs).
It logs browser geometry rather than asserting implementation details. Run from
the WebUI directory while its existing test harness is running:

```sh
node scripts/measure-node-density.mjs
DENSITY_SCENARIO=status node scripts/measure-node-density.mjs
DENSITY_SCENARIO=status DENSITY_FONT_SIZE=20 node scripts/measure-node-density.mjs
```

## Actual cause

- Old identity minima: 124px on wide cards and 140px on narrow cards, despite the
  old multi-line synchronization/association content already having been removed.
- Old chain minima: 160px / 240px. Grid's default stretch distributed their surplus
  over the metric rows, producing approximately 43.67px / 39px rows.
- Healthy cards still emitted an invisible empty diagnostic row.
- The external auto-fill grid and auto-rows-fr are necessary for cross-row equality
  and were not removed or changed.

Only intrinsic region content is observed; equalized minima apply to separate
outer wrappers. This prevents an old maximum feeding back into future measurements
and lets real wrapping/data/font changes increase or decrease all cards together.
Resource minimum space (112px) and normal Validator metric space (116px / 176px)
are retained. No font sizes, field formatting, themes, map, overview cards, filters,
sorting or column-width rules changed.

## Normal-fixture before / after (CSS px)

All five cards had identical dimensions at each measured width, including the
partial final row. Order remained Hydra, Chimera, Satyrs, Sync, Sync LEB.

| Viewport width | Card width | Card height before → after | Identity height before → after | Metadata bottom → CPU before → after | Chain region before → after |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1440 | 408 | 548 → 415 | 124 → 68 | 66 → 10 | 160 → 83 |
| 1280 | 408 | 548 → 415 | 124 → 68 | 66 → 10 | 160 → 83 |
| 768 | 362 | 548 → 415 | 124 → 68 | 66 → 10 | 160 → 83 |
| 390 | 358 | 704 → 523 | 140 → 68 | 82 → 10 | 240 → 131 |
| 360 | 328 | 704 → 523 | 140 → 68 | 82 → 10 | 240 → 131 |

Wide-card consensus rows: approximately 43.67px → 22px; row gap 8px → 2px;
row pitch approximately 51.67px → 24px. Three paired rows remain Head/QC,
Locked/Committed, Txs/Peers. The existing narrow-card safety fallback below the
22.5rem container threshold is retained (five rows, with Txs/Peers paired); those
rows changed from 39px to 22px. Long-number fallback also remains unchanged.

The card widths above are the deviation-23 22.5rem grid track. Deviation 26 later
returned the grid minimum to Emerald's own 300px; the 22.5rem container threshold
described here is unchanged, so a 303px auto-filled card now takes the narrow
five-row fallback.

The two major content-region gaps remain 10px. Section top offsets relative to
card top, shared by every card:

| Layout | Resources before → after | Chain before → after | Validator before → after |
| --- | ---: | ---: | ---: |
| Wide cards | 124 → 68 | 246 → 190 | 416 → 283 |
| Narrow cards | 140 → 68 | 262 → 190 | 512 → 331 |

Full Validator content measures 113px wide / 173px narrow inside unchanged
116px / 176px available regions. Empty cards retain that same region and print
only `No validator metrics` for the authoritative empty state. Other states retain
their distinct wording and last-good/partial data handling.

## Actual stress measurements

Status fixture combines a Resyncing Node, a long wrapping unhealthy diagnostic,
a failed source retaining six values, a loading empty source and a failed empty
source. These are intentionally heterogeneous cards in the same grid.

| Viewport width | All five heights, 16px root | All five heights, 20px root |
| --- | ---: | ---: |
| 1440 | 460 | 566 |
| 1280 | 460 | 566 |
| 768 | 476.5 | 566 |
| 390 | 594.5 | 746.46875 |
| 360 | 594.5 | 746.46875 |

For all 15 normal/status/enlarged-text combinations, measured card heights,
resource/chain/Validator top offsets match across all five cards. Each complete
card retains six metric cells; each empty card has zero placeholder metric cells.
The recorded metric-label/value horizontal-overflow lists are empty, and Validator
content extends 0px below its card. Loading and failed-empty text stay distinct;
failed last-good metrics stay populated. This checks 20px root-font enlargement,
not every OS text setting or browser zoom level.

## Executed checks

- Lint and TypeScript checking passed.
- Complete frontend unit suite passed (27 files, 385 tests).
- Focused browser matrix passed: 76 passed, 4 intentionally skipped (the 1920px
  check runs only in desktop-1440). Five files across all five fixed projects:
  Home overview layout, Node business metrics, Home Resync, Linked Validator,
  and Home convergence. This includes light/dark, touch/keyboard disclosures,
  full Node-detail navigation, long values, sorting and filtering.
- Initial runs caught obsolete assertions requiring the removed empty diagnostic
  element and old empty-state prose. Those existing expectations were updated;
  health explanations, state distinctions and detail interactions remain tested.
- Production build passed, with the existing large-chunk warning. Unit runs also
  emit existing React act-environment warnings.
- No Rust, entire repository E2E suite, or authenticated operator-data acceptance
  is claimed: this change is WebUI-only and the browser run is the focused matrix.

## Two-axis review

### Standards

0 documented-standard violations. One optional low-priority duplicated-wrapper
smell (four intrinsic-content wrappers); kept local rather than expanding this
layout-only task with an additional abstraction.

### Spec

0 confirmed violations. Shared region measurement preserves section alignment,
external equal-width/equal-height rules and data/status semantics. The runtime
stress checks above additionally verify wrapping and enlarged-root-font behavior.

## Runtime note

The existing operator service on port 8080 caches its entry document. Rebuilding
the shared static directory initially removed old hashed assets that this running
process still requested. Exact baseline JS/CSS were rebuilt in a temporary
checkout and restored alongside the new assets; the operator login page was then
verified in Chromium (no asset 404, expected unauthenticated session 401). The
operator service was **not restarted**, so its cached entry still selects the old
UI until the normal deployment/restart procedure loads the new entry. The new
build and test results above refer to the isolated port-4173 harness.

## Later state: issue #237 compact identity, three resource columns and one-line chain

The measurements above describe the Emerald migration's card. The parent Home
compaction (#236) then changed what they record in its first slice (#237):

- **Chain block.** Home keeps the current Block Height alone. The three paired
  rows this record measures (Head/QC, Locked/Committed, Txs/Peers) and the
  five-row narrow fallback are gone, so its chain-region heights (83px / 131px)
  and row pitches describe the old card only. The block is now one compact
  key-value line with no column switch.
- **Resources.** Three fixed columns (CPU, Memory, Disk) with one thin bar each
  in a shared row replace the two-column grid, the full-width Node data row and
  the wide/narrow Node data caption the 112px resource minimum belonged to. The
  used / capacity bytes and the Disk source sentence moved into the identity
  entry; the Peer count and its last-good cue moved to the secondary identity
  line. `MetricRow`'s `wideValue` capability and its `data-value-wide` /
  `data-value-narrow` rules were removed with their last caller.

`platpulse-web/e2e/home-card-finish.spec.ts` logs the current geometry at each
fixed project (CSS px, the harness fixture whose card carries a full Validator
region; light and dark agree):

| Project | Card | Resource region | Chain block |
| --- | ---: | ---: | ---: |
| phone-360-touch | 328 × 452 | 56 | 35 |
| phone-390-touch | 358 × 580 | 56 | 35 |
| tablet-768-touch | 362 × 488 | 56 | 35 |
| desktop-1280 | 303 × 452 | 56 | 35 |
| desktop-1440 | 303 × 452 | 56 | 35 |

The chain block is 35px at every width, against the 160px / 240px minima this
record lists, and the resource region is 56px against its 112px minimum: one
shared bar row under one label/value line per column, with every percentage on
its label's line at all five projects — including the 303px auto-filled card, the
tightest track the grid's own 300px minimum produces. The longest of the three
labels renders as `Mem` (the full `Process memory` name stays the label's title
and the bar's accessible name), which is what holds that same-line contract at
the minimum track in any font; a value that is not a percentage may still take
its own right-aligned line. The card's remaining height is the identity block's
wrapping and the Validator region's own 176px minimum, which the parent's slice 2
(#238) addresses; reaching the parent's 250–290px whole-card target is therefore
not this slice's claim.

## Final Compact Dashboard acceptance — issue #236 (2026-10-14)

Baseline for the six feature slices: `e331389`; slice implementation HEAD:
`6e4e772`. This working-tree acceptance cleanup repairs browser regressions,
updates old contracts and fixes one actual CSS cascade defect; it does not change
Server/Agent, API/DTO, health/freshness policy, collection or Node Detail layout.

### Reproduction and controlled fixtures

The [recorder](../../../platpulse-web/scripts/measure-node-density.mjs) uses the
existing isolated production-asset Playwright harness, not operator Nodes. It
replaces only the Public networks response through the existing routed-page seam.
Normal fixtures retain three Home Validator metrics, current Peer evidence,
process CPU/Memory, Node data used/capacity, upload/download and an evidence-backed
`At observed head`. Node-level freshness is an ISO receipt timestamp;
Peer freshness is the separate `current` enum. Ordinary fixtures contain no
fabricated last-good cue. Status fixtures retain long names/diagnostics, Provider
failure with last-good values, missing metrics, current negative, no Link, stale
negative and valid Resyncing rather than hiding exceptions.

`DENSITY_MATRIX=1` captures 40 combinations: five exact CI viewports × Light/Dark
× ordinary/status/20px-root-text/reduced-motion. The complete raw measurements are
[geometry.json](issue-236/geometry.json). Run from the WebUI directory against the
disposable existing harness on port 4186:

```sh
E2E_PORT=4186 DENSITY_MATRIX=1 \
  DENSITY_OUTPUT=../docs/visual-migration/emerald/issue-236/geometry.json \
  DENSITY_SCREENSHOTS=../docs/visual-migration/emerald/issue-236/home \
  node scripts/measure-node-density.mjs
```

These are browser geometry and screenshots, not a timing experiment with human
readers. The system font and existing text sizes are unchanged. The 20px mode
increases root-relative sizing, not every OS text setting or browser zoom level.
Existing browser acceptance also covers 200% zoom.

### Final ordinary complete Validator dimensions

Light and Dark have the same geometry; all seven ordinary cards retain three
Validator cells, current Peer metadata and the valid reference comparison.

| Fixed viewport | Columns | Card width × height (CSS px) | Natural structure |
|---|---:|---:|---|
| 360 × 800 touch | 1 | 328 × 292.5 | Two-column summary; Rewards on the next row |
| 390 × 844 touch | 1 | 358 × 292.5 | Same two-row summary |
| 768 × 1024 touch | 2 | 362 × 292.5 | Same two-row summary |
| 1280 × 800 | 3 | 408 × 246.5 | Three metrics on one row |
| 1440 × 900 | 3 | 408 × 246.5 | The unchanged 1280px shell does not force four columns |

250–290px is an aim, not a lower bound or cropping rule. A fully current desktop
card is naturally 246.5px; adding blank space to reach 250px would contradict the
content-driven contract. Narrow cards exceed 290px by 2.5px to keep the third metric
on a second readable row. There is no font shrink, fixed high minimum or clipping.
The ordinary chain remains 35px and resources/single-line Speed region 55.5px.

At both desktop widths the status row with a long diagnostic and extreme reward
reaches 337.5px; unrelated rows remain 246.5px. At 360px cards independently range
from 224.5px (identity-only) to 364px (long diagnostic); no empty metric placeholder
is reserved. At 20px root text ordinary cards grow to 302.7–380.2px and track counts
reduce as needed. All 40 records report no page horizontal overflow, metric clipping
or Validator content beyond its card. Reduced-motion geometry is unchanged.

A separate routed browser regression constrains the real Home `main` container
to 600px without changing its viewport, adding an internal measurement hook or
altering shell CSS. All five fixed projects in both themes keep one readable Node
track (568px at wider viewports): desktop viewport width does not force several
columns into an actually narrow container.

### Actual overflow repair and browser contract corrections

The extreme reward cell declared its wide-value intent, but the three-column
third-cell rule had higher CSS specificity than the full-row rule. A browser probe
measured 115px available / 134px scroll width before the fix; the corrected
parent-qualified selector lets it use all 376px with no overflow. The existing
long-value browser test now checks actual whole-row geometry.

Tests replace all-card/cross-row region equality with same-row bottoms and a
before/after check that a wrapped diagnostic grows only its own row. The formerly
undefined row helper is local and executable. Unknown rank fixtures explicitly
carry unknown evidence and exact accessible reasons, not inherited `Unranked`.
A confirmed non-Validator has no Node Detail performance panel; a real Validator
retains Unranked and all six detail metrics. No coverage is removed to force a pass.

The complete matrix also exposed a shared summary-title assertion reading the
new empty marker span as six extra titles; it now scopes the actual title span.
Two release zoom checks sampled the map canvas before ResizeObserver ran: a
browser probe measured viewport 320px / scroll 344px immediately after resize,
then scroll 320px and canvas width 288px after two animation frames. The existing
zoom seam now waits for that rendering cycle, without relaxing the no-overflow
assertion or introducing a sleep, a test hook or a production-layout change.

### Screenshot visual review

Twenty unmodified grid captures cover ordinary/status at every fixed viewport and
in both themes. They are document-space full-page clips: the sticky header is not
hidden/changed or accidentally painted over the first card by scroll-into-view.

- Ordinary desktop: [Light](issue-236/home-normal-1440-light.png), [Dark](issue-236/home-normal-1440-dark.png).
- Mixed/exception desktop: [Light](issue-236/home-status-1440-light.png), [Dark](issue-236/home-status-1440-dark.png).
- Ordinary phone: [Light](issue-236/home-normal-360-light.png), [Dark](issue-236/home-normal-360-dark.png).
- Mixed/exception tablet: [Light](issue-236/home-status-768-light.png), [Dark](issue-236/home-status-768-dark.png).

Agent visual review checks quiet normal cards; health mark by the name and Activity
on the right; readable CPU/Memory/Disk columns, distinct Speed arrows, single Head
line and three-summary/identity-only variants. Diagnostics and data failure attract
attention without stretching unrelated rows. Unknown is not zero or Non-validator;
Observing is not an Agent-offline verdict. This is screenshot review, not a measured
three-second human usability study. Final user acceptance of that qualitative goal
is explicit rather than fabricated.

### Verification status

Five-project ordinary/mixed-card and constrained-container regressions pass
(10 + 5 cases, both themes inside each case). Web lint/typecheck, 991 unit tests and
the production build pass. The complete fixed-project suite ran 1200 cases in
34.9 minutes: 896 passed, 298 conditionally skipped and six failed. The failures
were the summary-title selector and the pre-ResizeObserver zoom measurement in
the two phone projects, using test modules loaded before those in-session fixes.
The exact six failures were replayed with the final test code: **6 passed, 0 failed**
(14.4s). The corresponding three regressions also pass in all five projects
(**15 passed**, 40.2s). Initial failures are not erased or described as an
unconditionally green full run. A fresh run of every affected browser spec,
including the new constrained-container case, completes across all five projects:
**141 passed, 4 skipped, 0 failed** (7.3 minutes).

Local HTML reports (generated artifacts, not tracked design evidence):

- Full initial matrix: [report](../../../platpulse-web/playwright-report/issue-236-final/index.html).
- Exact six-failure rerun: [report](../../../platpulse-web/playwright-report/issue-236-rerun/index.html).
- All affected specs after fixes: [report](../../../platpulse-web/playwright-report/issue-236-affected/index.html).

Standards and Spec follow-up reviews have no remaining material findings. Two
independent AI visual reviews directly inspected all 20 screenshots with no visual
blocker. These are local working-tree results; the cleanup must be committed and
integrated before treating the remote implementation as ready to close #236.

## Second-round Home refinement acceptance (2026-10-15)

Baseline: `6e4e772` ("feat(web): Home Disk、Uptime 与 Attention First 稳定排序 (#242)").
Implementation: the same day's working tree. The recorder, fixtures and viewport
set above are unchanged, so the two records are directly comparable.

This round changed no layout rule. It shortens an exceptional Node's diagnostic to
one line, makes the Health marker three-state, gives the Attention tile its own
short reading and an accessible explanation, simplifies a confirmed Non-validator's
identity line, and separates the map's Geo-database freshness from its Peer
observation freshness.

### Measured effect on the exceptional card

The 40-record matrix was re-run against the same controlled fixtures. **30 of the
40 records are identical to the #236 record**, including every `normal` mode
(16px light/dark, 20px root text, reduced motion): 360 → 328 × 292.5, 390 →
358 × 292.5, 768 → 362 × 292.5, 1280/1440 → 408 × 246.5, identity 69px, metadata
16px, resources 55.5px, chain 35px, summary rows 1 at desktop and 2 below it.

The ten differing records are all `status`, and inside them only the exceptional
card changed — because the Server's multi-sentence reason now renders as one short
line instead of wrapping across two or three:

| Fixed viewport | Exceptional card height | Identity region | Ordinary cards in the same run |
|---|---:|---:|---|
| 360 × 800 | 364 → 331 | 140.5 → 107.5 | unchanged (224.5–292.5) |
| 390 × 844 | 354 → 321 | 130.5 → 97.5 | unchanged (224.5–292.5) |
| 768 × 1024 | 354 → 321 | 130.5 → 97.5 | unchanged (224.5–308.5) |
| 1280 × 800 | 337.5 → 321 | 114 → 97.5 | unchanged (246.5) |
| 1440 × 900 | 337.5 → 321 | 114 → 97.5 | unchanged (246.5) |

The exception still grows only its own row (321px against 246.5px ordinary desktop
cards) and the growth is now one readable line: 16.5px at desktop and 33px on
phones, where the old wrapped diagnostic cost 71.5–73.5px. All 40 records report
no page horizontal overflow, no clipped metric and no Validator content beyond its
card, in both themes.

### Map chip states

A routed probe (not committed) drove the two freshness dimensions independently
against the real Server payload: Geo current + Peer current shows no chip; Geo
stale + Peer current shows `Map data stale`; Geo current + Peer stale shows
`Peer observations stale` on its own; an expired country attribution with a
current Geo state and current Peer observations shows `Map data stale`, because
the retained attribution belongs to the Geo dimension; both stale names both
dimensions. Every warning case renders the theme's warning colour with a warning
dot, keeps exactly one visible line in the `role="status"` element, and carries
the whole two-dimension explanation plus the last-good statement as its `title`
and as an `sr-only` description.

### Verification status

Web lint, typecheck and 997 unit tests pass. The complete fixed-project suite ran
1200 cases in 35.4 minutes: 902 passed, 298 conditionally skipped and 5 failed —
all five were `home-card-finish.spec.ts:138` in the five projects, whose assertion
the run had loaded before it was corrected for the simplified Non-validator line.
Replaying that spec with the final test code passes **15 passed, 0 failed** (33.6s),
so no browser failure remains open. The initial failure is recorded rather than
described as an unconditionally green run. Standards and Spec reviews
(cliproxyapi/gpt-6.1-sol, high) report no remaining findings on the final tree.

