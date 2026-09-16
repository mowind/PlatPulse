# Emerald visual migration — provenance, mapping and deviations

This directory is the evidence and provenance record for the WebUI visual
migration decided in [ADR 0002](../../adr/0002-webui-emerald-visual-authority.md).
It holds the upstream reference, the component mapping, the asset licences, and
the register of every place where PlatPulse deliberately differs from upstream.

## 1. Reference revision

| | |
|---|---|
| Repository | https://github.com/Tokinx/komari-theme-emerald |
| Commit | `c2c5e88ea19c7cbe18d14a50414e10deca3cc66e` |
| Tag | `v1.0.12` (2026-09-10) |
| Licence | MIT, Copyright (c) 2026 Tokinx |
| Inspect | `git clone https://github.com/Tokinx/komari-theme-emerald && git -C komari-theme-emerald checkout c2c5e88` |
| Source of record | `src/styles/main.css` (tokens, `@theme inline`, base layer), `src/components/ui/**` (`cva()` variants), `src/components/NodeCard.vue`, `src/components/NodeGeneralCards.vue`, `src/components/NodeEarthMaps.vue`, `src/views/HomeView.vue` |

The repository publishes no lockfile, so upstream's *declared* ranges are the
available evidence:

| Package | Upstream range | PlatPulse resolved |
|---|---|---|
| `tailwindcss` / `@tailwindcss/vite` | `^4.1.16` | 4.3.3 |
| `class-variance-authority` | `^0.7.1` | 0.7.1 |
| `clsx` | `^2.1.1` | 2.1.1 |
| `tailwind-merge` | `^3.4.0` | 3.7.0 |
| `echarts` | `^6.0.0` | 6.1.0 |
| `tw-animate-css` | `^1.4.0` | 1.4.0 |
| `vue-sonner` | `^2.0.9` | React `sonner` 2.0.8 |

## 2. Vendored assets — sources and licences

Nothing in this table is fetched at runtime. The Server sends
`default-src 'self'; script-src 'self'; connect-src 'self'` on every response,
so a runtime CDN request cannot succeed even if it were attempted.

| Asset | Source | Licence | Regenerate |
|---|---|---|---|
| `public/assets/geo/world-countries-echarts-www-v1.json` (987 KiB, 217 polygons, 26,273 points) | `https://raw.githubusercontent.com/apache/echarts-www/4e9b6889abf0995b1784610a907cdb8821f59996/asset/map/json/world.json` — the file upstream Emerald fetches (unpinned) from four CDNs | Apache-2.0 (Apache ECharts); underlying outlines from Natural Earth (public domain). Licence copy: `public/assets/geo/apache-echarts-www-LICENSE.txt` | `node scripts/vendor-emerald-assets.mjs` (checksum-verified: `049b3345…d2fa`) |
| `public/assets/flags/*.svg` (271 files, 1.9 MiB) | `flag-icons@7.5.0`, `flags/4x3/` | MIT, copyright Hampus Joakim Borgström. Licence copy: `public/assets/flags/LICENSE-flag-icons.txt` | same script |
| `lucide-react` | npm | ISC | — |
| `sonner` | npm | MIT | — |
| `class-variance-authority`, `clsx`, `tailwind-merge`, `tailwindcss`, `tw-animate-css` | npm | MIT | — |
| `echarts` | npm | Apache-2.0 | — |

**Geometry difference (recorded, not silent).** Upstream fetches
`apache/echarts-www@master` at runtime; PlatPulse vendors the same file at a
pinned commit. The replaced PlatPulse asset, Natural Earth 1:110m simplified
with Douglas-Peucker 0.14° (`world-countries-110m-v1.json`, 87,812 B, 175
countries, 7,366 path commands), is retired with the SVG renderer it fed.

**Flag difference (recorded, not silent).** Upstream's `/assets/flags/{code}.svg`
files are served by the Komari host and are **not in its repository**, so their
file naming and aspect ratio cannot be verified from public sources. PlatPulse
vendors `flag-icons` at the 4:3 aspect ratio and lowercases the ISO code at the
call site, because PlatPulse's `countryCode` is uppercase.

## 3. Upstream → PlatPulse component mapping

| Emerald (Vue) | PlatPulse (React) | Status |
|---|---|---|
| `src/styles/main.css` `:root`/`.dark`/`@theme inline`/base layer | `src/styles/emerald.css` | verbatim |
| `ui/button/index.ts` | `components/ui/button.tsx` | verbatim variants |
| `ui/input/Input.vue` | `components/ui/input.tsx` (`Input`) | verbatim |
| `ui/badge/index.ts` | `components/ui/badge.tsx` | verbatim variants |
| `ui/alert/index.ts` | `components/ui/alert.tsx` | verbatim variants |
| `ui/card-x/CardX.vue` | `components/ui/card-x.tsx` (`CardX`) | verbatim |
| `ui/progress-thin/ProgressThin.vue` | `components/ui/progress-thin.tsx` | verbatim + unknown-percentage rule |
| `ui/spinner/Spinner.vue` | `components/ui/spinner.tsx` | verbatim |
| `ui/empty/Empty.vue` | `components/ui/empty.tsx` | verbatim |
| `ui/data-tooltip/DataTooltip.vue` | `components/ui/data-tooltip.tsx` | ported (behaviour verbatim) |
| `ui/dialog/*` (reka-ui) | `components/ui/dialog.tsx` (Radix) | ported |
| `ui/tabs/*` (reka-ui) | `components/ui/tabs.tsx` (Radix) | ported |
| `ui/back-top/BackTop.vue` | not adopted (PlatPulse has no back-to-top affordance) | deviation |
| `ui/sonner/Sonner.vue` | not adopted (PlatPulse's notices are persistent, never transient) | deviation |
| `ui/avatar/*` | not adopted (no PlatPulse use) | n/a |
| `components/Background.vue` | `components/BackgroundDecoration.tsx` | ported verbatim (fixed 1300px wash) |
| `components/Header.vue` | `layouts/HomeLayout.tsx` header | ported (h-14 sticky, blur on scroll) |
| `components/Footer.vue` | `components/AppFooter.tsx` | metrics ported, copy replaced |
| `components/NodeGeneralCards.vue` | `components/HomeDashboard.tsx` top area | ported (12-col: stats left, map right) |
| `components/NodeGeneralCards.vue` finance tiles | not adopted (no PlatPulse field) | deviation |
| `components/NodeCard.vue` | `components/HomeDashboard.tsx` `HomeNodeCard` | ported |
| `components/NodeEarthMaps.vue` + `utils/echartsWorldMap.ts` | `components/GeoWorldMap.tsx` + `components/mapChartOption.ts` on `echarts` | ported (option literal; geometry pinned locally) |
| `components/NodeEarthGlobe.vue` (`cobe`) | not adopted — `maps` mode only | deviation |
| `views/HomeView.vue` | `components/HomeDashboard.tsx` | ported |
| `views/InstanceDetail.vue` | `pages/HomePages.tsx` Node Page | adapted (see deviation 16) |
| `—` | Admin pages (no upstream counterpart) | Emerald tokens, primitives, spacing and table rules only |

## 4. Deviation register

Every entry is a place where PlatPulse intentionally differs from upstream, with
the reason. Nothing here is a silent divergence.

| # | Deviation | Reason |
|---|---|---|
| 1 | `--status-healthy/warning/critical/unknown` alias Emerald's `--success`, `--warning`, `--destructive`, `--muted-foreground`. No new hues. | Emerald models only online/offline. PlatPulse must express degraded, unknown, stale and never-observed distinctly, and cannot show them as `0`/`false`/Healthy. |
| 2 | Home top row keeps PlatPulse's four counters (Active Nodes, Healthy Nodes, Attention, Networks) instead of Emerald's six resource tiles. | Upstream's "remaining value" tile has no PlatPulse field (no price/expiry/plan/billing data exists), and summing Host memory/disk/traffic across Nodes would double-count Hosts shared by several Nodes and mix Networks. Layout, card shell, typography and spacing still follow upstream. |
| 3 | The map plots **Peer records by country** using Server-provided country centroids. | Upstream plots server node locations. Transplanting its semantics would misrepresent PlatPulse's statistic; a country centroid is never a Node's or Peer's real position. |
| 4 | Map geometry and flags are vendored from pinned sources instead of fetched from `jsdelivr`/`fastly`/`gcore`/`raw.githubusercontent`. | `connect-src 'self'` forbids it, and the decision records the version rather than tracking `@master`. |
| 5 | Flags are `flag-icons` 4:3, not the Komari host's own files. | Upstream's flag assets are not published in its repository, so their naming and aspect ratio cannot be verified. |
| 6 | System font stack only; `@fontsource-variable/inter` removed. | Emerald loads no webfont. The previous WebUI split (Inter globally, `system-ui` on Home) is resolved to upstream. |
| 7 | No shadcn Select/Table/Tooltip/Skeleton/Dropdown exist upstream. PlatPulse's admin surfaces get dedicated primitives built from Emerald's tokens and typography. | Upstream has no admin surface to copy; inventing none would leave admin pages on the retired sheet. |
| 8 | Emerald's FX providers, visitor-geolocation providers, Iconify runtime API, footer links, brand and ICP reference are not adopted. | Out of PlatPulse's product scope and blocked by CSP; upstream brand must not be carried over. |
| 9 | The Tailwind patch level differs (upstream declares `^4.1.16`, no lockfile; PlatPulse resolves 4.3.3). | Upstream publishes no lockfile, so an exact patch cannot be reproduced; the token block, `@theme inline` mapping and class strings are copied literally. |
| 10 | Interactive controls keep a 44×44 minimum (`min-h-11`, `min-w-11` in the primitives) instead of Emerald's compact 24–36px scale. | PlatPulse's existing accessibility contract asserts every visible interactive control is at least 44×44 (e2e/helpers.ts `expectVisibleInteractiveTargets`), and the brief requires preserving touch operation. Colour, radius, typography, spacing and state treatment still follow upstream. |
| 11 | The Home statistics slot holds four counters in a 2×2 grid (`col-span-6 row-span-2`) rather than Emerald's six tiles in a 3×2 grid. | Consequence of deviation 2. The slot's position, size, card shell and typography are upstream's. |
| 12 | The Home Node card keeps the grey non-Healthy health marker (issue #141) and carries the exceptional signal on the card's red ring and diagnostic line. | The existing tested contract is that only Healthy is green and every other state is grey, so health is never colour-only; Emerald's red marker encodes online/offline, which is a different dimension from PlatPulse Node health. |
| 13 | The map prints the exact per-country Peer count in the scatter label, as upstream does, instead of PlatPulse's previous four-glyph abbreviation. | Upstream's label formatter is the raw aggregate. Bug-for-bug fidelity was chosen over the earlier abbreviation, which is a deliberate quality change to disclose: a count of five or more digits can overrun the 14px disc. |
| 14 | The map's per-country keyboard activation is replaced by a screen-reader country list; the chart container is a labelled `role="img"`. | ECharts paints into a canvas with no per-country element, so upstream's map has no keyboard path at all. Every observed country, its count, its stale count and whether it could be plotted remain available as text, and abnormal states stay announced. This is a different accessibility mechanism, not a claim of parity. |
| 15 | ECharts is loaded on demand (dynamic import) rather than in the entry chunk. | The library is ~530 KiB minified; only Home needs the map. Login, Admin, Network and Node Detail keep the previous bundle size. |

| 16 | The Node Detail page keeps PlatPulse's accepted "A container": an uncarded identity block, four summary tiles and three parallel observation panels, rather than upstream's single-instance card stack. | Upstream's InstanceDetail is built for one Komari instance; PlatPulse's page carries six 60-second charts, per-dimension collection/freshness/value state and public/Admin key partitioning. The visual language (tokens, card shell, type scale, spacing, table and chip treatment) is upstream's; the information architecture is the one the project already accepted in ADR-free design review and asserts in tests. |
| 17 | Every page is now on one stylesheet: `src/index.css` (5,386 lines) and `src/pages/AdminOverview.css` (291 lines) are deleted. | Built CSS fell from 170.31 kB to 70.17 kB (gzip 30.28 -> 12.37 kB) and no retired token reaches the bundle. This is the outcome the earlier entries were migrating towards; recorded here so the size change is accounted for. |
| 18 | `ui/back-top` and `ui/sonner` are not adopted. | PlatPulse has no back-to-top affordance, and its notices are persistent status regions rather than transient toasts; adding either would be a new feature rather than a migration. |
| 19 | Both map layers use the full canvas with `preserveAspect: 'contain'`. The desktop canvas has an independent 2:1 aspect ratio and Emerald's upward one-sixth offset instead of inheriting the 200px summary content height. | Automatic sizing prevented cropping but shrank the painted world to 40.4% of its available width at a 1280px viewport. Separating canvas height raised this to 77.7%; removing ECharts' implicit 20% padding with explicit contain fitting uses over 90% without cropping or stretching. The approved 37:61 tracks, 232px summary band, mobile canvas, geometry and Peer semantics stay intact. Regression: `src/components/mapChartLayout.test.ts` checks all shipped geometry points and marker alignment; `e2e/map-size.spec.ts` checks painted browser pixels at 1280/1440/2390px. |

## 5. Screenshot evidence

`screenshots/` holds the before/after captures for the acceptance viewports
(360×800, 390×844, 768×1024, 1280×800, 1440×900). They are evidence only: no
`toHaveScreenshot` baseline is recorded or re-recorded, so no regression can be
hidden by refreshing a baseline.

## 6. Verification results (actual, 2026-09-14)

### Web checks — all green

| Check | Result |
|---|---|
| `npm run lint` | pass (no output) |
| `npm run typecheck` | pass (no output) |
| `npm test` | **23 files, 249 tests pass** (was 22 / 260) |
| `npm run build` | pass; CSS 70.17 kB (gzip 12.37 kB), entry JS 695.37 kB (gzip 194.01 kB) |
| retired sheet gone | `--paper`, `--admin-panel-bg`, `--home-content-width`, `metric-row-progress`, `dashboard-node-card` all absent from the bundle |

CSS fell from 170.31 kB to 70.17 kB when the legacy sheet was deleted, which is the direct evidence that the second design system is gone.

### Playwright — NOT passing: 327 passed, 156 failed, 57 skipped, 30 did not run (31.1 min)

Run with `npm run test:e2e` against a freshly built production bundle served by the real Server, across all five projects (360×800, 390×844, 768×1024, 1280×800, 1440×900). **This suite does not pass and is not claimed to.** The 156 failures group into four causes:

1. **Assertions that encode the pre-migration visual values (~40).** `theme.spec.ts` and `home-geo-map.spec.ts` assert the old palette and geometry directly: the pre-mount canvas hex, an atmosphere that "spans the full viewport width" (upstream's wash is a fixed 1300px, so the earlier full-width adaptation is what made that pass), specific CSS colours, and "the map sits above the statistics" (Emerald puts the map in the right six columns on desktop). Fixing these means restating the expectations from Emerald's values — a deliberate acceptance change, not a regression fix.
2. **One spec still drives a native confirm (~5).** `access.spec.ts` calls `page.on('dialog', …)` for the Site Access Mode change. The migration replaced that `window.confirm` with an in-app Radix dialog (unit tests were updated, this spec was not), so the mutation never fires. Mechanical fix.
3. **Real regressions (16 + 8).** Sixteen failures are the platform 44×44 interactive minimum, on Admin and Node-detail controls the page migrations introduced; eight are a mobile horizontal overflow inside AdminSettings' CardX panels at 360px (the log reports a 346px child inside a 328px content column). Both are genuine defects of this migration and both need source fixes.
4. **Timeouts and cascades (~90).** Failing clicks and visibility waits concentrated in the specs that already failed for reasons 1–3, plus 30 tests that never ran because a shared fixture aborted.

The failure taxonomy above is the honest state: the migration's own web checks are green and its screenshot evidence exists, but the end-to-end suite has not been reconciled with the new design system.

### Screenshot evidence

`screenshots/<project>/<page>.png` — 25 captures (login, Home, Network detail, Node detail, Admin overview) across the five projects, 3.4 MB. Captured with `EMERALD_EVIDENCE=1 npx playwright test e2e/visual-evidence.spec.ts`. There is no `toHaveScreenshot` assertion anywhere in the repository, so these are evidence and never a gate. **They have not yet been reviewed against upstream's rendering**, so no fidelity claim is made.


## 7. Run history (three consecutive full runs, ~30 min each)

| Run | passed | failed | skipped | did not run |
|---|---|---|---|---|
| after the migration | 327 | 156 | 57 | 30 |
| after the 44x44, 360px-overflow, dialog and three restatement fixes | 359 | 129 | 57 | 25 |
| after the tab-role, aria-selected, oklab, checkbox and reduced-motion fixes | **405** | **96** | 64 | **5** |

The largest single win was a real regression: the migration dropped
prefers-reduced-motion support, because upstream Emerald ships no such block
while the retired sheet carried six !important rules. Restoring it in the design
system removed 33 failures and cleared 20 of the 25 tests that had been aborted
by cascade.

What still fails (96), and why it needs judgement rather than another sweep:

1. Assertions that encode the pre-migration design, roughly 60. theme.spec card
   CSS values (15 toHaveCSS), shell.spec geometry (10 toBeGreaterThanOrEqual),
   home-geo-map.spec composition, release-candidate.spec,
   node-detail-six-charts.spec. Each has to be restated from Emerald's own
   values and recorded as an acceptance change.
2. home-geo-map.spec:728 "stacks the statistics over a compact map on narrow
   screens" (4). This is an open design decision, not a defect: upstream's
   mobile composition pulls the statistics up over the map's lower band
   (-mt-42), while this assertion requires the map to sit entirely above the
   statistics. The measured overlap is map-bottom 441 against statistics-top
   290. Either upstream's overlap is accepted and the assertion restated, or the
   overlap is dropped.
3. expectVisibleInteractiveTargets still reports one undersized input (8). Every
   primitive carries a 44px minimum now - Input, Textarea, Select and Checkbox
   were each verified in the source - so the offending element could not be
   identified from the log alone. The next step is a targeted locator dump, not
   a guess.
4. The rest are click and visibility timeouts cascading from 1 and 2.

## 8. The largest remaining class is narrower than it looks

The 15 toHaveCSS failures are **not design mismatches**. The values agree; only
the serialization does. Measured evidence from theme.spec.ts:396 on the Home
statistics card:

    Expected: "rgba(255, 255, 255, 0.6)"
    Received: "oklab(1 0 0 / 0.6)"

oklab(1 0 0 / 0.6) is white at 60% alpha - the same colour the assertion means,
because bg-background/60 resolves through Emerald's --background (oklch(1 0 0))
and Chrome now serialises that as oklab. The same pattern was already fixed once
for the dark grid fill of the background atmosphere.

So this class should be settled by making the comparison notation-agnostic -
resolve both the expected and the computed value through the browser and compare
components, or accept either spelling in the matcher - **not** by restating the
expected colour. The assertion keeps pinning the exact colour and alpha; it stops
depending on how a browser chooses to spell it.

Two concrete items were also closed here:

- The AdminSettings Geo provider radios were four 16px native
  input[type=radio] instances rendered from one template. They are 44px now,
  like the checkbox. A custom indicator that keeps a 16px visual dot inside a
  44px row was left as follow-up here; section 29 completes it (deviation 10).
- The input reported by expectVisibleInteractiveTargets was identified with a
  temporary locator dump against a live server, not by guessing. That dump is how
  the line above was found; nothing in the log alone named the element.

## 9. Decision: the narrow-screen statistics-over-map overlap

Upstream Emerald's narrow-screen composition pulls the statistics up over the
map's lower band with -mt-42 (10.5rem, 168px). The pre-existing PlatPulse
assertion required the map to sit entirely above the statistics, which the
upstream composition cannot satisfy.

**Decision (owner-approved, option A): keep upstream's overlap.** The covered
band carries no marker, label or control, so the readability guarantee the old
assertion was protecting is not what is at stake - a control being covered is.
The assertion was therefore restated to pin upstream's geometry instead of
prohibiting the meeting:

    const statsOverlap = mapBox.y + mapBox.height - stats.y
    expect(statsOverlap).toBeGreaterThan(0)
    expect(statsOverlap).toBeLessThanOrEqual(169)
    expect(mapBox.y).toBeLessThan(stats.y)

Measured overlap at 360x800: 151px, inside the 168px band. Verified by running
that test on phone-360-touch and phone-390-touch: both pass.

What this decision does NOT license: any overlap that grows past the upstream
band, or any Node card or interactive control being covered. Those would still
fail.

## 10. Colour comparison is now by value, and it paid off immediately

e2e/helpers.ts gained 'expectComputedColor', which resolves both the expected
and the computed colour to sRGB components through a 1x1 canvas. Verified
against this project's own headless Chromium:

    rgba(255, 255, 255, 0.6) -> fillStyle accepted, pixel [255,255,255,153]
    oklab(1 0 0 / 0.6)       -> fillStyle accepted, pixel [255,255,255,153]
    oklch(1 0 0)             -> fillStyle accepted, pixel [255,255,255,255]

So the sampler is sound and the notation question is closed. The six colour
comparisons in theme.spec.ts and node-detail-six-charts.spec.ts now use it, and
'borderless' cards are genuinely border-style: none (Tailwind's preflight leaves
border-style: solid everywhere, so 'bordered={false}' previously produced a
zero-width solid border rather than no border).

With notation out of the way, the remaining background-color mismatches turned
out to be a different problem, and the helper now reports it plainly:

    oklab(1 0 0 / 0.6) must equal rgb(255, 255, 255)
    oklab(0.141 0.00136333 -0.00481054 / 0.613119) must equal oklch(0.141 0.005 285.823)

Both expect the OPAQUE colour while the element is at 60% alpha: the assertion
is looking at the hovered state and the element is not hovered. That is the next
thing to chase - either the hover assertion's mouse path and timing, or
'hover:bg-background' genuinely not applying on the card. It is now a
well-labelled failure instead of an ambiguous colour diff.

## 11. The colour family is closed; it was never a palette problem

Three findings, each verified against a live server:

1. **Notation.** 'expectComputedColor' now canonicalises every colour token in a
   value through a 1x1 canvas, so a box-shadow written with rgba() and the same
   shadow derived from an oklch token compare equal while the geometry (offsets,
   blur, spread) must still match exactly. Verified by probe: rgba(255,255,255,0.6)
   and oklab(1 0 0 / 0.6) both sample to [255,255,255,153].
2. **Transitions.** The helper polls instead of reading once. A single read
   caught the interpolated value mid-transition - the card surface transitions
   over 150ms - which is why the hover assertions appeared to be palette
   failures. A probe confirmed the hover itself works: the summary card reads
   oklab(1 0 0 / 0.6) at rest and oklch(1 0 0) on hover, restoring on mouse-out.
3. **Borderless meant zero-width, not borderless.** Tailwind's preflight leaves
   border-style: solid on everything, so CardX with bordered={false} and the
   Home node card produced a zero-width solid border. Both now state border-none.

The whole 'public Emerald cards' test now walks: colour, font, borderless,
backdrop, background-image, quiet shadow, transform, hover colour, hover
borderless, hover glow - and stops on the hover transform, expecting
matrix(1, 0, 0, 1, 0, -2) from hover:-translate-y-0.5. That is the single next
thing to look at, and it is one assertion rather than a class of them.

## 12. The 'public Emerald cards' contract passes in both themes

After the fixes in sections 10 and 11 plus four restatements, the largest single
failure cluster (theme.spec 'aligns public Emerald cards and isolates Admin in
light/dark') passes on desktop-1280. Each restatement was forced by a decision
already recorded above:

- The hover lift is asserted by its effect: Tailwind v4 expresses translate-*
  through the individual 'translate' property, while v3 emitted a transform
  matrix. Both are the 2px lift; the helper reads whichever is in play.
- The Admin shell shares the public font now, because the migration resolved the
  old Inter/system-ui split into Emerald's single stack (deviation 6). The
  obsolete ADMIN_FONT constant is gone.
- Admin panels use Emerald's single card surface, so they resolve to the same 60%
  background as the public cards (deviation 2) rather than the retired 68%
  admin surface.
- Admin panels are borderless on all four sides, like Emerald's own cards,
  instead of carrying a 1px border. The four-side check is stronger than the
  single-side colour assertion it replaced.

## 13. Fifth run: 426 passed / 75 failed / 64 skipped / 5 did not run

Progression: 156 -> 129 -> 96 -> 75 failed (passed 327 -> 359 -> 405 -> 426).

Two fixes came out of that run, both verified with targeted re-runs:

- The overlap assertion from decision A is now gated to the stacked layout. At
  wider breakpoints the map and the statistics sit side by side in the
  12-column band, so there is no overlap to measure there and the desktop
  composition is asserted by its own test.
- The Geo provider consequence copy embeds fixed HTTPS endpoints. A URL is one
  unbreakable token, so it widened the 360px page by a couple of pixels; that
  copy now breaks words.

Remaining failures by spec (each count is across projects):

| spec:line | count | what |
|---|---|---|
| home-convergence.spec.ts:41, 140, 199, 237, 266 | 25 | the converged Home card contract: header, both metric rows, summary shell, viewport grid, filtering/sorting, keyboard activation, dark theme |
| shell.spec.ts:83, 237, 311 | 15 | Admin shell semantics, fixed-viewport alignment, keyboard focus ring |
| release-candidate.spec.ts:292, 485 | 10 | Public Geo states; Node Detail continuous reading |
| configuration.spec.ts:44 | 5 | History Window end-to-end save flow (the unit flow was updated, this spec was not re-checked) |
| agent-lifecycle.spec.ts:38, 229 | 4 | Agent priority summary columns; Agent detail independence |
| shell.spec.ts:264 | 2 | Admin shell ultrawide |
| node-detail-six-charts.spec.ts:95 | 2 | the six-chart closure |
| home-geo-map.spec.ts:482, 728, 786 | 4 | map composition per breakpoint |
| convergence-acceptance.spec.ts:328, theme.spec.ts:476, agent-lifecycle.spec.ts:229 | 4 | mixed |

The next single target is home-convergence: 25 of the 75 failures, all in the
old Home card contract, which is the last place where the pre-migration markup
expectations still dominate.

## 14. home-convergence passes: 25 failures cleared, and one was not ours

The whole home-convergence spec now passes on all five projects (25 tests,
48.5s). Three causes:

1. Four network-filter locators still asked for a button named after the
   convergence Network. They are tabs now. (My earlier sweep only covered the
   literal 'All Networks', not the locator built from a constant.)
2. The metric-row abbreviation was displayed instead of the full label. The
   migrated card fits the full label at every fixed viewport, so the full word is
   the visible label now and the abbreviation is never shown; it keeps its
   accessible name.
3. **A pre-existing spec/source drift that this migration did not cause.**
   main's b4b509a ('compact Home metrics') changed the Home Validator row to
   True/False but left home-convergence.spec.ts expecting Yes/No, while d46f0ad
   had already moved Node Detail to True/False on purpose. Verified in git:
   f6a980e had 'Yes'/'No' in both places, b4b509a changed the source only. The
   spec is restated to the vocabulary main adopted; the source keeps main's
   True/False. This is disclosed rather than quietly absorbed: those assertions
   had been red before any of this work started.

## 15. shell.spec: 15 failures down to 2

The Admin shell spec went from 15 failures to 2 (50 passed, 1.3m). Causes:

- The Admin shell element itself had no background (it relied on body), so the
  spec's luminance reading of the shell was black. It now paints bg-background.
- The Admin header carried no surface at rest; it is a light translucent surface
  now, with Emerald's blur still arriving on scroll.
- Admin page headings sat in the card-title tier (18px); they are 24px
  (Emerald's text-2xl) now, which is the page-title tier the spec asserts.
- **The spec's own colour parser assumed rgb()/rgba()** and regex-read the digits
  of an oklab() value as near-black RGB - the same notation problem already fixed
  on the assertion side, this time inside a measurement helper. It resolves the
  colour through a canvas now.
- The Admin workbench no longer expands past Emerald's 1280px content column on
  an ultrawide display (deviation 7); the spec asserts the cap and that the
  column stays centred inside the Admin main area instead.
- The Admin icon is not the second tab stop (the header also carries the theme
  control). The spec walks to it, which is the pattern the public shell's spec
  already used.
- The register-Network panel is an article now, as AdminHome's panels already
  were, and the form proof accepts an article or an article role.

Still failing on desktop only: theme.spec's sibling assertion inside
shell.spec:83. That is the next item.

## 16. shell.spec is green (52 passed)

The last shell failure was the Admin navigation's group label contrast reading
4.35 against an expected 4.5 - on desktop only. The desktop navigation was
bg-transparent, so the spec composited the label against nothing and compared it
with black. The navigation paints bg-background now, which is both the correct
surface for a bordered side column and what makes the measurement meaningful;
the real contrast of muted-foreground on the page background is above AA.

shell.spec: 52 passed, 3 skipped, 0 failed.

## 17. A real regression found while chasing a column order: the stacked table is gone

agent-lifecycle's priority summary asserts TWO different orders, and they are
both correct:

- the thead order (line 44): Agent, Reporting status, Last received,
  Node Inventory, Credentials, Diagnostics;
- the visual order on a narrow screen (line 164), sorted by each cell's
  getBoundingClientRect().top: Agent, Reporting status, Last received,
  Diagnostics, Node Inventory, Credentials.

On main both held: the DOM had Diagnostics last (matching thead), while the
retired index.css turned the table into stacked cards on small screens and
placed the Diagnostics card fourth visually.

The migration kept the data-label attributes but not the stacking rules, so the
migrated table scrolls horizontally instead of stacking, the visual order equals
the DOM order, and line 164 fails on the narrow projects. Restoring the DOM order
to match the visual one was tried and is WRONG: it breaks the thead assertion, and
on main the two orders were deliberately different (DOM order for the header,
visual order for the narrow layout).

**Open regression:** the narrow-screen stacked-card layout for data-label tables
must be re-implemented, with the Diagnostics dimension fourth, so both orders hold
again. This affects every data-label table the migration re-expressed, not only
the Agent summary - the Admin list matrix and the audit tables use the same
pattern.

## 18. The stacked-table regression is fixed

The retired sheet turned a data-label table into a stack of cards below
47.9rem, each cell prefixed by its own label, and the Agent priority summary
kept the product order (identity, reporting and receipt, evidence, inventory,
credentials) by assigning each dimension an explicit order. The migration kept
the data-label attributes but dropped those rules, so a migrated table scrolled
horizontally on a phone instead of stacking - a mobile readability regression,
not just a test failure.

The rules are restored in emerald.css with Emerald tokens, keyed on a
data-stack attribute that AdminAgents, AdminNodes, AdminNetworks and AdminAudit
opt into. The priority-summary test now passes on all five projects (5 passed),
and the build, lint, typecheck and the unit suite are green.

## 19. release-candidate and convergence-acceptance are green (75 passed)

Two causes, both real:

- The Peer insight region was 32px narrower than the card that contains it,
  because CardX always applies its own content padding and the labelled region
  sat inside it. The padding moved inside the region (contentClassName="!p-0"
  plus p-4/p-3 on the section), so the region fills the card - the same shape
  upstream uses for its map panel (content-class="h-full !p-0").
- The Geo provider consequence copy embeds fixed HTTPS endpoints, and
  overflow-wrap (Tailwind's break-words) does not lower an element's min-content
  contribution, so a grid item carried a 314px floor inside a 296px box. The
  item is min-w-0 now.

release-candidate.spec.ts and convergence-acceptance.spec.ts: 75 passed, 0 failed.

## 20. Three more specs green (54 passed); five single failures left

- The three Node Detail observation panels stack until the wide breakpoint, as
  the retired sheet did (they split at 768 before, so a tablet showed three
  columns where the spec expects one).
- The fixed table layout and column shares are restored for the Agent and Audit
  summary tables: without table-layout: fixed, wrapping a long detail resized a
  neighbouring column by 28.8px.
- Two home-geo-map assertions described the retired aspect-ratio map and the
  retired non-overlapping narrow composition. Under decision A they now assert
  upstream's map box and upstream's -mt-42 overlap, with the same band check as
  the other narrow test.

home-geo-map.spec.ts, node-detail-six-charts.spec.ts and admin-list-matrix.spec.ts:
54 passed, 21 skipped, 0 failed.

Still failing (five, each one project):

- theme.spec:476 and admin-overview.spec:179 query a 'Menu' button on
  desktop-1440. The Admin navigation toggle is hidden from lg up, so it is not in
  the accessibility tree there. These are mobile-drawer tests; the retired sheet
  hid the toggle on desktop too, and desktop-1440 is the project this migration
  added, so they likely never ran at 1440 before. They need to drive a phone
  viewport themselves.
- agent-lifecycle.spec:38 and :229 read credential state ("1 active", a Revoke
  button) that earlier specs in the shared Server have already mutated. That is
  test isolation, not layout: the seeded credential is revoked by the time these
  run on that project.
- agent-lifecycle.spec:208 sees a 95px overflow from the closed mobile drawer at
  200 percent zoom: the drawer is off-screen with translate, and the overflow
  helper counts its negative x. The closed drawer needs to be out of the
  measurement, not merely translated.

## 21. The remaining single failures are fixed too

- The Agent identity link was an inline-flex box, which sizes to max-content, so
  it overflowed the narrow Agent column instead of wrapping. It is a full-width
  flex box now, and the "Show full Agent ID" control may wrap inside its column.
- AdminHome's Node Health Summary expansion row lacked the detail-row hook that
  its sibling table in AdminNodes already carried.
- The Admin drawer's closed state is display:none below the wide breakpoint
  rather than merely translated off-screen: a translated box still contributed
  to the overflow measurement under a page scale factor. At the wide breakpoint
  it is the sticky sidebar as before.
- The "Show N additional issues" control could not fit a 320px reflow viewport
  because the Emerald button base is whitespace-nowrap; it may wrap now.
- Two assertions were relocated rather than weakened: the Admin-drawer steps are
  skipped on desktop-1440 as they already were on desktop-1280 (1440 is a desktop
  project and the drawer is a mobile/tablet affordance), and the credential
  summary is asserted by its shape and every count instead of assuming the shared
  Server's seeded credential is still active.

## 22. The Playwright suite is GREEN

npm run test:e2e: **503 passed, 67 skipped, 0 failed, 0 did not run** (15.3 min,
all five projects). Exit code 0.

The full progression, every run a complete suite against the real Server:

| run | passed | failed | did not run |
|---|---|---|---|
| after the migration | 327 | 156 | 30 |
| 2 | 359 | 129 | 25 |
| 3 | 405 | 96 | 5 |
| 4 | 426 | 75 | 5 |
| 5 | 456 | 45 | 5 |
| 6 | 473 | 28 | 5 |
| 7 | 473 | 28 | 5 |
| 8 | 484 | 17 | 5 |
| 9 | 491 | 10 | 5 |
| 10 | 494 | 7 | 5 |
| 11 | **503** | **0** | **0** |

Every failure was attributed before it was fixed: a regression of this migration
(fixed in src), a stale expectation the migration invalidated (restated and
recorded), a pre-existing failure on main (proved in git history and disclosed),
or a test-isolation defect in the shared Server. No assertion was deleted and
none was weakened to reach zero.

## 23. The map is visible in the evidence now, and the scatter question is answered

The first captures showed an empty band where the map belongs. The cause was not
a bug: the e2e Server's seeded Geo provider is **Disabled**, so the map correctly
renders nothing and states "Peer countries · Disabled by server" to assistive
technology. The evidence capture now enables the Local MMDB provider before
capturing, so the screenshots show the map the acceptance is about.

With the map enabled, the 1440 Home capture shows:

- the world map in the right six columns of the top band, the four statistics in
  the left six, which is Emerald's 6/6 composition;
- the map's upstream faint slate fill and its borders;
- **a scatter symbol with its aggregate numeral** (a green dot labelled 10 around
  the seeded country) and the corner counter at "10" - so the scatter does paint
  on real data. The earlier doubt from a pixel scan is resolved: that scan looked
  at a Disabled provider, where there is nothing to paint by design;
- the world vertically cropped inside the band, which is upstream's own
  composition (its map box is 352px inside a 232px band, shifted up by a sixth).

This is an observation of our own render, not a comparison against upstream's. The
side-by-side review is still the one acceptance step not performed, and no
fidelity claim is made until it is.

## 24. First real comparison: our 1440 Home against the local reference image

The repo carries a local design reference, emerald-maps.jpg (2446x1101, a 2x
capture of roughly 1223 CSS px), which is upstream in maps mode. Comparing it
with our own desktop-1440 Home capture:

**Same** - the world map in the right half of the top band with the statistics in
the left half and the toolbar and node-card grid below; the faint slate basemap
with visible country borders; teal scatter symbols carrying white aggregate
numerals; the white rounded tooltip; the card surfaces, radii, muted label
typography, dotted-leader info rows and the table/heading treatment.

**Content differences (expected)** - the reference shows six resource tiles where
PlatPulse shows four counters (deviation 2, approved); it has many observed
countries where the seeded Server has one; its node cards carry ping strips, cost
rows and tags that PlatPulse has no data for.

**Style differences found, to resolve or record:**

1. **The proportional split differs.** The reference image puts the statistics at
   about 37% of the content width and the map at about 61%. The pinned source
   (v1.0.12) is a 12-column grid with the statistics at columns 1-6 and the map
   at 7-12, which is 50/50 and is what this migration implements. The image and
   the pinned source disagree; the source was followed, and the discrepancy is
   recorded rather than guessed at.
2. **The background grid decoration is not visible in our render.** The reference
   clearly shows the inclined 72x56 grid in the upper left. The classes are
   upstream's literals, so this needs a computed-style probe of the grid SVG
   rather than a guess.
3. **The top wash is fainter in our render** than in the reference.
4. **The header affordance differs**: the reference shows the current theme name
   as a chip ("light theme") beside a sun icon; PlatPulse keeps its icon-only
   theme control with an accessible name.

Items 2 and 3 are the only ones that look like defects of this migration. They are
not yet fixed, and no fidelity percentage is claimed.

## 25. A real regression found by the comparison: the whole background was invisible

Comparing the crops exposed it. The pre-paint canvas added to index.html during
this migration set the background on :root (the html element). CSS only propagates
a BODY background to the canvas while the root element has none; because html had
one, body painted its own opaque white in the normal-flow step and covered the
background decoration, which is a z-index:-1 layer painted before it. The wash and
the inclined grid were therefore never visible on any page, and the translucent
card surfaces had nothing to be translucent over.

Fixed by moving the pre-paint canvas to the body element (html.dark body for the
dark theme). A 1:1 crop now shows the emerald-to-lime wash, the inclined 72x56
grid, and the 60%-opacity cards sitting over both - which is the reference
composition.

This also invalidates every screenshot captured before it: the evidence set was
re-captured, and the full suite was re-run because the change touches a surface
every page renders.

## 26. After the background fix

fixing the background exposed one assertion that had been reading a proxy rather
than the thing itself: the pre-mount canvas check read
getComputedStyle(documentElement).backgroundColor. With the canvas colour now
propagating from the body (as it must, for the decoration to be visible), the root
element is transparent and the assertion failed even though the first paint is
correct. It reads the effective canvas colour now - the root's background, or the
body's when the root is transparent - which is what "the first paint is the theme
background" means. theme.spec.ts: 60 passed, 0 failed.

The two style defects from section 24 are therefore fixed: the inclined grid is
visible and the wash reads at the reference's strength. The remaining recorded
differences are the 4-vs-6 statistics (approved content), the map box height, the
header's theme-chip affordance, and the statistics-to-map split where the pinned
source and the reference image disagree.

## 27. Reviewing the rest of the evidence

Login and Node Detail at desktop-1440 were reviewed against the same reference.

**Login** now shows the wash and the inclined grid behind a centred, cardless form
column: muted label typography above each field, Emerald's rounded input surface,
its neutral near-black primary action ("Sign in"), and the theme control in the
corner. That matches upstream's treatment of ordinary primary actions.

One structural difference: **the reference's App shell renders its header, and so
its brand, on every route including the login page; PlatPulse's login page renders
only the theme control.** Recorded rather than changed, because PlatPulse's login
page is its own surface and the e2e asserts its current shape.

**Node Detail** shows the accepted A container: an uncarded identity block, four
summary tiles, three parallel observation panels, then the six 60-second charts.
The Host resources panel carries the "collected once per Agent; shared by every
Node it monitors" note and its own progress bars, so the Host de-duplication rule
is visible on the page rather than only in the data layer. Unknown values read
Unknown, never zero.

Two evidence-quality notes, both capture artefacts rather than product defects:
the charts were caught while their history query was still resolving (the capture
now waits for that placeholder to clear), and the chart loading placeholder is
what the seeded Server shows when no history has been retained yet.

## 28. Human-approved composition decisions

The continuation decision “37/61、232、加页头” resolves the open split, height, and
Login header questions above. Home now uses 37:61 fractional tracks after the
existing 8px gap at md and above, rather than upstream’s equal six-column slots.
The outer band remains 232px including padding; mobile stacking is unchanged.
This is an explicit reference-image exception to the pinned-source composition.

Login now includes the PlatPulse brand/Home link and theme toggle in the 56px
Emerald header, with the same 1280px alignment and scroll blur as Home. The form
sits in the remaining viewport space. No Home queries, Admin controls, auth
changes, or new brand assets are introduced. Earlier sections describe the
historical review state, not unresolved decisions.

Added regression coverage checks the Login banner and access boundary, both
theme layouts and touch targets, the 37:61 measured track ratio, the 232px band,
and unchanged phone track widths. No existing assertions or visual baselines
are removed or re-recorded.

Validation for this slice: lint, typecheck, all 250 unit tests and production
build pass. The five-project run of emerald-decisions, shell, theme,
home-convergence and home-geo-map enumerated 195 scenarios and exited 0 with
no failed tests (viewport-specific skips retained). CI=1 forced a fresh real
Server rather than reusing an old build. The newly captured 768px and 1440px
Home images were inspected for the new split and summary/map fit. This is a
targeted regression run, not a new full-suite result; remaining evidence review
and the checkbox/radio indicator refinement were follow-up work at this point.

## 29. Compact native selection indicators

`Checkbox` and `Radio` now share a native-backed selection primitive. The actual
input remains 44×44px and keyboard-focusable; its pointer-inert, aria-hidden
indicator is 16×16px. CSS follows native checked, indeterminate, focus-visible,
invalid and disabled states, including a disabled fieldset. There is no duplicate
React state and no custom click/key handler. Forced-colors mode deliberately
restores the browser-native control and hides the decorative indicator. This
completes deviation 10’s oversized-checkbox/radio follow-up without reducing
touch targets.

Settings uses the Radio primitive without changing provider selection or save
behavior. Checkbox consumers are retained, currently unrouted Agent recovery
and rotation components; no route or remote-control feature was added to expose
them. Nine primitive tests cover native props/refs, controlled and uncontrolled
state, labels, required validation, disabled fieldsets, form values/reset, radio
grouping and indeterminate state. Browser coverage tests the shared geometry
and radio keyboard behavior on the real Settings route: 16px indicator, 44px
input, activation from the input corner, arrow navigation, visible focus ring,
and forced-colors fallback. Both themes and all five fixed viewports pass.

The explicit evidence run passed all 20 tests and refreshed six pages per
viewport (30 PNGs): Login, Home, Network detail, Node detail, Admin overview and
Settings. These remain review evidence, not screenshot assertion baselines.
Final full-suite validation and the visual-review findings are recorded below.

## 30. The remaining screenshot review found real gaps

All 30 light-theme images were inspected across the five projects. Login,
Settings and Home had no apparent overlapping surfaces. Inspection did not
claim pixel parity with upstream or replace keyboard/geometry assertions.

Three findings required action:

- **Capture readiness:** some Node images contained only Loading Node or
  stopped at the first chart row, and some Network captures ended mid-card.
  Waiting for the absence of Loading metric history before the Node exists
  can succeed immediately. The evidence script now positively requires the
  Node title, six metric cards and summary before requiring settled history;
  Network title/panel and a painted map are also mandatory. Absent route links
  and failed readiness checks no longer silently skip evidence or only warn.
- **Missing-value copy:** the Node directory helper concatenated an optional
  formatter result into a string, producing literal `undefined`. A call-site
  `Unknown` fallback fixes that without changing the shared formatter or
  converting unobserved values to zero. Eight rendered cases cover missing,
  partial, zero, non-zero and retained stale data; the missing cases reproduced
  the exact defect before the one-line fix.
- **Overview responsiveness:** migration commit `ff1c0e3` dropped Node/Agent
  cell labels and phone priority-card styling, leaving unconditional table
  minimum widths; the new Button also made long Node labels nowrap. This is
  a migration regression, not an intentional phone scroller. A new browser
  regression measures cells against the visible wrapper, not merely against
  the document, so a hidden right-hand column cannot pass by hiding overflow.

The earlier full browser run was deliberately stopped on its obsolete build
when these defects were confirmed; it is not reported as a passing run.
Compact Home metric labels still use upstream-style truncation and can be very
short beside large values; that legibility tradeoff is disclosed rather than
claimed as full-label parity. Complete values remain in the semantic content.

### Corrections and re-review

Overview now opts into the existing Emerald phone table stacking and restores
all twelve cell labels. Desktop minimum widths apply only from md; tablet
inner scrolling remains supported. Node names wrap within a 14rem upper bound
without replacing desktop auto table layout. Agent column shares restore main’s
16/13/15/21/22/13 proportions, with Overview’s field order explicitly preserved.
The new test failed before this repair on phone360 and desktop1280; after it,
all five projects passed, and the new plus existing Overview suite produced
35 passes and 10 existing skips. The Spool label now also refuses to shrink
into a vertical string of letters while its value wraps; a one-line-height
geometry assertion protects that correction.

The settled capture run passed all five projects. Re-review confirmed loaded
Node detail with all six charts, disclosures and footer; complete Network
cards/actions/footer; bounded desktop Overview columns; and all fields/actions
in the restored phone priority cards. Phone Overview is necessarily long
(about 11,000px with this fixture). Its PNGs exceed the image tool’s 8192px
side limit, so lossless 2400px vertical slices were reviewed consecutively;
source images were not downscaled or cropped.

The two broad dark Validator rank bars are the existing minimal SVG chart:
comparison to main confirms unchanged rank/bar math, currentColor fill,
opacity, and title/table alternative. Sparse rank samples create broad bars;
this is not a lost chart asset. Compact metric/helper truncation, very tall
phone Overview pages, and tablet-local table scrolling remain disclosed
readability tradeoffs. A fixture can also show a receipt timestamp with null
report sequence; the existing “Never received” sequence sublabel is an
independent semantic ambiguity, not changed in this visual slice.

### Full-run failure attribution and test synchronization

The first complete corrected-build run produced **522 passed, 67 skipped,
1 failed**. Its only failure was `SCN-NODE-LAST-GOOD-REFRESH` on desktop1440
before the initial Node heading was visible. The test injected 503 after the
first request, conflating initialization requests with the intended refresh.
Ten isolated repetitions did not reproduce the timing failure, so the exact
extra-request trigger is not claimed as proven. The fixture now starts
failing only after the initial heading and Peer disclosure are ready,
immediately before its explicit invalidation event. Production refresh/cache
code and every last-good assertion are unchanged. This phase-gated scenario
passed **50 repetitions** (ten at each fixed viewport). No retries, timeout
increases, skips or assertion removals were used to make the test pass.

## 31. Final continuation acceptance

- `npm run lint` and `npm run typecheck`: pass on final source.
- `npm test`: **267 passed**, 24 files.
- Production build: pass, rebuilt by each fresh real-Server E2E harness.
- Final full five-project `CI=1 npm run test:e2e`: **523 passed, 67 skipped,
  0 failed** in 15.7 minutes. Skips are the existing viewport/explicit-evidence
  conditions, not new exclusions.
- Final explicit Overview geometry + screenshot run: **10 passed**; 30 PNGs
  refreshed. Spool’s final unbroken label was checked visually and by measured
  one-line height across all five projects.
- `git diff --check`: pass. Test Server listener released after completion.

Local logs: `/tmp/platpulse-final-unit-confirmation.log`,
`/tmp/platpulse-final-polish-evidence.log`,
`/tmp/platpulse-last-good-gated.log`, and
`/tmp/platpulse-emerald-final-acceptance.log`. The earlier failed/cancelled
runs remain attributed above rather than being relabelled as successes.

No Rust implementation, API contract, auth logic or Server trust boundary was
changed; Rust checks and OpenAPI regeneration were not rerun for this
presentation/test-only slice. Changes remain local and uncommitted; nothing
was pushed and no PR was created.
