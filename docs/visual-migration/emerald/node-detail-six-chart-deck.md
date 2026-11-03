Part of the [Emerald visual migration record](README.md): sections 32–36. Sections keep their numbers; image and evidence links are relative to this directory.

## 32. Node Detail six-chart port (issue #225): gates, bundle and Playwright

- `npm run lint`, `npm run typecheck`: pass on final source.
- `npm test`: **1,049 passed**, 54 files — 50 files / 1,011 tests before the
  port, so 38 cases come from this change: `src/components/metricChartAxis.test.ts`
  (14), `src/components/nodeMetricChartOption.test.ts` (13), the
  `// @vitest-environment node` `src/components/nodeMetricChart.test.ts` (8),
  which drives the real engine with `renderer: 'svg', ssr: true` and proves the
  window geometry (lines and bars alike), the break at a silence, the absent zero
  and the marker sizes against arithmetic this file derives from the option
  module's own grid — `convertToPixel` is a cross-check, never the expectation —
  and `src/components/echartsCore.test.ts` (3) for the loader memo.
- Production build: pass. Entry chunk `dist/assets/index-*.js` 1,193.34 kB
  (1,193,347 bytes on disk; gzip 306.80 kB, the figure Vite itself reports; the same file through
  `gzip -9` is 303.33 kB, so a differing gzip number is a compressor setting
  rather than a different revision) against 1,189.79 kB (gzip 305.67 kB) before the port, and
  every ECharts chunk byte-identical, because the loader already imported
  `echarts/charts`, `echarts/components` and `echarts/renderers` wholesale for
  the map (deviation 15).
- Playwright, the specs this change touches — `node-detail-six-charts`, `theme`,
  `release-candidate`, `linked-validator`, `emerald-refinement`,
  `emerald-decisions` and `visual-evidence` → **195 passed, 0 failed,
  5 skipped** (8.5 min, exit 0) over the five fixed projects and both themes;
  the skips are the pre-existing viewport/explicit-evidence conditions. The
  first runs were not green and the record is kept rather than tidied: the first
  run of the Validator/evidence group reported **20 failed / 45 passed**, every
  failure in `linked-validator`, because moving the diagnostics out of the
  performance card left the spec looking for `details` *inside*
  `getByRole('region', { name: 'Validator performance' })` (`element(s) not
  found` at `e2e/linked-validator.spec.ts:190`, `:277` and `:391`); scoping those
  lookups to the disclosure itself (`page.locator('details[data-slot="disclosure"]',
  { hasText: 'Validator diagnostics' })`) fixed three of them, and a later run
  still failed the ten cases that read the full-precision reward through the card
  (`detail.getByText('Cumulative rewards (full precision)')` at `:350`, now
  `disclosures.getByText(…)`). That is a test catching a real
  information-architecture move, not a broken chart. A single `emerald-refinement`
  failure in between — the Home map's `[data-slot="geo-chart"] canvas` not visible
  on desktop-1280 within 5 s while the same test passed on the other four projects —
  did not reproduce in the two later full runs and is recorded as a flake in the
  on-demand ECharts chunk load rather than a regression from this change.
- Three findings only a real browser produced, all fixed here: a bar series makes
  ECharts 6 widen the shared time axis by half a band, so the two block charts
  disagreed with the four lines about the Server's own window (deviation 32 (a));
  the pointer reaches the crosshair only when the plot is centred first, because
  on the 360/390 projects the sticky shell header otherwise sits over the point
  the pointer is sent to; and an unconfined tooltip scrolls the whole page
  sideways on a 360px phone, which `expectNoHorizontalOverflow` measures
  (deviation 29 (i)).
- No Rust implementation, API contract, auth logic or Server trust boundary was
  changed: the Public metrics response and its coverage judgement are untouched.
  Changes remain local; nothing was pushed and no PR was created. (Committed to `main` as
  `a2d28ea` after this entry was written.)

## 33. Second refinement round on the six-chart deck: gates, browser evidence and the two review findings

This round changed only the six chart cards, their ECharts options and the axis arithmetic the
brief asked for; the page shell, the map, Validator performance and Diagnostics are untouched
(the round touches 12 files — Web presentation, its e2e specs and these two docs — and its exact
`+/-` count is not quoted here, because correcting this paragraph moves it). The deviation register entry above is
the contract; this section is the evidence.

- `npm run lint`: **exit 0**. `npm run typecheck`: **exit 0**. `npm test`: **1,057 passed**, 54
  files, eight more cases than deviation 32's 1,049 — the network ceiling ladder's boundaries and
  its 4,001-point peak sweep, the integer tick steps, the per-axis unit formatter, the literal
  series colours, the bar recipe and the memory gradient's `0.08 → 0.01`.
- Production build: **exit 0** (the exit code is now read from the build itself, not from the
  `tail` in the pipeline that used to hide it). Entry chunk `dist/assets/index-*.js` 1,195.23 kB
  (gzip 307.57 kB) against 1,193.34 kB (gzip 306.80 kB) at the end of deviation 32; every ECharts
  chunk byte-identical, as in deviation 32, because the deck rides the loader the map already
  paid for.
- Playwright, the six-chart spec alone → **35 passed, 0 failed** over the five fixed projects,
  including the fourteen-step `normal / unknown / stale / single-CPU-failure × light / dark`
  matrix (28.9 s on desktop-1440) and the two bar/line geometry cases. The spec was run twice:
  once for the card and option work (3.9 min), and again (4.0 min) after the headroom redesign
  below, against a rebuilt bundle. In the same session `release-candidate` plus the six-chart spec
  → **65 passed, 0 failed** once the finding below was fixed.
- One honest record of a failure this round earned and the harness taught: the first run after the
  last edits reported **5 failed** (one per project) at `e2e/node-detail-six-charts.spec.ts:319`,
  the new secondary-reading assertion, with `secondarySize: 'none'` on all six cards — not a
  product defect. Playwright runs with `reuseExistingServer: !process.env.CI`, and a
  `platpulse-e2e-9u2wCK` Server from an earlier run was still serving an **older** bundle, so the
  new `data-slot="node-metric-value-secondary"` marker was simply not in the served DOM. The
  stale Server and its fixture refresher were stopped, the run repeated against a freshly built
  bundle, and the attribute is now asserted from that bundle rather than assumed.
- The second failure was the test's own bug, and it is worth stating because it looks like a
  product hang: asserting the clock labels with `plots.locator('svg').first()` waits **forever** in
  the `unknown` and `stale` scenarios, where all six cards mount
  `[data-slot="node-metric-chart-empty"]` and no chart exists at all (180 s test timeouts on all
  five projects). The assertion now reads the axis text from the charts that *do* exist
  (`evaluateAll` over mounted `svg`s) and demands a clock label from each of them, so a mounted
  chart that lost its `HH:mm:ss` axis still fails while a sampleless scenario does not hang.
- **Review finding P2 (network headroom) — fixed twice, and only the second fix is measured.**
  The first `rateChartMax` rounded *up* from a 12%-above-peak target, and the ladder's coarse
  `6 → 8` and `12 → 16 MiB/s` jumps turned that into **33%** headroom at a 6 MiB/s peak (45% at
  5.5), against the brief's 10–20%; the test then accepted up to 50%, and this register called it
  "about 12%". The replacement — the step *nearest* the 12% target, with a 5% floor — was then
  swept numerically over 400 000 peaks from 1 B/s to 1 TiB/s instead of being reasoned about, and
  it broke its own promise: the ladder's widest rung gap was 1.25×, so a peak landing just above
  such a rung (1 950 B/s, or a real 7.7 MiB/s window between the 8 and 10 MiB/s rungs) still
  received a **31%** ceiling whichever rung was chosen. The rule is now a single line — **the
  first rung that clears the window peak by at least 5%** (`RATE_CHART_MIN_HEADROOM`) — and the
  ladder gained half-step rungs (1.125, 2.25, 4.5, 9, 18) that cap its widest gap at **1.2×**, so
  the headroom cannot leave **[5%, 26%]** (worst case 1.05 × 1.2): 5 MiB/s → 6 MiB/s (20%),
  6 MiB/s → 7 MiB/s (16.7%), 7.7 MiB/s → 9 MiB/s (16.9%), 8 MiB/s → 9 MiB/s (12.5%),
  3 MiB/s → 3.5 MiB/s (16.7%), 1 MiB/s → 1.125 MiB/s (12.5%), 39 010 B/s → 49 152 B/s (26.0%).
  `src/components/metricChartAxis.test.ts` no longer settles for twelve peaks: it sweeps 4 000
  peaks from 1 B/s to 1 TiB/s, asserts that band, and pins the 26% boundary by name. Two harness
  lessons are worth keeping from the sweep: never print inside the loop (the first run emitted
  millions of lines and a 3.5 MB spill file), and measure a ladder rather than reasoning about it
  (a predicted 40% case at 20 466 B/s is really 20.1%). **Superseded by deviation 36 (b):** the
  binary `RATE_CHART_MULTIPLES` ladder and its half-step rungs (1.125, 2.25, 4.5) no longer exist —
  the rate axis steps in the decimal ladder of the unit its own peak chose, because a half-step rung
  under a large unit prints `2.8125 GiB/s` — and that ladder's widest rung gap is 1.5×, so the
  `[5%, 26%]` bound above is historical. The measured bound is 31.2% at four and five bands and
  40.0% at three (cap `31.2%` = `1.05 × 1.32`).
- **Review finding P3 (Download's reading) — fixed.** The second direction on the network card was
  inheriting the 10px of its caption wrapper; both directions are current readings, so the number
  is now 12px (`text-xs`, `data-slot="node-metric-value-secondary"`) with only the direction's
  *name* at 10px, and the e2e asserts that exactly one card carries a secondary reading and that
  it is 12px.
- One standards-review judgement call was **not** taken and is recorded as such: the primary and
  secondary readings in `pages/HomePages.tsx` still have two prop shapes (`value` / `valueLabel` /
  `direction` / `directionColor` against `{ label, value, direction, color }`). A shared
  `MetricReading` type would need a mode branch in the header's layout to stay compact, since the
  secondary reading is a second line rather than a field of the first; the duplication was judged
  cheaper than that branch.
- Necessary differences from `LoadChart.vue` that this round did **not** close, and why: the deck
  still writes no missing-value zeros, still cuts one series per run with `connectNulls: false`,
  still keeps a single observation as a marked run, still shows an empty state as a DOM sentence
  with no chart at all, and still derives its window from the Server's own coverage judgement; its
  rate axis is unified to one unit per axis where upstream lets a per-value formatter mix them;
  the legend stays on the two multi-series cards only; and a Process CPU above 100% keeps its own
  ceiling instead of upstream's pin at 100. These are the deviations 29 and 32 (a)–(e) still in
  force, not regressions.
- `docs/design/webui.md` was synchronized in the same round: the coral authorization (line 789)
  now names **CPU and Process memory** as the two series it covers and cites deviation 33 (f), the
  plot heights (line 832, and the three viewport paragraphs at 843–845) and the acceptance list
  (line 860) state the new header recipe, the clock labels, the single-unit rate axis and the
  integer axes.
- No Rust implementation, API contract, auth logic or Server trust boundary was touched, and the
  round's changes were made directly on `main` and committed there (the hash is deliberately not
  recorded in this file: writing it in would change it); nothing was pushed and no PR was created.

## 34. The coverage verdict moves into the chart's accessible description (issue #225 follow-up)

**Requested after the second refinement round, from the live page.** The reader
pasted the two lines a card was showing under its title —
`1 gap in this window · No samples for the last 3 seconds` above
`161,350,108 → 161,350,107` — and asked for that kind of hint to go. Both are
PlatPulse's own vocabulary, not Emerald's: `LoadChart.vue` has no absence to
name, so the sentence was the one element that made a PlatPulse card taller and
busier than the card it is ported from.

**Scope decided with the reader before any edit** (its own question, because the
card's footer holds three different kinds of line):

- **Deleted** — the visible coverage paragraph: `data-slot="node-metric-coverage"`
  (`m-0 text-[11px] leading-4 text-muted-foreground`), the only place the six
  sentences `No samples reported yet`, `No retained observations`,
  `Last observation N ago`, `N gaps in this window` and
  `No samples for the last N` were drawn. The slot does not exist any more at
  any viewport, in either theme, in any scenario.
- **Kept** — the `detail` line (the pasted `161,350,108 → 161,350,107` on the
  Block interval card, `last-good value retained · collection Error` on a failed
  CPU collection, `1 inbound · 2 outbound` on Peer connections), the italic
  `Upload unavailable in this window` direction note, and the
  `No samples in the last minute` empty state. None of them is a coverage hint;
  each answers a different question.
- **Re-homed, not lost** — the sentence itself. It was already appended to the
  chart's `sr-only` description (platpulse-web/src/pages/HomePages.tsx builds it
  and platpulse-web/src/components/NodeMetricChart.tsx renders it as an
  `<p class="sr-only">` that `[data-slot="node-metric-chart"]` names through
  `aria-describedby`), so the verdict is now written once instead of twice: a
  screen reader hears exactly what the sighted reader used to see, and nothing
  about it is derived differently. `coverageNote`/`coverageNotes`
  (platpulse-web/src/metricCoverage.ts) are untouched.

**Why this is a deviation entry rather than only a style tweak.** The visible
sentence was a documented contract of §15.26 (`The card says what the curve
cannot`), asserted in jsdom and in the browser as a *visible* slot. It is now
documented as an accessible-only one, which is the honest description: the
information is still reachable and still asserted, only no longer printed.
docs/design/webui.md §15.26 is rewritten to say so — the bullet that once
described the paragraph's text and position now describes the `sr-only`
paragraph as its only home, and the acceptance bullet that read `the notice … above
the plot and inside its sr-only description` now names the visible slot as
asserted-absent.

**Regression, all green on the shipped source.**

| Check | Result |
|---|---|
| `npm run typecheck` | exit 0 |
| `npm run lint` | exit 0 |
| `npx vitest run` | 54 files / 1,057 tests passed (unchanged from deviation 33: three assertions converted, none added or removed) |
| `npm run build` | exit 0 — `dist/assets/index-*.js` 1,195.11 kB (gzip 307.55 kB), against 1,195.23 kB / 307.57 kB at the end of deviation 33 |
| `npx playwright test e2e/node-detail-six-charts.spec.ts` | **35 passed / 0 failed** (3.9 min, then 4.1 min after the review's per-card fix) over the five fixed projects, including the seven-scenario × two-theme loop |

The conversion is the interesting part. `src/App.test.tsx` asserted the
paragraph's text through `[data-slot="node-metric-coverage"]`; it now asserts
that lookup is `null` and reads the same sentences from the `sr-only`
paragraph by id. `e2e/node-detail-six-charts.spec.ts` asserted the same slot's
text in four places across seven scenarios. The replacement is one assertion
below the scenario chain — the whole deck's slot count is `0` — so it holds for
all seven scenarios rather than for the four that mentioned it, and the sentences
are read back by resolving each plot's `aria-describedby` id
(`chartDescriptions`, a helper in the spec) in the never-observed and expired
scenarios, which still prove the same split they did before (four ledgered cards
saying `No samples reported yet` / `Last observation 40 days ago`, the two
unledgered block cards answered as `No retained observations`) — and proved the
same way they were before: per card. `expectAbsenceVerdicts` pairs each
description with its card's title and requires **exactly one** absence verdict
per card and the one that card's series has earned, because four cards saying
`No samples reported yet` is also what a deck would produce where one card said
it twice, another said nothing at all and a sixth carried both sentences. The gap
scenario still proves `1 gap in this window` and the normal scenario the
unobserved tail on the block card. A deletion that removed the information would
have had to delete those assertions too; they moved, which is the evidence that
only the presentation changed. A read-only review of the round caught the first
version's gap: four inline assertions could not support a claim about seven
scenarios, and the per-card loop one of them added was already covered by the
deck-wide count.

**Visual evidence.** `platpulse-web/test-results/…/node-detail-six-charts.png`
from this run (desktop-1440, single-CPU-failure, light) shows the six cards with
no grey line above any plot: the Block interval card's footer is the heights it
spans, Peer connections' is its two directions, and the CPU card that failed its
collection shows the retained value and its legend with nothing else between the
header and the plot.

## 35. Visual consistency restoration of the six ECharts charts to Emerald LoadChart.vue

**Scope and motivation.** PlatPulse's Node Detail six-chart deck was previously ported to ECharts 6, but retained visual differences from Emerald's `LoadChart.vue` (grid margins, missing Y-axis names and units, separate DOM legends, distinct memory gradient opacity, mismatched inbound peer color). This round completes the visual alignment with Emerald while strictly preserving PlatPulse's unique data semantics.

### Key restorations:
1. **Grid dimensions & outerBoundsMode**:
   - Ordinary charts: `top: 30, right: 24, bottom: 32, left: 56`.
   - Charts with legend (Network, Peer connections): `top: 30, right: 24, bottom: 52, left: 56`.
   - In ECharts 6, `outerBoundsMode: 'none'` ensures the grid rect remains strictly at the configured coordinates, preventing automatic inward shrinkage from axis names.
2. **Y-Axis names & styling**:
   - Restored Y-axis names: `CPU %`, `Memory %`, `Speed`, `Connections`, `ms`, and `Txs`.
   - `nameTextStyle`: `{ color: textSecondary, fontSize: 11, padding: [0, 40, 0, 0] }`, positioned directly above the Y axis.
   - Restored subtle dashed splitLine using Emerald's `splitLineColor` (`rgba(0, 0, 0, 0.06)` light / `rgba(255, 255, 255, 0.06)` dark).
3. **X-Axis distribution**:
   - `splitNumber: 3` on the 60-second time axis produces ~4–5 evenly spaced time labels across mobile and desktop container widths.
   - `axisLabel.margin: 12` matching Emerald's `baseXAxisConfig`.
4. **Native ECharts Legend**:
   - Registered `LegendComponent` in `platpulse-web/src/components/echartsCore.ts`.
   - Native bottom-centered legend: `bottom: 4, itemWidth: 12, itemHeight: 12, itemGap: 20, icon: 'roundRect', textStyle: { fontSize: 11, color: textSecondary }`.
   - Removed external DOM legend (`[data-slot="node-metric-legend"]`).
5. **Curve colors and gradients**:
   - Process CPU and Process memory both use `#FF6B6B` with `0.25 → 0.02` vertical area gradient (matching Emerald CPU and RAM).
   - Host network: Download `#60A5FA`, Upload `#A78BFA` (no area fill).
   - Peer connections: Inbound `#FF6B6B`, Outbound `#4ECDC4` (matching Emerald TCP `#FF6B6B` and UDP `#4ECDC4`).
   - Line properties: `width: 1.5, cap: 'round', smooth: false, animation: false, showSymbol: false` (except single isolated observations).
6. **Tooltip styling**:
   - Tooltip configured with `borderRadius: 6`, `extraCssText: box-shadow: 0 0 0 1px ..., 0 0 16px ...`, `axisPointer: { type: 'cross', lineStyle: { type: 'dashed' }, crossStyle: { color: textTertiary } }`.
   - HTML formatter displays timestamp in bold `textSecondary`, followed by series items with 8px circular colored dots and right-aligned bold values.
7. **Data semantics preserved**:
   - PlatPulse's domain-specific metrics (Process CPU/memory, dynamic max >100%, network binary peak ladder, discrete block intervals/txs, disconnected gap breaks) remain intact without synthetic data interpolation.

**Regression & Verification:**
- `npm run typecheck`: clean (exit 0)
- `npm run lint`: clean (exit 0)
- `npm test`: 54 files / 1,057 tests passed (exit 0)
- `npm run build`: clean (exit 0)
- `npx playwright test e2e/node-detail-six-charts.spec.ts`: 7/7 passed across all 5 projects (`phone-360-touch`, `phone-390-touch`, `tablet-768-touch`, `desktop-1280`, `desktop-1440`).
- Visual comparison with Emerald `LoadChart.vue` captured in `docs/visual-migration/emerald/screenshots/desktop-1440/emerald-vs-platpulse-direct-comparison.png`.


## 36. Rendered-defect pass on the six-chart deck: one unit per axis and a proof that nothing clips

§35 restored Emerald's grid, legend, colours and gradients, and it verified that with
configuration-level tests. This round starts from the screenshots instead, and they showed
four defects that a configuration test cannot see.

### What the screenshots showed

1. **The rate axis was clipped.** Host network ticks repeated their unit (`0 B/s`,
   `977 KB/s`, …), so the tick column was wider than the 56px left margin minus the 6px
   label margin and the leading digits were cut off.
2. **Units were duplicated.** The Process CPU and Process memory headers carried a `%`
   chip while the axis names said `CPU %` and `Memory %`.
3. **The network header was stacked.** Upload above Download, each with its own `Upload` /
   `Download` caption, where the reference keeps one compact row.
4. **The block interval axis contradicted itself.** The axis name said `ms`; the ticks said
   `2s`, `4s`, `10s`.

### Decisions

- **One resolved axis per chart.** `MetricAxisRule = (peak: number, maxBands: number) => MetricAxis`
  and `MetricAxis = { max, interval, name, format }` in `platpulse-web/src/components/metricChartAxis.ts`.
  The axis is resolved once from the data the chart actually draws, so the unit cannot drift
  between the title and the ticks.
- **The unit lives in the axis title; ticks are bare numbers** (`bareNumber()`). This is a
  deliberate deviation from upstream `LoadChart.vue` at `c2c5e88`, which repeats the unit on
  every tick: that repetition is what clipped the rate column, and it is what turned the
  header's `%` into a duplicate. The plot states its unit exactly once, in the title. A card
  therefore reads its unit **twice**: once beside the live value it labels — the header keeps the
  unit its own current value carries, as Emerald's header does — and once over the axis, and never a
  third time on a tick or in a chip of its own. The tooltip keeps each sample with its unit — a
  byte rate on the header's own binary ladder (`2.00 KiB/s`, deviation 39), a duration as retained
  (`1240 ms`).
- **Nice steps inside a measured band budget.** `niceAxisScale(target, maxBands, integer?)` chooses
  whole steps from `[1, 1.5, 2, 2.5, 3, 4, 5, 6, 8] × 10^p` and takes the *lowest* whole-step
  ceiling that divides into a step a reader counts in. `percentAxis` feeds it the `percentChartMax`
  ladder rung for the window's peak as a **target, never the ceiling**: a rung no step divides into
  five bands (700%) moves the ceiling on, so 620% lands on 750% at 150% and 1,100% on 1,200% at 300%
  (`metricChartAxis.test.ts`);
  `metricChartMaxBands(height, showLegend)` derives the budget from the *rendered* chart height
  (`clamp(floor((height - 30 - (legend ? 52 : 32)) / 14), 1, 5)`, `MIN_TICK_SPACING = 14`), so a
  phone-sized card can never ask for more bands than it has pixels for. The fixture peak of 380.95
  lands on 400 at 80 — **5.00%** headroom, so nothing is rounded up for its own sake — while a dense
  sweep puts the worst case at **31.2%** across four and five bands and **40.0%** at three. Both
  bounds are asserted in `metricChartAxis.test.ts` rather than reasoned about: the decimal ladder's
  widest rung gap is 1.5×, and paying that on a short phone plot is cheaper than the `2.8125 GiB/s`
  labels a half-step rung would print under a large unit. Both helpers live in `metricChartAxis.ts` beside the axes
  they feed (they import the margin constants from `nodeMetricChartOption.ts`, never the reverse),
  so the band budget is covered by the module's own unit tests rather than only by a browser run.
  The rung-as-target rule is pinned end to end by an SSR test in `nodeMetricChart.test.ts`: the
  renderer prints `CPU (%)` with ticks `0/150/300/450/600/750` for the `percentAxis('CPU')(620, 5)`
  that the old ladder collapsed onto `0` and `700`.
- **Duration switches unit, never mixes them.** `durationAxis('Duration')` keeps an `ms` title
  with whole-millisecond steps below 5,000 ms and switches to a `(s)` title with a seconds scale
  ×1000 above it. The tooltip stays `formatMillisecondsSample` (`1,240 ms`); sampling is untouched,
  so no missing sample is zero-filled and no curve is bridged across a gap.
- **The axis name sits inside the plot, not over the gutter.** `nameLocation: 'end'` with
  `padding: [0, 0, 0, METRIC_CHART_AXIS_NAME_INSET - METRIC_CHART_LEFT_MARGIN]` anchors the title
  at x=4 of the container, above the tick column rather than beside it.
- **The network header is one row of readings.** `MetricReading = { label, value, direction, color }`
  rendered with a lucide chevron coloured to its curve; the `Upload` / `Download` captions became
  `sr-only` labels and the plot height is unchanged because the readings share the header row
  (they wrap below 390px instead of shrinking the chart).
- The Grid constants were **not** touched — §35 had already matched them to Emerald.

### Where the Emerald comparison stands

Upstream is not vendored, so the same-width comparison is the recorded geometry register in the
acceptance spec (`EMERALD`, from `LoadChart.vue` at `c2c5e88`: `chartMargin` 30/24/32/56, the
`+legend` bottom of 52, 11px axis labels at margin 12 on x and 6 on y, a `h-48` = 192px plot box)
read against measured `DOMRect`s at the running project's own width. There is no Emerald render in
this repository to re-measure, and the one same-width artifact that exists,
`screenshots/desktop-1440/emerald-vs-platpulse-direct-comparison.png`, predates this round: its
PlatPulse column still shows the old per-tick units and the stacked network header, so it is
**stale for the axis and header questions** and is not evidence for them.

Reading the upstream source settles what upstream actually does with units, and it is more than the
rendered deck suggests: the CPU header prints `12.5` in one span and `%` in the next, the CPU
`yAxis` is `name: 'CPU %'` with `formatter: '{value}%'` on every label, and the network header
prints two `formatBytesSplit(...).value` + `.unit` + `'/s'` readings on one row behind
`tabler:chevron-up` / `chevron-down`. Upstream therefore states the CPU unit **three times**
(header, axis name, every tick). This round keeps the header's own unit — the hierarchy Emerald
uses for CPU, memory, disk and network alike, and the one the committed contract already asked for —
and states that unit once in the axis title with bare ticks, which is what removes the duplicated
`%` without a unitless `5.0` that would read as a different metric. The alternative (bare header
value, unit only on the axis) was considered and declined for exactly those reasons.

### Verification

- `npm run typecheck`, `npm run lint`, `npm run build`: clean.
- `npm test` is Vitest over `src/` and does **not** cover `e2e/`: 54 files / 1,060 tests passed. The
  Playwright suites below are the only proof for the browser assertions. The three added tests are
  the `metricChartMaxBands` ladder, the scalar `percentChartMax` probes, and the SSR render of
  `percentAxis('CPU')(620, 5)` that prints `CPU (%)` over `0/150/300/450/600/750`; `App.test.tsx`
  also derives the chevron colours from `METRIC_SERIES_COLORS` instead of hard-coding two `rgb()`
  strings, so a palette change can no longer leave the assertion passing against an old colour.
- New `platpulse-web/e2e/node-detail-chart-visual-acceptance.spec.ts`: a fixed fixture for the six
  cards, and for every visible X/Y text a check that its measured box stays inside the chart
  container — run on all five projects (`phone-360-touch`, `phone-390-touch`, `tablet-768-touch`,
  `desktop-1280`, `desktop-1440`) at 390/768/1440/1920px **plus the width the running project itself
  fixes** (`measuredWidths(testInfo)` adds `testInfo.project.use.viewport.width`, so
  `phone-360-touch` really measures 360px and `desktop-1280` 1280px against the widths
  `docs/design/webui.md` names) in Light and Dark, with cases for a long
  network unit, a large peer count, a block interval above 5,000 ms and CPU above 100%. The spec
  also pins the plot box itself (left 56, right width−24, top 30.5, bottom height−32 or −52), that
  every tick centre lands on a split line, that the value labels and the clock band stay in
  separate rows, and that the tooltip opens over the plot rather than behind a sticky header. It
  waits for the deck to settle before measuring (`settledDeck`, so a resize mid-paint cannot be read
  as a clipped label: its poll requires the drawn grid's left edge, its bottom edge **and its right
  edge** `chart.width - 24` to sit where the current viewport puts them, because a width change
  resizes the card before ECharts repaints and the right edge is the only edge that moves with it),
  proves the axis title clears the top tick, maps every clock label onto its
  wall-clock instant on the real time axis within 1.5px, and checks that each card's drawn curve
  starts and ends on the plot's own left and right edges. Per-card screenshots at 390 and 1440px in
  both themes are attached to every run; `EMERALD_EVIDENCE=1` additionally writes them and the
  `geometry-*` records into this directory.
  `npx playwright test e2e/node-detail-chart-visual-acceptance.spec.ts`: 10 passed across the five
  projects (6.7–6.9s for the geometry test, 1.6–1.7s for the tooltip test, on each).
- `npx playwright test` (the whole suite, all five projects, on this build): **922 passed, 298 skipped,
  0 failed** in 36.5 min (exit 0). The 298 skips are the suite's pre-existing project gates
  (`test.skip(testInfo.project.name !== 'desktop-1280', 'the process-heavy flow runs once')`) and
  environment gates (`test.skip(!enabled, 'set EMERALD_EVIDENCE=1 to capture visual evidence')`),
  none of them in the chart specs: the named regression spec
  `e2e/node-detail-six-charts.spec.ts` is 35 passed (7 tests × 5 projects) after its two rewritten
  assertions.
- Measured geometry (fixture, Light): every card is 384.0×192.0 at 1440 and 334.0×132.0 at 390; the
  plot is 56→360 × 30.5→160.5 without a legend and →140.5 with one at 1440, and 56→310 × 30.5→100.5 /
  80.5 at 390, with six split lines (five on Block interval). Titles and ticks: `CPU (%)`
  0/30/60/90/120/150, `Memory (%)` 0/20/40/60/80/100, `Speed (GiB/s)` 0/0.6/1.2/1.8/2.4/3
  (0/1/2/3 at 390), `Connections` 0/300/600/900/1,200/1,500 (0/500/1,000/1,500 at 390),
  `Duration (s)` 0/15/30/45/60, `Txs` 0/800/1,600/2,400/3,200/4,000.
- Evidence: [chart acceptance screenshots](screenshots/chart-acceptance/desktop-1440) — the whole
  deck at four widths, per-card screenshots at 390 and 1440 in both themes, and the measured
  `geometry-light-1440.json` / `geometry-light-390.json` records behind the numbers above. The
  chart-region captures and the geometry records are deliberately narrower than the width matrix:
  390 and 1440 carry them, 768 and 1920 are covered by the whole-deck screenshot alone, and the
  per-width assertions run at every accepted width either way.
