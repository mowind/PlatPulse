Part of the [Emerald visual migration record](README.md): sections 2–3. Sections keep their numbers; image and evidence links are relative to this directory.

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
| `ui/card-x/CardX.vue` | `components/ui/card-x.tsx` (`CardX`) | verbatim (includes `headerExtra` from `header-extra` slot; see deviation 42) |
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
| `components/NodeGeneralCards.vue` | `components/HomeDashboard.tsx` top area (`OverviewBand`) | ported (12-col: stats left, map right; icon hover tier and unit tier aligned via deviation 41) |
| `components/NodeGeneralCards.vue` finance tiles | not adopted (no PlatPulse field) | deviation |
| `components/NodeCard.vue` | `components/HomeNodeCard.tsx` (`HomeNodeCard`) | ported |
| `components/NodeEarthMaps.vue` + `utils/echartsWorldMap.ts` | `components/GeoWorldMap.tsx` + `components/mapChartOption.ts` on `echarts` | ported (option literal; geometry pinned locally) |
| `components/NodeEarthGlobe.vue` (`cobe`) | not adopted — `maps` mode only | deviation |
| `views/HomeView.vue` | `components/HomeDashboard.tsx`, `components/HomeToolbar.tsx`, `components/HomeNodeList.tsx` | ported (search debounced via deviation 42) |
| `views/InstanceDetail.vue` | `pages/HomePages.tsx` Node Page | adapted (see deviation 16) |
| `views/InstanceDetail.vue` LoadChart/PingChart row | `pages/HomePages.tsx` six-chart deck on `echarts` | ported for the load deck (see deviations 28–31); `PingChart` has no PlatPulse counterpart |
| `src/components/LoadChart.vue` (the six load charts) | `components/NodeMetricChart.tsx` + `components/nodeMetricChartOption.ts` + `components/metricChartAxis.ts` on `echarts` | ported (option literal; colours, gradient, 1.5px stroke and tooltip kept — deviations 28–31) |
| `src/components/LoadChart.vue` realtime tabs (1/6/24/… h) | not adopted — the deck keeps the Server's real latest 60 seconds | deviation (29) |
| `src/utils/echarts.ts` (`use([...])` registration) | `components/echartsCore.ts` (`loadECharts`) | ported (one on-demand entry serving the map and the six charts, canvas + SVG renderers) |
| `src/utils/recordHelper.ts` (`fillMissingTimePoints`, `interpolateNullsLinear`, `cutPeakValues`) | not adopted | deviation (29) |
| `—` | Admin pages (no upstream counterpart) | Emerald tokens, primitives, spacing and table rules only |
