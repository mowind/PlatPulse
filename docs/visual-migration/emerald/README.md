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
| `ui/back-top/BackTop.vue` | `components/ui/back-top.tsx` | planned |
| `ui/data-tooltip/DataTooltip.vue` | `components/ui/data-tooltip.tsx` | planned |
| `ui/dialog/*` (reka-ui) | `components/ui/dialog.tsx` | planned |
| `ui/tabs/*` (reka-ui) | `components/ui/tabs.tsx` | planned |
| `ui/sonner/Sonner.vue` | `components/ui/toaster.tsx` (`sonner`) | planned |
| `ui/avatar/*` | not adopted (no PlatPulse use) | n/a |
| `components/Background.vue` | `components/BackgroundDecoration.tsx` | ported verbatim (fixed 1300px wash) |
| `components/Header.vue` | `layouts/HomeLayout.tsx` header | ported (h-14 sticky, blur on scroll) |
| `components/Footer.vue` | `components/AppFooter.tsx` | metrics ported, copy replaced |
| `components/NodeGeneralCards.vue` | `components/HomeDashboard.tsx` top area | ported (12-col: stats left, map right) |
| `components/NodeGeneralCards.vue` finance tiles | not adopted (no PlatPulse field) | deviation |
| `components/NodeCard.vue` | `components/HomeDashboard.tsx` `HomeNodeCard` | ported |
| `components/NodeEarthMaps.vue` + `utils/echartsWorldMap.ts` | `components/GeoWorldMap.tsx` on `echarts` | planned |
| `components/NodeEarthGlobe.vue` (`cobe`) | not adopted — `maps` mode only | deviation |
| `views/HomeView.vue` | `components/HomeDashboard.tsx` | planned |
| `views/InstanceDetail.vue` | `pages/HomePages.tsx` Node Page | planned |
| — | Admin pages (no upstream counterpart) | Emerald tokens/typography only |

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

## 5. Screenshot evidence

`screenshots/` holds the before/after captures for the acceptance viewports
(360×800, 390×844, 768×1024, 1280×800, 1440×900). They are evidence only: no
`toHaveScreenshot` baseline is recorded or re-recorded, so no regression can be
hidden by refreshing a baseline.
