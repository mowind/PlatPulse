# ADR 0002: Adopt komari-theme-emerald as the WebUI visual authority

- **Status:** Accepted
- **Date:** 2026-09-14
- **Decision scope:** `platpulse-web` presentation only

## Context

The WebUI's visual language was hand-written: a single 5,386-line
`src/index.css` holding three parallel token systems (shared `:root`,
`--admin-*`, and per-page locals), 81 selectors defined more than once, and
sections of Emerald-derived values appended after the originals they were meant
to replace. There was no shared primitive layer — no Button, Input, Card, Badge,
Table, Dialog or Select — so every page carried its own bespoke rules. Choosing
"a similar emerald palette" again would have added a fourth layer to that sheet.

The reference implementation, komari-theme-emerald, is a Vue 3 + Tailwind v4 +
shadcn-vue theme. Three of its properties make a naive port impossible:

1. **Its visual fidelity lives in Tailwind class strings**, not in standalone
   CSS. Porting the tokens alone would reproduce the palette and nothing else.
2. **Its map geometry is fetched at runtime from four unpinned CDNs**
   (`cdn.jsdelivr.net`, `fastly.jsdelivr.net`, `gcore.jsdelivr.net`,
   `raw.githubusercontent.com`, all `@master`), and its country-flag and OS-logo
   assets are not in its repository at all — the Komari host serves them.
3. **It has no shadcn Select, Table, Tooltip, Skeleton, Dropdown or toast
   primitive**; its only dropdown is a native `<select>`.

PlatPulse's own constraints bound the other side. The Server enforces
`default-src 'self'; script-src 'self'; connect-src 'self'` on every response,
so no runtime third-party fetch can succeed. The product is a Server-Agent-WebUI
monitoring suite whose public map plots **Peer records by country**, not server
node locations, and whose unknown/stale/never-observed states must never render
as `0`, `false` or Healthy. The Rust workspace, API contract, auth protocol and
deployment architecture are out of scope by definition.

## Decision

### Visual authority is a pinned revision, not a moving target

komari-theme-emerald at commit `c2c5e88ea19c7cbe18d14a50414e10deca3cc66e`
(tag `v1.0.12`, MIT, © 2026 Tokinx) is the visual authority for
`platpulse-web`. Every adopted value is traceable to that revision's
`src/styles/main.css`, `src/components/ui/**` or component templates. A later
upstream revision is a new decision, not an automatic update.

### Styling mechanism: Tailwind v4 with upstream's tokens verbatim

`platpulse-web` adopts Tailwind v4 (`@tailwindcss/vite`, CSS-first, no JS
config) and re-creates Emerald's `main.css` layer structure: the `:root` and
`.dark` token blocks, the `@theme inline` mapping, the
`@custom-variant dark (&:where(.dark, .dark *))` variant, the base layer and the
scrollbar rule are copied literally. Emerald's shadcn-vue primitives are ported
to React one for one, keeping each `cva()` variant and size class string
verbatim so the rendered result is upstream's, not an approximation.

The retired sheet is imported into `layer(legacy)` — the lowest layer — while
pages migrate, so it cannot outrank the new system, and it is deleted page group
by page group. No parallel design system is permitted to survive the migration.

### Map geometry and flag assets are vendored from pinned sources

Runtime third-party requests are never adopted, even where upstream relies on
them. The world geometry is vendored from
`apache/echarts-www@4e9b6889abf0995b1784610a907cdb8821f59996` (Apache-2.0),
checksum-pinned, and served from PlatPulse's own origin; it replaces the
Natural Earth 1:110m build (217 polygons and 26,273 points against 175 and
7,366, so map fidelity rises rather than falls). Country flags are vendored
from `flag-icons` 7.5.0 (MIT) under `/assets/flags/{iso2}.svg`. Upstream's FX
rate providers, visitor-geolocation providers and the Iconify runtime API are
never adopted, and the map renders in upstream's `maps` mode, not its `cobe`
globe mode.

### Business semantics outrank visual parity

The map keeps plotting Peer-country distribution; Emerald's server-node
semantics are not transplanted onto it. The Home top row keeps PlatPulse's own
Node, health, attention and Network counters rather than adopting Emerald's six
resource tiles, because upstream's "remaining value" tile has no PlatPulse
analogue and cross-Node resource aggregation would double-count Hosts shared by
several Nodes. Where Emerald has no equivalent, PlatPulse derives the state from
Emerald's own tokens (`--success`, `--warning`, `--destructive`,
`--muted-foreground`) instead of introducing a second palette; each such
addition is registered as a deviation.

### Theme persistence, routing and access boundaries are untouched

`platpulse.themeMode` keeps its key, its `auto` default and its pre-paint
`public/theme-init.js`; an explicit user choice is never overwritten. The
theme is applied as the `.dark` class, which is already what Emerald's
`@custom-variant` expects. Routes, query parameters, filters, sorting, detail
links, auth flows, role gates and the Public/Admin data split are unchanged.

## Consequences

- `src/index.css` and `src/pages/AdminOverview.css` are deleted; the WebUI
  ships one design system instead of three token vocabularies.
- New build dependencies: `tailwindcss`, `@tailwindcss/vite`,
  `tw-animate-css`, `class-variance-authority`, `tailwind-merge`, `clsx`,
  `lucide-react`, `sonner`, `echarts`. `@fontsource-variable/inter` is
  removed: Emerald is system-font-only, and the previous split (Inter globally,
  system-ui on Home) is resolved toward upstream.
- ~2.7 MiB of vendored static assets are added (987 KiB geometry, 1.9 MiB
  flags) and served same-origin under the existing CSP.
- Emerald defines only online/offline. PlatPulse's wider observation states are
  mapped inside Emerald's palette and registered as deviations, so the WebUI can
  never claim pixel parity it does not have.
- Visual verification is screenshot evidence at the fixed viewport matrix,
  including the added `desktop-1440` project. Screenshots are evidence, never
  gates: no baseline is recorded or re-recorded to make a regression disappear.
- Upstream's brand, footer links, ICP reference and external service calls are
  not adopted.
