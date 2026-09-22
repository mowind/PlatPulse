# Node card compact equal-height verification

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
