# Emerald visual migration — provenance, mapping and deviations

This directory is the evidence and provenance record for the WebUI visual
migration decided in [ADR 0002](../../adr/0002-webui-emerald-visual-authority.md).
It holds the upstream reference, the component mapping, the asset licences, and
the register of every place where PlatPulse deliberately differs from upstream.

## Where each section lives

The record is split by area into the files beside this README, so each file stays within the document
budget (`scripts/check-doc-budgets.py`). Every section keeps its number and heading, and every image and
evidence link stays relative to this directory. A new section goes into the file of its area, or into a
new file listed here when that file would exceed the budget.

| Sections | Contents | File |
|---|---|---|
| 1 | Reference revision: upstream repository, commit, licence, declared package ranges | this file, below |
| 2–3 | Vendored assets with their sources and licences; upstream → PlatPulse component mapping | [assets-and-component-mapping.md](assets-and-component-mapping.md) |
| 4, deviations 1–27 | Deviation register: status tokens, Home tiles, map, flags, fonts, Admin primitives, single stylesheet, Home Node card and grid | [deviation-register-1-27.md](deviation-register-1-27.md) |
| 4, deviations 28–43 | Deviation register: Node Detail six-chart deck, Metric Window selector, network tooltip, back-top and sonner, Overview cards, `CardX`, Node card entrance animation | [deviation-register-28-43.md](deviation-register-28-43.md) |
| 5–22 | Screenshot evidence, verification results and the Playwright run history until the suite is green | [playwright-acceptance-runs.md](playwright-acceptance-runs.md) |
| 23–31 | Map evidence, comparison against the reference image, background regression, composition decisions, screenshot review, final continuation acceptance | [screenshot-review-and-acceptance.md](screenshot-review-and-acceptance.md) |
| 32–36 | Node Detail six-chart port (issue #225), its refinement rounds, coverage verdict, LoadChart.vue restoration and rendered-defect pass | [node-detail-six-chart-deck.md](node-detail-six-chart-deck.md) |
| Admin workspace follow-up | ADR 0003 supersedes the centered Admin shell | this file, below |

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

## Admin workspace follow-up (ADR 0003)

The centered 1280px Admin shell/page rule from the initial Emerald migration is
superseded by [ADR 0003](../../adr/0003-admin-adaptive-workspace.md). Admin now
uses the available width beside its existing sidebar; the header shares that
partition. Emerald visuals and public layout remain unchanged. Local form
width limits remain valid. The shell geometry test now protects workspace fill
and shared alignment rather than the former cap.

Matched screenshots and source audit: [Admin workspace evidence](admin-workspace/README.md).
