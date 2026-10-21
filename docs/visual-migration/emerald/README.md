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
| `ui/data-tooltip/DataTooltip.vue` | `components/ui/data-tooltip.tsx` | ported (bottom-end placement and touch={false} added for header actions; see deviation 40) |
| `ui/dialog/*` (reka-ui) | `components/ui/dialog.tsx` (Radix) | ported |
| `ui/tabs/*` (reka-ui) | `components/ui/tabs.tsx` (Radix) | ported |
| `ui/back-top/BackTop.vue` | `components/ui/back-top.tsx` (`BackTop`) | ported (44×44px touch target) |
| `ui/sonner/Sonner.vue` | `components/ui/sonner.tsx` (`Toaster`, `toast`) | ported |
| `ui/avatar/*` | not adopted (no PlatPulse use) | n/a |
| `components/Background.vue` | `components/BackgroundDecoration.tsx` | ported verbatim (fixed 1300px wash) |
| `components/Header.vue` | `layouts/HomeLayout.tsx` header | ported (h-14 sticky, blur on scroll) |
| `components/Footer.vue` | `components/AppFooter.tsx` | metrics ported, copy replaced |
| `components/NodeGeneralCards.vue` | `components/HomeDashboard.tsx` top area | ported (12-col: stats left, map right; icon hover tier and unit tier aligned via deviation 41) |
| `components/NodeGeneralCards.vue` finance tiles | not adopted (no PlatPulse field) | deviation |
| `components/NodeCard.vue` | `components/HomeDashboard.tsx` `HomeNodeCard` | ported |
| `components/NodeEarthMaps.vue` + `utils/echartsWorldMap.ts` | `components/GeoWorldMap.tsx` + `components/mapChartOption.ts` on `echarts` | ported (option literal; geometry pinned locally) |
| `components/NodeEarthGlobe.vue` (`cobe`) | not adopted — `maps` mode only | deviation |
| `views/HomeView.vue` | `components/HomeDashboard.tsx` | ported |
| `views/InstanceDetail.vue` | `pages/HomePages.tsx` Node Page | adapted (see deviation 16) |
| `views/InstanceDetail.vue` LoadChart/PingChart row | `pages/HomePages.tsx` six-chart deck on `echarts` | ported for the load deck (see deviations 28–31); `PingChart` has no PlatPulse counterpart |
| `src/components/LoadChart.vue` (the six load charts) | `components/NodeMetricChart.tsx` + `components/nodeMetricChartOption.ts` + `components/metricChartAxis.ts` on `echarts` | ported (option literal; colours, gradient, 1.5px stroke and tooltip kept — deviations 28–31) |
| `src/components/LoadChart.vue` realtime tabs (1/6/24/… h) | not adopted — the deck keeps the Server's real latest 60 seconds | deviation (29) |
| `src/utils/echarts.ts` (`use([...])` registration) | `components/echartsCore.ts` (`loadECharts`) | ported (one on-demand entry serving the map and the six charts, canvas + SVG renderers) |
| `src/utils/recordHelper.ts` (`fillMissingTimePoints`, `interpolateNullsLinear`, `cutPeakValues`) | not adopted | deviation (29) |
| `—` | Admin pages (no upstream counterpart) | Emerald tokens, primitives, spacing and table rules only |

## 4. Deviation register

Every entry is a place where PlatPulse intentionally differs from upstream, with
the reason. Nothing here is a silent divergence.

| # | Deviation | Reason |
|---|---|---|
| 1 | `--status-healthy/warning/critical/unknown` alias Emerald's `--success`, `--warning`, `--destructive`, `--muted-foreground`. No new hues. | Emerald models only online/offline. PlatPulse must express degraded, unknown, stale and never-observed distinctly, and cannot show them as `0`/`false`/Healthy. |
| 2 | Home top row fills Emerald's 3×2 six-tile shape with PlatPulse's own six metrics — Active Nodes, Healthy Nodes, Cumulative blocks, Attention, Networks, Cumulative rewards — instead of Emerald's resource tiles. | Upstream's "remaining value" tile has no PlatPulse field (no price/expiry/plan/billing data exists), and summing Host memory/disk/traffic across Nodes would double-count Hosts shared by several Nodes. The two cumulative cards use the Server's per-Network deduplicated Validator summaries; their Cross-Network Validator Overview adds those native-unit numbers without asset conversion, prints cumulative block counts complete, keeps the abbreviated reward tile, and prints each metric's actual scope and `N/M reporting` coverage on the card face, retaining stale and missing-Network-summary notices rather than a `Partial` marker; the denominator covers only Networks whose summary the Server published, zero eligible Validators reads `No eligible Validators`, all summaries missing reads `Coverage unknown`, and no known value remains `Unknown` rather than zero, with exact values plus per-Network coverage still available through each card's Breakdown. Layout, card shell, typography and spacing follow upstream. |
| 3 | The map plots **Peer records by country** using Server-provided country centroids. | Upstream plots server node locations. Transplanting its semantics would misrepresent PlatPulse's statistic; a country centroid is never a Node's or Peer's real position. |
| 4 | Map geometry and flags are vendored from pinned sources instead of fetched from `jsdelivr`/`fastly`/`gcore`/`raw.githubusercontent`. | `connect-src 'self'` forbids it, and the decision records the version rather than tracking `@master`. |
| 5 | Flags are `flag-icons` 4:3, not the Komari host's own files. | Upstream's flag assets are not published in its repository, so their naming and aspect ratio cannot be verified. |
| 6 | System font stack only; `@fontsource-variable/inter` removed. | Emerald loads no webfont. The previous WebUI split (Inter globally, `system-ui` on Home) is resolved to upstream. |
| 7 | No shadcn Select/Table/Tooltip/Skeleton/Dropdown exist upstream. PlatPulse's admin surfaces get dedicated primitives built from Emerald's tokens and typography. | Upstream has no admin surface to copy; inventing none would leave admin pages on the retired sheet. |
| 8 | Emerald's FX providers, visitor-geolocation providers, Iconify runtime API, footer links, brand and ICP reference are not adopted. | Out of PlatPulse's product scope and blocked by CSP; upstream brand must not be carried over. |
| 9 | The Tailwind patch level differs (upstream declares `^4.1.16`, no lockfile; PlatPulse resolves 4.3.3). | Upstream publishes no lockfile, so an exact patch cannot be reproduced; the token block, `@theme inline` mapping and class strings are copied literally. |
| 10 | Interactive controls keep a 44×44 minimum (`min-h-11`, `min-w-11` in the primitives) instead of Emerald's compact 24–36px scale. | PlatPulse's existing accessibility contract asserts every visible interactive control is at least 44×44 (e2e/helpers.ts `expectVisibleInteractiveTargets`), and the brief requires preserving touch operation. Colour, radius, typography, spacing and state treatment still follow upstream. |
| 11 | The Home statistics slot keeps Emerald's six cards in a 3×2 grid, but at ≥1024px it takes roughly 5:6 against the map (the map is the larger track) without the pinned 37:61 ratio or a fixed 232px band; below that the complete 2:1 map comes first (upstream's DOM order) and the six cards follow it. | The approved overview needs room for three columns of equal cards while the complete world map stays visible, and the original fixed band cannot hold either. The 5:6 split gives the map the larger half of the band as the reference lays it out — the map column then fills that width and grows with it — while three equal statistics columns still fit; when the six tiles are shorter than the map band they sit on its floor, so the gap down to the Network group keeps the same 16px rhythm as the Node cards; the tile keeps the earlier four-card grid's compact height: a 44px header row holds the title and the metric marker or Breakdown control, and the value sits beneath it over a shared one-line stat slot with no footer row, so the two cumulative tiles can print their scope/coverage line without breaking the six-tile height or value baseline. The page keeps its 1280px ceiling and the Node grid's auto-fill minimum track (**recalibrated in deviations 23, 26 and 27; the current value is in 27**). Slot position, card shell and typography are upstream's. |
| 12 | The Home Node card keeps the grey non-Healthy health marker (issue #141) and carries the exceptional signal on the card's red ring and diagnostic line. | The existing tested contract is that only Healthy is green and every other state is grey, so health is never colour-only; Emerald's red marker encodes online/offline, which is a different dimension from PlatPulse Node health. |
| 13 | The map prints the exact per-country Peer count in the scatter label, as upstream does, instead of PlatPulse's previous four-glyph abbreviation. | Upstream's label formatter is the raw aggregate. Bug-for-bug fidelity was chosen over the earlier abbreviation, which is a deliberate quality change to disclose: a count of five or more digits can overrun the 14px disc. |
| 14 | The map's per-country keyboard activation is replaced by a screen-reader country list; the chart container is a labelled `role="img"`. | ECharts paints into a canvas with no per-country element, so upstream's map has no keyboard path at all. Every observed country, its count, its stale count and whether it could be plotted remain available as text, and abnormal states stay announced. This is a different accessibility mechanism, not a claim of parity. |
| 15 | ECharts is loaded on demand (dynamic import, `components/echartsCore.ts`) rather than in the entry chunk, and that one loader registers both renderers (canvas for the map, SVG for the six charts). **Amended by deviation 28: the claim that Node Detail keeps the previous bundle size no longer holds.** | The library is ~530 KiB minified; keeping it out of the entry chunk means Login, Admin, Network and the map-free parts of the shell never pay for it. Measured on this change: entry chunk 1,189.79 → 1,192.80 kB (gzip 305.67 → 306.68 kB, +1.01 kB gzip), and 1,193.34 kB / gzip 306.81 kB once the review corrections of deviation 32 landed, while every ECharts chunk stayed byte-identical (`echarts/charts` and `echarts/renderers` are imported wholesale, so `LineChart`, `BarChart` and `SVGRenderer` were already inside the lazy chunks). What changed is that Node Detail now *loads* those chunks when it draws the deck. |

| 16 | The Node Detail page keeps PlatPulse's accepted "A container": an uncarded identity block, four summary tiles and three parallel observation panels, rather than upstream's single-instance card stack. | Upstream's InstanceDetail is built for one Komari instance; PlatPulse's page carries six 60-second charts, per-dimension collection/freshness/value state and public/Admin key partitioning. The visual language (tokens, card shell, type scale, spacing, table and chip treatment) is upstream's; the information architecture is the one the project already accepted in ADR-free design review and asserts in tests. |
| 17 | Every page is now on one stylesheet: `src/index.css` (5,386 lines) and `src/pages/AdminOverview.css` (291 lines) are deleted. | Built CSS fell from 170.31 kB to 70.17 kB (gzip 30.28 -> 12.37 kB) and no retired token reaches the bundle. This is the outcome the earlier entries were migrating towards; recorded here so the size change is accounted for. |
| 18 | **Historical; superseded by deviation 40.** `ui/back-top` and `ui/sonner` are not adopted. | PlatPulse has no back-to-top affordance, and its notices are persistent status regions rather than transient toasts; adding either would be a new feature rather than a migration. |
| 19 | Both map layers fill the whole track box (`left/top/width/height`), and the `map` series keeps its polygons silent (`tooltip.show: false`). From `xl` up the map track is upstream's fixed 22rem (352px) band instead of a proportional track; below `xl` it stays proportional (2:1), so phones and tablets keep the compact map the mobile acceptance measured. | Upstream sets width only and lets ECharts derive the height from the world's aspect ratio. With this geometry (`aspectScale` 0.75 over the 139.3° latitude span the `echarts-www` asset carries — no Antarctica) that ratio is ~1.94:1, so the 2:1 phone/tablet track is flatter than the world and the derived height overflowed it, slicing the poles; the accepted mobile evidence (`docs/visual-migration/mobile-map/after-390-dark.png`) shows that cut. Filling the track scales the world by the ~3% the two ratios differ, which is invisible, keeps every coastline whole, and keeps upstream's edge-to-edge map; `preserveAspect` stays at its default so the world is never letterboxed away from the track edges. The `xl` band needs no derivation either way. The band applies from `xl` because that is where Home reaches its ceiling and the column stops changing; below that a fixed band would overrun the narrower column. The **37:61 tracks and 232px summary band remain superseded by deviation 11**. Regression: `src/components/mapChartLayout.test.ts` (every vertex stays inside the box and the world fills both dimensions, and the two layers agree); `e2e/map-size.spec.ts` and `e2e/home-geo-map.spec.ts` check the fixed band and painted browser pixels at 1280/1440/2390px. |
| 20 | The Home refinement round compresses the Node identity block (the Node-name anchor keeps its measured 44px hit area through a negative block margin, and the header Role chip reads `Node: Non-validator` on one line), shares each Linked Validator metric pair's grid rows through `subgrid` (one CSS source, no JSX restatement), shortens only the `Validator status unknown` chip to a real 44px `Unknown` control with a focusable explanation, converges an authoritative empty Validator with no metric value into a compact line even when the only other status is `Unranked` (decided from raw fields and the identity/request state, never a formatted string), keeps the six overview values on one baseline by top-aligning each value under the uniform header, and drops the last dotted label/value leader on the Speed row. **The authoritative-empty Validator presentation is adjusted by deviation 23.** | Emerald has no Linked Validator, staking-identity or Unranked concept, and its Node card has more vertical room than the PlatPulse card needs. The refinements keep PlatPulse's own evidence semantics (a ranking status is not a metric value; Provider failure is never a no-live-Validator result) while adopting Emerald's compact spacing and label-over-value anatomy. Regression: unit tests for the fold predicates and the summary caption, `e2e/home-convergence.spec.ts`, `e2e/validator-summary.spec.ts`, `e2e/node-business-metrics.spec.ts` and `e2e/linked-validator.spec.ts` at the fixed viewports. The Linked Validator subgrid/six-stacked-cell anatomy is **superseded by deviation 21**. |
| 21 | The Home Node card keeps the emphasized label-over-value form only for the cumulative metrics (Cumulative blocks/rewards, and the chain heights when the card is narrow). Every ordinary parameter — chain heights on a wide card, Txs/Peers, CPU/Memory/Node data/Speed, and the four Linked Validator parameters — is a compact left-label/right-value key-value line with no per-row frame, no dotted leader and no fixed two-line label height. A named `node-card` container query on the card's own width switches each group between two columns and one full-width line per metric, and `wideValue` moves the Node data used/total caption onto the title line on a wide card. The two Linked Validator cumulative cells stay two columns at every width, a narrow card gives the four parameters four full-width lines (all default-visible), short display names keep their full field name as an accessible name and in Details, and the identity / Provider Data state / identifier / status dimensions are unchanged. | Emerald's Node card has more vertical room and no Linked Validator, so its exact anatomy cannot be transplanted. This round keeps the reference's compact, high-density key-value rhythm without hiding a field or reducing type. Measured on the seeded convergence fixtures (Chromium, no viewport scaling): the ordinary validator card falls from 665.5px to 546.0px (**−18.0%**) at a 362px card and from 619.2px to 496.0px (**−19.9%**) for the healthy Node-A card. At the 303px card the desktop-1280 four-column grid produces, the mandated four full-width chain rows and four full-width parameters offset the compaction, so the card stays within about ±3% (681.5 → 682.0px for Node H; 651.2 → 667.2px for Node A) rather than shrinking by cutting fields. The width switch is a CSS container query, never a viewport breakpoint or a JavaScript size listener. Regression: `e2e/node-business-metrics.spec.ts` (forced 300/360/560px container-query switching), `e2e/linked-validator.spec.ts`, `e2e/home-convergence.spec.ts`, `e2e/emerald-refinement.spec.ts`, plus the `MetricRow`, `HomeDashboard` and `LinkedValidator` unit suites. **The desktop grid geometry and the switch's minimum track are recalibrated by deviations 23 and 26; deviation 27 supersedes the track and the six-metric Home Validator anatomy (the complete metrics remain on Node Detail).** |

| 22 | The Home resource progress bars (CPU, Memory, Node data) pass the theme's `info` fill instead of `ProgressThin`'s default `bg-primary` (**remapped to the normal-state `success` token by deviation 23**); the Info/Copy/Details icon controls share one 14px icon size; the chain/parameter grid also falls back to full-width rows when a formatted number cannot fit a compact cell; and a compact Linked Validator parameter states `Unknown` instead of the em-dash placeholder. | `bg-primary` is white in the dark theme, so every ordinary resource bar read as an empty/white track. PlatPulse owns no occupancy percentage threshold rule, so none was invented — the bars use a theme colour rather than an invented success/warning/error condition, and a Server-owned rule can replace it later. The value-length fallback is still width-driven: a 21-digit safe integer cannot fit a compact cell at any card width, and the spec forbids shrinking, splitting or clipping it. `Unknown` is the explicit vocabulary the chain rows already use and never reads as zero; its sanitized reason stays announced. The reference capture's conspicuous Info frame was audited in the browser — the control is transparent by default (no background, border, outline or shadow), accent-tinted on hover, and paints the shared 3px focus-visible ring, which is kept because it is required keyboard feedback. Regression: `e2e/node-business-metrics.spec.ts`, `e2e/emerald-refinement.spec.ts`, and the `MetricRow`/`HomeDashboard`/`LinkedValidator` unit suites. |
| 23 | The Home Node-card density round recalibrates the auto-fill minimum track from 300px to the same 22.5rem (360px at the default root) that the card's own `node-card` container query uses, so the desktop-1280 content column renders three 408px-wide cards instead of four 303px-wide ones; on a wide card the consensus grid is two columns by three rows (Head/QC, Locked/Committed, Txs/Peers) and the four Linked Validator parameters are two columns by two rows, while a card below the track keeps one full-width row per metric. The authoritative-empty Linked Validator card keeps its `No live Validator` and `Unranked` conclusions on a stable Provider-data status row (the long explanation stays in Details), the heading and the status cluster each occupy their own row so the status can never strand right-aligned on a line of its own, and the ordinary resource progress bars pass the normal-state `success` token instead of `info`. | At the 300px minimum the auto-fill grid produced four 303px cards at desktop-1280/1440/1920, below the card's own 22.5rem two-column switch, so both parameter groups rendered as single-column tables — five consensus rows and five Linked Validator rows per card. Aligning the grid minimum with the switch makes the width decision self-consistent (auto-fill can never produce a track narrower than the two-column grids need); desktop-1280 resolves to three wide cards, and four columns return only when the content column can hold four 22.5rem tracks, which the 1280px-capped Home content does not reach. Measured on the seeded Public fixtures (Chromium, no viewport scaling, 408px card): the consensus group falls from 196px/5 rows to 136px/3 rows and the Linked Validator group from 176px/5 rows to 100px/3 rows; the no-validator card falls 519→443px and the linked-validator card 684→517px. The switch stays the card's own container query, never a viewport breakpoint or a JS size listener, and the value-length fallback (deviation 22) still keeps an over-long safe integer untruncated. Regression: `e2e/node-business-metrics.spec.ts` (forced 300/360/560px switching plus a natural-width row and per-cell overflow check at every fixed project), `e2e/home-overview-layout.spec.ts`, `e2e/linked-validator.spec.ts` (the folded empty card), `e2e/home-convergence.spec.ts`, and the `LinkedValidator`/`HomeDashboard` unit suites. **The minimum-track recalibration is superseded by deviation 26 and then 27; deviation 27 also replaces the earlier Home Validator parameter grid with the three-item summary.** |
| 24 | The Home Node-card top-right Validator Activity badge maps PlatScan's Activity to an icon plus the status name and a tone: teal for `verifying`, emerald for `producing`/`active`, an outline emerald (transparent background, lighter border, solid `emerald-700` light / `emerald-400` dark foreground) for `candidate`, and the neutral `muted-foreground` for `observing` and every out-of-scheme value. `active` takes a solid `emerald-700` foreground on light and `emerald-400` on dark, over Producing's unchanged background tint and a lighter border; no glow, solid fill or size change. | Emerald has no PlatScan Validator Activity; the badge is a PlatPulse addition recorded in ADR 0006. The foreground is chosen per theme because the `--success` token (≈emerald-500) fails 4.5:1 at the badge's 11px text on a light surface, while `emerald-400` is only legible on the dark surface, so the earlier single 70%-opacity Emerald was too dark in dark mode. Regression: `ValidatorActivityBadge.test.tsx` asserts the named-state mapping, the neutral Observing tone, one badge geometry and the 14px/2px/`currentColor` glyph. |

| 25 | The Home Peer-country corner chip keeps the in-scope Peer-record total and adds a second, static neutral dot with the Server's Unknown country count and the visible word `unknown` beside it, worded at every width; the Unknown count leaves the exceptional notice and appears there only when nothing was resolved (`knownCountryCount == 0 && unknownCountryCount > 0` → `No locations to show`). Upstream's chip shows two figures, a green online count and a yellow offline count, as the two parts of one node set. | PlatPulse's map plots Peer records, not monitored servers: a Peer whose country could not be resolved is still a reported connection, so it must not be presented as an abnormal or offline state, and the amber this map already uses for last-good country results cannot be reused for it. The Server's own denominator is `available = known + unknown`, so the total stays the primary figure and the Unknown bucket is a labelled subset rather than a second part; Emerald's two-part form would have dropped the denominator the map's fills are drawn from. Registered under [ADR 0002](../../adr/0002-webui-emerald-visual-authority.md) and specified by `docs/design/webui.md` "Home overview and Peer country map" items 3, 7 and 8 (issue #200). |

| 26 | **Historical; superseded by deviation 27.** The Home Node-card auto-fill minimum track returns from deviation 23's 22.5rem (360px) to Emerald's own 300px (`HomeView.vue`'s `minmax(300px, 1fr)`), so desktop-1280/1440/1920 resolves to four ~303px cards instead of three 408px ones. The card's own two-column switch stays at the measured 22.5rem, so a 303px auto-filled card keeps the narrow fallback — each chain count and each ordinary Validator parameter takes one full-width line, while Txs/Peers and the two cumulative Validator cells stay paired — instead of pairing its metrics. | Deviation 23 aligned the grid minimum with the card's two-column switch, which cost the reference's column size and left three 408px cards at desktop; Emerald's own track is 300px, and `docs/design/webui.md` and `docs/design/platpulse.md` both still specify `minmax(300px, 1fr)`, so the grid returns to the reference value and the design docs become accurate again. Lowering the switch to 300px is not an option: an 11-character chain height plus its label (for example `Committed 159,320,291`) cannot share the ~127px cell a 303px two-column card would give it, which is why 22.5rem is the measured switch point; keeping the narrow fallback is also what the reference's own single-column rows do. Regression: `e2e/home-overview-layout.spec.ts` (auto-fill column math from the grid's own measured width), `e2e/node-business-metrics.spec.ts` (natural-width row counts plus the forced 300/360/560px switching and per-cell overflow checks), `e2e/home-convergence.spec.ts`. |
| 27 | Issue #238 supersedes deviation 26's 300px auto-fill minimum: the Home Node grid returns to a clamped 22.5rem track (`min(100%, 22.5rem)`, 360px at the default root), so no auto-filled card is ever narrower than the compact Validator summary it holds and a narrower container never overflows; desktop-1280/1440 resolves to three ~408px cards again, a tablet to two and a phone to one. The Linked Validator block keeps only the three-item summary (Network rank, Cumulative blocks, Cumulative rewards) for a Validator status the Server currently confirms, with the full metric name as each short label's accessible name and title, an over-long compact value taking the whole row instead of splitting its digits; a currently confirmed non-Validator, a retained (non-current) negative or an unconfirmable identity replaces the summary with one short identity line that reserves no metric height, and the full six metrics, states, exact values and long exception explanations stay on Node detail. The grid no longer shares one height across rows (`auto-rows-fr` removed) while each row's cards still coordinate their bottoms |
| 28 | The Node Detail six-chart deck is ported from Emerald's `LoadChart.vue` onto ECharts 6 (`components/NodeMetricChart.tsx`, `components/nodeMetricChartOption.ts`, `components/metricChartAxis.ts`), mounted through the existing on-demand loader with the **SVG renderer**, and the page's hand-drawn 600×150 `<svg role="img">` chart is deleted together with its private helpers (`chartLinePath`, `chartAreaPath`, `chartBarCoordinates`, `chartBarWidth`, `niceChartMax`), its `data-slot="node-metric-chart-line"` / `data-slot="node-metric-chart-bar"` marks and its three aria-hidden axis spans. | The hand-drawn plot was the last hand-built visual in the deck, and the brief asks for the reference's real chart: a genuine axis, a crosshair tooltip stating the retained sample rather than the page's display rounding of it — no byte-rate unit conversion and no one-decimal pre-rounding, with three decimals as the display cap (deviation 32 (b); the byte-rate part is superseded by **deviation 39**, issue #249) — a vertical gradient area and resize behaviour. The SVG renderer is chosen over the map's canvas because a 1.5px hairline stays crisp at any DPR and because DOM geometry can then be asserted (deviation 29). Ported literally: the coral `#FF6B6B` CPU series with its `rgba(255, 107, 107, 0.25) → rgba(255, 107, 107, 0.02)` vertical gradient, the teal `#4ECDC4`, the network pair `#60A5FA` (download) / `#A78BFA` (upload) with no area fill, `lineStyle.width 1.5` with `cap: 'round'`, `showSymbol: false`, `smooth: false`, `animation: false`, the dashed low-contrast split lines, the light axis line with 11px secondary-coloured labels, the `left: 56` grid gutter (widened to 60, with the axis naming its own window, by **deviation 33**), the crosshair axis pointer with the Y label suppressed, the tooltip recipe (transparent border, 6px radius, 12/20 type, and `confine: true` where upstream lets the panel float — deviation 29 (i)) and Emerald's dark/light neutral pairs (text .85/.55/.35, border .1, splitLine .06, crosshair .15, tooltip 40,40,40 @.95 against 255,255,255 @.8). Colours stay literals in the option module keyed by a `dark` boolean, following `mapChartOption.ts`; the used series colours are not read from `--chart-*` (still no consumer). Regression: `src/components/nodeMetricChartOption.test.ts`, `src/components/metricChartAxis.test.ts`, `src/components/nodeMetricChart.test.ts` (real ECharts, SSR SVG output), `src/App.test.tsx`, `e2e/node-detail-six-charts.spec.ts`. |
| 29 | Deliberate divergences from `LoadChart.vue` inside the ported deck: (a) **no missing-value zeros** — upstream writes `r.ram ?? 0`, `r.net_in ?? 0`, `r.connections ?? 0` and so on (LoadChart.vue:513/534/588/662/670/733/741/793), while the deck cuts each direction into runs at the Server's own `gaps` and renders **one series per run** with `connectNulls: false`, so nothing is joined across a silence and no gap becomes a drop to 0; (b) an **isolated observation** stays visible as its own run drawn with a marker (`showSymbol` true only when the run holds one point), and an observation outside the window is **dropped** rather than clamped onto the edge; (c) the **empty state is a DOM sentence** (`No samples in the last minute`) with no chart mounted at all, where upstream hides the plot and shows `Empty` beside the canvas; (d) **whole-number axes cannot repeat a tick** (originally `minInterval: 1`, with a minimum ceiling of 4 for count series, which upstream's `Math.round(value)` label formatter does not guarantee; **restated as an explicit tick step by deviation 33 (c)**); (e) the CPU/memory **ceiling ladder continues past 100%** (5/10/25/50/100/150/200/250/400) so a Process CPU above 100% keeps its own axis, where upstream pins CPU at 100; (f) a real **`time` axis** with the tooltip printing each sample's own `HH:mm:ss`, because a pixel-space axis cannot state it honestly — the window edges were originally kept as a relative `60s … 0s` DOM caption under the plot, which **deviation 33 (b)** replaces with clock labels on the axis itself; (g) **no legend inside the canvas** — the deck keeps its DOM legend with `aria-label`, so both directions are announced rather than painted; (h) **no `dataZoom`**, no range tabs (upstream's 1/6/24/720h switcher) — **the range-tab part is superseded by deviation 38 / ADR 0012; the rest of (h) stands**, no `fillMissingTimePoints` bucket grid and no `interpolateNullsLinear` — the deck draws exactly the observations the Server retained and prints the Server's cadence/gap/tail verdict (originally as visible small print above the plot, moved into the chart's own accessible description by **deviation 34**); (i) the tooltip is **confined to its own plot** (`confine: true`) where upstream leaves it free, because a free panel is 162px wide, lands at a negative x on a 360px phone and scrolls the whole page sideways — which the suite's no-horizontal-overflow acceptance measures and rejects. | PlatPulse's contract is that a missing value is never `0` and that no run is joined across a Server-judged silence (`docs/design/webui.md` §11.1 items 8–9 and §15.26). The deck's window is the Server's own 60 seconds, so a front-end range selector would advertise a window no data supports, and each card states the Server's real coverage instead. (d) and (f) exist because this deck charts whole-number counts and sub-percent rates the reference never plots. Regression: the three new unit files plus `src/metricCoverage.test.ts` (15 cases: the semantics kept, the pixel-space geometry cases rewritten for the Server-unit seam it now returns) and `e2e/node-detail-six-charts.spec.ts` across the gap, single-cpu-failure, never-observed, expired, unknown and stale scenarios in both themes. |
| 30 | The deck's plot is the ECharts container (`data-slot="node-metric-chart"`) at **116px on phones, 148px from `md`, 176px from `lg`** (superseded by **deviation 33: 132 / 168 / 192px**), and `e2e/release-candidate.spec.ts` now caps the cards at **340px on desktop / 360px below** instead of 270/290. | The brief asks for a 176–192px desktop plot because the shipped 100px plot read as cramped; upstream's `h-48` (192px) box paints only a ~130px band inside its 30/32px margins, so 176px is the closest container height that paints ~160px. This supersedes the `plot AND SVG: 7.25rem below lg, 6.25rem from lg` wording in `docs/design/webui.md` §11.1 item 8 and its acceptance checklist, which were written for the hand-drawn SVG and are rewritten in the same change; the cards grow with the plot, which is why the height cap rises. Regression: `e2e/node-detail-six-charts.spec.ts` measures the container height at every fixed viewport, `e2e/release-candidate.spec.ts` the card cap. |
| 31 | Emerald's coral `#FF6B6B` is used as the **Process CPU data-series colour** (with its area gradient), Memory originally took the teal `#4ECDC4` where upstream also paints RAM coral (**deviation 33 (d)** restores the coral), and the two bar charts use `#FFB347` (Block interval) and `#A78BFA` (Transactions / block) from the same palette. | `docs/design/webui.md` §11.1 keeps red for destructive/error states, so the coral series is an explicit, registered exception: it is a data-series colour on a chart surface and never a status, the status vocabulary keeps `--destructive`, and no status semantics ride on any series colour. **Historical:** Memory diverged from upstream on purpose so the deck's first two charts were not two identical coral curves; the second refinement round judged the duplicate coral pair the smaller cost against upstream fidelity, so **deviation 33 (d)** restores it and takes the fainter area gradient instead. The two bar charts have no upstream counterpart, so they are coloured from Emerald's own list rather than a second palette. Regression: `src/components/nodeMetricChartOption.test.ts` asserts the literal per-series colours in both themes. **Amended by deviation 33 (d):** memory now takes upstream's coral as well, told apart from the CPU curve by its fainter 0.08 → 0.01 area gradient rather than by a hue of its own. |
| 32 | Post-review corrections to the ported deck (issue #225 follow-up): (a) the two **bar** charts set `containShape: false` on their time axis; (b) the tooltip prints the **retained sample itself** rather than a display reading of it (`formatPercentSample` / `formatRateSample` / `formatCountSample` / `formatMillisecondsSample` in `components/metricChartAxis.ts`, so 12.345% stays 12.345% and 1025 B/s is not restated as 1.00 KiB/s — **the byte-rate half of (b) is superseded by deviation 39 (issue #249)**: a rate tooltip now reads on the header's binary ladder, so 1025 B/s does print as 1.00 KiB/s); the promise is about **what is not done to the value** — no unit conversion, no one-decimal pre-rounding — while the *number of decimals* is capped at three, a display cap that drops float noise (a raw Agent CPU reading such as 12.34567% prints as 12.346%) and never climbs back into the rounded reading the card's own value prints; (c) an isolated run’s marker is sized **per direction** — 8px (r 4) for a card’s first direction and 7px (r 3.5) for the next ones — so coincident inbound/outbound points stay visible; (d) a failed lazy load is **visible** (`data-chart-error` on the container plus a `node-metric-chart-error` line) and `components/echartsCore.ts` clears its memoised promise on failure so a remount can retry; (e) the relative `60s … 0s` labels are inset by the grid’s own gutter (`pl-14 pr-1.5`; the whole caption is deleted under **deviation 33 (b)**, which names the window on the axis itself); (f) the DOM legend wrapper is a `role="group"` rather than a bare `aria-label` on a `<div>`; (g) the tooltip is **confined to its plot** (see deviation 29 (i)) and the browser test reaches it with a real pointer gesture — the plot is centred first (`scrollIntoView({ block: 'center' })`), because on the phones the sticky shell header otherwise sits over the point the pointer is sent to, and a check that the point really belongs to the plot guards the assertion. (h) the on-demand loader is one memoized attempt that forgets a rejection (`memoizeLoader` in `components/echartsCore.ts`), so a transient chunk failure cannot blank six charts for the rest of the session, and three cases in `components/echartsCore.test.ts` pin the shared in-flight attempt, the cached success and the retry after a rejection. | ECharts 6 widens a time axis by half a bar’s bandwidth whenever a bar series shares it (`containShape`, on by default in `coord/scaleRawExtentInfo.js`; the handler is `createBandWidthBasedAxisContainShapeHandler` in `chart/helper/axisSnippets.js`, registered from `layout/barGrid.js`), which pulled both block charts’ window inward — a single observation mapped to x 104.44…225.56 inside a 56…274 plot box — and made them disagree with the four line charts about the Server’s own window. Turning it off costs half a bar at each edge when an observation sits exactly on `from`/`to`, which is honest and accepted. These six came from an adversarial read-only review, and (a)’s regression is now independent of the engine: `src/components/nodeMetricChart.test.ts` computes expected positions arithmetically from the option’s own grid instead of reading them back through `convertToPixel`, and was verified to fail (24.27px and 10.9px off) with `containShape` temporarily removed. |
| 33 | **Amended by deviation 36 (a)–(b):** the axis unit is now stated once in the axis title instead of on every tick, and each window derives one band-countable ceiling; `rateChartMax`, `niceChartMax`, `formatRateAxisAt` and `integerChartTicks` are **deleted** and `RATE_CHART_MULTIPLES`' binary ladder is replaced by the decimal ladder of the unit the window's own peak chose, so the `[5%, 26%]` headroom bound recorded in (h) below is **historical**: the measured worst case is 31.2% at four and five bands and 40.0% at three, and the decimal ladder's widest rung gap is 1.5×. Second refinement round on the ported six-chart deck (issue #225 follow-up), bringing the cards and the plot geometry back onto `LoadChart.vue`: (a) every card is `CardX size="small"` using the component's own header slot, so the title and the plot share one 12px inset — the title is the card's largest type (**16px, bold**), the current reading sits at the header's right at **12px** and never below the plot, the `min-h-8 flex-1` spacer that put dead space under the title is deleted, and the legend slot is reserved at a fixed height on all six cards so the six plots keep one baseline; (b) the plot is **132px on phones / 168px from `md` / 192px from `lg`** (upstream's `h-48`); (c) the axis now names the window itself — `HH:mm:ss` labels on the real time axis (`fontSize: 11`, `hideOverlap`, the `clockTime` formatter) **replace the relative `60s … 0s` DOM caption**, which is deleted, while the 60-second window stays the axis `min`/`max`; (d) grid `top: 12, right: 24, bottom: 26, left: 60` — the gutter widens from 56 so a unified rate label such as `1.25 MiB/s` fits inside it, and `right: 24` is upstream's own number, which also fixes the six cards' right edge on one column; (e) a whole-number axis takes an **explicit tick step** (`integerChartTicks`: max 20 → 4 ticks of 5, max 7 → 3 ticks of 2) instead of `minInterval: 1`; (f) **Memory returns to upstream's coral** `#FF6B6B` — told apart from the CPU curve by a fainter area gradient (`0.08 → 0.01` against the CPU's literal `0.25 → 0.02`, `areaOpacity` per series) rather than by a hue of its own; (g) the two bar charts give up weight rather than colour (`barMaxWidth: 12`, `borderRadius: [3, 3, 0, 0]`, `opacity: 0.75`); (h) the network ceiling is derived from the window's **real peak** (`rateChartMax`: the first rung of a binary ladder that clears the peak by at least 5%, which holds the headroom inside **[5%, 26%]** — a 5 MiB/s peak lands on 6 MiB/s rather than the 9.5 MiB/s the decimal `niceChartMax` ladder produced) and every label is written **in that ceiling's single unit** (`formatRateAxisAt`), so `MiB/s` never sits beside `KiB/s` on one axis. | The first port matched the reference's ingredients but not its proportions, which is what the round's brief measured: the reading was set at 20px, so the value shouted louder than the chart's own name; the flexible spacer plus a legend slot only two cards rendered left the six plots starting at different heights; the plot kept the port's own 116/148/176 heights while upstream's box is 192px; the `60s … 0s` caption spent a row on information the axis carries better; the decimal ceiling ladder put a 5 MiB/s peak on a 9.5 MiB/s axis and a per-value unit formatter then mixed units on that axis; and a count axis could still repeat a tick. The rate ladder's own gaps were tightened in review, twice: the coarse 6 → 8 and 12 → 16 MiB/s jumps had left a 33% headroom where the brief asked for 10–20%, and the first replacement — the step nearest a 12% target — still reached 31% wherever a peak sat just above a 1.25× rung, so the ceiling now takes the first rung clearing the peak by 5% and the ladder carries half-step rungs (1.125, 2.25, 4.5, 9, 18) that cap its widest gap at 1.2×; that is what bounds the headroom at 26%, measured over 4 000 swept peaks rather than reasoned about. Both directions of the network card are 12px readings rather than 10px captions. Every number here is upstream's where upstream has one (the title/plot/reading scale, `right: 24`, the `h-48` desktop plot) and a measured consequence of the real data where it has none (the gutter, the tick step, the rate ceiling). Nothing about the evidence changed: still no zero for a missing sample, no line across a silence, no re-sampling, no clamping of a >100% CPU reading, and the coverage/stale/gap/unobserved-tail verdicts are untouched. Regression: `src/components/nodeMetricChartOption.test.ts` (the grid rectangle, the clock-axis formatter, the tick-step axes, the bar recipe, both area gradients), `src/components/metricChartAxis.test.ts` (`rateChartMax`, `formatRateAxisAt`, `integerChartTicks`), the SSR `src/components/nodeMetricChart.test.ts` (the new gutter and margins inside the window geometry and the split-line extents), `src/App.test.tsx` and `e2e/node-detail-six-charts.spec.ts` (the 16px/12px header recipe, the six plots' shared columns, the paint recipe — coral CPU 0.25/0.02, coral memory 0.08/0.01, `#FFB347` bars at 0.75, one unit per rate axis — and the absent captions), `e2e/release-candidate.spec.ts` (the card height cap and the six equal heights). |
| 34 | The six Node Detail cards no longer print the coverage verdict as visible small print (issue #225 follow-up, requested after the second round): the `data-slot="node-metric-coverage"` paragraph (`m-0 text-[11px] leading-4 text-muted-foreground`) that sat in the card's detail area beside the other absence notes is deleted, and the same sentence — `No samples reported yet`, `No retained observations`, `Last observation N ago`, `N gaps in this window`, `No samples for the last N`, joined per card by `coverageNotes` — is now written only into the chart's `sr-only` description, which `[data-slot="node-metric-chart"]` already pointed at through `aria-describedby`. Untouched: the `detail` line (`161,350,108 → 161,350,107`, `last-good value retained · collection Error`), the italic `… unavailable in this window` direction note, the `No samples in the last minute` empty state, and the sentences themselves. | Upstream's `LoadChart.vue` card has no absence vocabulary to print, so this sentence was the one element that made a PlatPulse card taller and busier than the card it is ported from: a card with something missing opened with a line of 11px grey text above its plot, and the deck's footer heights varied with what each series happened to have to say. The evidence semantics do not change — the verdict is still derived by `coverageNote`/`coverageNotes` (platpulse-web/src/metricCoverage.ts) from the Server's own per-series coverage, still never a zero and never a line across a silence, and still asserted — but it is now only reachable by a screen reader, which hears exactly the sentence the sighted reader used to see. `src/App.test.tsx` asserts the slot is absent and reads the sentence from the `sr-only` paragraph, and `e2e/node-detail-six-charts.spec.ts` asserts the deck's `[data-slot="node-metric-coverage"]` count is zero once per scenario — one assertion below the scenario chain, so all seven scenarios prove it — while the never-observed and expired scenarios read all six sentences back through each plot's `aria-describedby` id and check them card by card against the verdict that card's series has earned (`expectAbsenceVerdicts`: exactly one verdict per card, so a card cannot drop its sentence or carry two). The register's own §15.26 record of the visible surface is rewritten in docs/design/webui.md accordingly. |
| 35 | **Amended by deviation 36 (a)–(d):** the `CPU %` / `Memory %` / `Speed` / `ms` axis names, the `[0, 40, 0, 0]` name padding and the per-tick units recorded here are historical — each unit is now stated once in the axis title with bare ticks and the title sits at x=4 inside the plot. The helper names recorded here and in deviation 33 (`integerChartTicks`, `rateChartMax`, `niceChartMax`, `formatRateAxisAt`, `axisInterval`) no longer exist: the live helpers are `niceAxisScale`, `percentAxis`, `countAxis`, `durationAxis`, `rateAxis` and `metricChartMaxBands`. Third refinement round on the ported six-chart deck: complete visual consistency restoration to Emerald's `LoadChart.vue`. (a) Grid margins unified to Emerald original: ordinary chart `top: 30, right: 24, bottom: 32, left: 56`, chart with legend `top: 30, right: 24, bottom: 52, left: 56`, with `outerBoundsMode: 'none'` so ECharts 6 does not shrink plot rect for axis names. (b) Y-axis names and units restored on all six charts (`CPU %`, `Memory %`, `Speed`, `Connections`, `ms`, `Txs`) with `nameTextStyle` padding `[0, 40, 0, 0]` and 11px font. (c) Grid splitLine restored to Emerald's dashed `splitLineColor` (`rgba(0, 0, 0, 0.06)` / `rgba(255, 255, 255, 0.06)`). (d) X-axis configured with `splitNumber: 3` and `margin: 12`, yielding ~4–5 evenly distributed time labels without crowding. (e) Multi-series charts (Host network, Peer connections) use native ECharts `LegendComponent` with Emerald styling (`bottom: 4`, `itemWidth: 12`, `itemHeight: 12`, `itemGap: 20`, `icon: 'roundRect'`, 11px `textSecondary`, centered) replacing the separate DOM legend slot. (f) Process CPU and Process memory both use Emerald's `#FF6B6B` with identical `0.25 → 0.02` vertical area gradient (superseding deviation 33 (f)'s fainter memory opacity). (g) Peer connections colors aligned to Inbound `#FF6B6B` and Outbound `#4ECDC4` (matching Emerald Connections TCP/UDP). (h) Tooltip restored to Emerald visual styling: 6px border radius, subtle multi-tier box shadow, crosshair axisPointer with `textTertiary` label and dashed line. | Strict visual alignment with Emerald reference while preserving PlatPulse data semantics: no fabricated metrics, disconnected line segments across silences, accessible coverage text retained in `sr-only` description, dynamic rate/height axes, and >100% Process CPU support. Regression: `nodeMetricChartOption.test.ts`, `nodeMetricChart.test.ts`, `App.test.tsx`, and Playwright `e2e/node-detail-six-charts.spec.ts` across all 5 projects. |
| 36 | Rendered-defect pass on the six-chart deck (issue #225 follow-up, fourth round): (a) each chart resolves **one value axis** through `MetricAxisRule = (peak, maxBands) => MetricAxis` (`components/metricChartAxis.ts`), whose unit is stated **once in the axis title** (`CPU (%)`, `Memory (%)`, `Speed (GiB/s)`, `Connections`, `Duration (s)`, `Txs`) with **bare-number ticks** (`bareNumber`); (b) `niceAxisScale` takes the lowest whole-step ceiling, `percentAxis` treats the `percentChartMax` rung as a target rather than the ceiling (620% → 750% at 150%, 1,100% → 1,200% at 300%) and `metricChartMaxBands(height, showLegend)` caps the bands by the rendered height; (c) `durationAxis` states `ms` below 5,000 ms and `(s)` above it, so a tick list never mixes `2s` with `ms`; (d) the **network header is one row** of direction-coloured readings (`MetricReading` + a chevron, with `Upload` / `Download` demoted to `sr-only` labels) instead of two stacked blocks; (e) the axis title is anchored inside the plot at x=4 (`nameLocation: 'end'`, padding `[0, 0, 0, METRIC_CHART_AXIS_NAME_INSET - METRIC_CHART_LEFT_MARGIN]`); (f) the settled Node Detail contract records the same rules — `docs/design/webui.md` PAGE-HOME-NODE item 8 now states the header-unit/one-title-unit hierarchy, the rung-as-target ladder and the height-derived tick budget, and the `SCN-HOME-NODE-DETAIL` row names them together; the Grid constants of deviation 35 (a) are **unchanged**. | The rendered deck still clipped the rate gutter, printed `%` twice on two cards and mixed `2s` with `ms` on a third: upstream repeats the unit on every tick, and that repetition is exactly what overflowed the 56px gutter and duplicated the header's `%`. Stating the unit once, in a title placed above the tick column, keeps every tick box inside the gutter without hiding a label, truncating text or shrinking the font, and one resolved axis makes the unit impossible to drift between title and ticks. Data accuracy is untouched: the tooltip still prints the retained sample with its own unit, no missing sample is zero-filled, no curve is bridged across a gap and nothing is smoothed. Regression: `src/components/metricChartAxis.test.ts`, `src/components/nodeMetricChartOption.test.ts`, `src/components/nodeMetricChart.test.ts`, `src/App.test.tsx`, `e2e/node-detail-six-charts.spec.ts` and the new `e2e/node-detail-chart-visual-acceptance.spec.ts`. |
| 37 | Node Detail closing refinement — **behaviour only, no drawing change**: the deck's colours, line widths, gradients, grid, axes, bottom legend, top-right readings, card sizes and 3×2 layout are exactly deviation 36's (geometry dumps of `node-detail-chart-visual-acceptance` before and after differ only in clock tick labels). (a) Every option is still applied with `notMerge`, so expired runs leave and silences stay broken, and the reader's `legend.selected` is restated on each apply (`withLegendSelection`) so a hidden Upload / Download / Inbound / Outbound stays hidden across refreshes, resizes and Light/Dark; each chart keeps its own choice and the deck is keyed by Node. (b) A failed metrics refresh keeps the last drawn answer with its own window and shows one `History update failed · Showing previous window` notice beside a `Retained 60-second window` heading; nothing is shown in the normal state. (c) Standing explanation lines on the Sync collection tile, Process and Host resources moved into on-demand info hints (`components/InfoHint.tsx`); the Validator diagnostics disclosure reads as three key/value groups and the performance card uses a 17px / 13px two-tier hierarchy with two-decimal rates. Specified in `docs/design/webui.md` §11.1 items 3, 5–8 and §15.26. Regression: `components/nodeMetricChartLegend.test.ts` (real ECharts: hidden direction survives refreshes, resize and theme; expired runs removed; gaps not bridged), `components/NodeMetricChart.test.tsx` (listener bound once, removed on unmount, fresh mount starts clean), `e2e/node-detail-refinement.spec.ts` (legend, retained window, first failure, info hints by touch and keyboard), and the unchanged `e2e/node-detail-chart-visual-acceptance.spec.ts`. |
| 38 | **Delivered by issues #247 and #248 (ADR 0012, docs/design/webui.md §15.38).** The deck regains Emerald's range switcher as a Metric Window selector — `Live · 1h · 6h · 24h · 7d`, default Live — superseding the range-tab part of deviation 29 (h). Still divergent from `LoadChart.vue`: (a) the presets are a fixed Server enum, not upstream's 1/6/24/720h, and there is no 30-day preset; (b) no `fillMissingTimePoints` grid and no `interpolateNullsLinear` — an aggregate bucket is drawn by its stored maximum, an absent bucket stays absent, and no run is joined across a Server-judged silence; (c) no `dataZoom`; (d) one selector for all six cards, held in the URL (`?window=`), rather than per-card state; (e) **resolved by #248:** Block interval and Transactions / block plot every window from the block aggregate tiers, each bucket drawn by its maximum (the longest interval; the most transactions one block carried, with the bucket's transaction total in the tooltip) — the interim `Block charts show the Live window only` sentence is gone. | Deviation 29 (h) refused the switcher because the Public deck only had 60 seconds of evidence. Since #214 every counted observation is accumulated into the 1-minute and 5-minute tiers at ingestion, so longer windows are now answered by real Server evidence; the switcher no longer advertises a window no data supports. Regression: `src/components/MetricWindowSelector.test.tsx`, `src/components/nodeMetricChartOption.test.ts`, `src/App.test.tsx` and `e2e/node-detail-metric-window.spec.ts` (block bars and the 24h/7d block gap since #248); the Live matrix of `e2e/node-detail-six-charts.spec.ts` is unchanged. |
| 39 | **Issue #249:** the Host network tooltip reads each byte-rate sample on the **same binary ladder and decimals as the card header** (`B/s → KiB/s → MiB/s → GiB/s → TiB/s`, two decimals under 10, one under 100, none above, the printed number always below 1024 of its unit — a value that only rounds up to 1024 moves to the next unit, so 1023.7 B/s reads `1.00 KiB/s` rather than `1024 B/s`): `formatRateSample` now delegates to `formatMetricRate`, and both walk the one `RATE_UNITS` table the rate axis title already used — which also lets the header reach `TiB/s` (`1.00 TiB/s` where it used to print `1024 GiB/s`, on Node Detail and on the Home card's Network row) — so a hovered `82366 B/s` / `151618 B/s` prints `80.4 KiB/s` / `148 KiB/s` beside a `↑ 1.52 MiB/s` header and a `Speed (MiB/s)` axis, while a sample under 1 KiB stays in bytes (`800 B/s`, never `0.78 KiB/s`). Supersedes the byte-rate half of deviation 32 (b) and the matching clause of deviation 28; the percentage, count and duration tooltips keep 32 (b)'s retained-sample rule unchanged, and the tooltip still goes through the generic `valueFormat` hook (no Host-network branch). | Option A of the issue: the retained-sample rule printed a bare, unscaled byte count, so one card stated its rate in two units and the reader had to divide by 1024 to match the tooltip against the curve and the header, while long numbers widened the `confine: true` panel inside the plot. The distinction 32 (b) protected — 1024 B/s and 1025 B/s printing differently — was already given up by the header (`formatMetricRate` reads both as `1.00 KiB/s`), so the tooltip now accepts the same rounding step rather than keeping extra digits (option B) or a second raw-byte line (option C) that would disagree with the header. Regression: `src/components/metricChartAxis.test.ts` (the issue's samples, the 1 KiB boundary, the round-up-to-1024 edge, and tooltip = header for every sample), `src/components/nodeMetricChartOption.test.ts` (the two-direction tooltip body), `e2e/node-detail-six-charts.spec.ts` (a real hover on Host network at all five viewports: every reading is a number below 1024 in a ladder unit no larger than the one the `Speed (…)` axis title chose). |

| 40 | **Issue #263:** `ui/back-top` and `ui/sonner` are adopted as `components/ui/back-top.tsx` and `components/ui/sonner.tsx`, and `DataTooltip` adds start/end placements (`bottom-end` for header action controls) with `touch={false}` passthrough. | BackTop is adopted with PlatPulse's 44×44px interactive minimum (deviation 10) and smooth-scroll with `prefers-reduced-motion` support. Sonner provides global transient toasts (mapped to Emerald's `--popover` and theme tokens in `components/ui/sonner.tsx`) while persistent status notices remain intact. DataTooltip supports `bottom-end` placement (`top-full right-0 mt-2`) to prevent horizontal viewport clipping on sticky headers, and `touch={false}` ensures actionable controls trigger immediately without swallowing taps on touch devices. |
| 41 | **Issue #264:** Overview summary metric card auxiliary icon muted/highlight tier, secondary unit typographic hierarchy, and dark-mode rate label contrast colours (`text-emerald-600 dark:text-emerald-400` / `text-sky-600 dark:text-sky-400`). | Ported from Emerald `NodeGeneralCards.vue`: summary cards (`SummaryMetricCard`) adopt the subtle resting auxiliary icon (`text-muted-foreground/30`) with smooth hover highlight (`group-hover:text-foreground transition-colors motion-reduce:transition-none`) while preserving the non-clickable card surface (no lift or glow, respecting deviation 11/12). A dedicated `unit` tier renders secondary unit labels (such as `LAT` on the Balance tile) in smaller muted typography (`text-xs md:text-sm font-normal text-muted-foreground`) alongside bold tabular-nums values. Network rate directions use Emerald/Sky palette pairs with dark-mode tints to improve contrast across themes. |

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

`screenshots/<project>/<page>.png` — 25 captures (login, Home, Node detail, Admin overview, Settings) across the five projects. Captured with `EMERALD_EVIDENCE=1 npx playwright test e2e/visual-evidence.spec.ts`. There is no `toHaveScreenshot` assertion anywhere in the repository, so these are evidence and never a gate. **They have not yet been reviewed against upstream's rendering**, so no fidelity claim is made.


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

> **Superseded layout:** the 37:61 tracks and 232px band recorded in this section
> were replaced by the six-card overview in **deviation 11** (4:3 statistics/map
> at ≥1024px, a contained 2:1 map, statistics above the map below that). This
> section remains as the record of that earlier, human-approved composition.

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

The explicit evidence run passed all 20 tests and refreshed five pages per
viewport (25 PNGs): Login, Home, Node detail, Admin overview and
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
presentation/test-only slice. Changes remain local; nothing was pushed and no PR
was created. (Committed to `main` as `a2d28ea` after this entry was written.)

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

## Admin workspace follow-up (ADR 0003)

The centered 1280px Admin shell/page rule from the initial Emerald migration is
superseded by [ADR 0003](../../adr/0003-admin-adaptive-workspace.md). Admin now
uses the available width beside its existing sidebar; the header shares that
partition. Emerald visuals and public layout remain unchanged. Local form
width limits remain valid. The shell geometry test now protects workspace fill
and shared alignment rather than the former cap.

Matched screenshots and source audit: [Admin workspace evidence](admin-workspace/README.md).
