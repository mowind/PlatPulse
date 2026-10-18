# Admin information layering — closure round, matched before/after evidence

Captured by `e2e/admin-layering-evidence.spec.ts` with
`ADMIN_LAYERING_EVIDENCE=before|after ADMIN_LAYERING_EVIDENCE_DIR=admin-layering-closure` against the
local seeded e2e Server (`e2e/start-server.sh`). No production service is touched and no credential is
captured. Both phases apply the same local route fixture and seed (normal Node A; stale + RPC error +
identity mismatch Node B; never-observed Nodes; retired Node D; a long Node and a long Agent name;
Agent `…0011` with Server Attention Items plus multiple recorded findings).

- `before/`: `main` @ `384be92`.
- `after/`: this round (webui.md §8.7 "Agent evidence summary rule" and "Nodes on phones").

Files: `<page>-<width>-<theme>.png` at 390/768/1280/1440 in light and dark for `overview`, `nodes`,
`agents`, `validators`, `networks`, `node-detail`, `agent-detail` (default Overview section) and
`agent-diagnostics` (where the complete recorded evidence lives), plus `metrics.json` per phase.

## Default-view weight (light theme; dark matches within a few characters)

| Page | Width | Height (CSS px) | Visible text | Page overflow |
|---|---|---|---|---|
| overview | 390 | 3060 → 3004 (-2%) | 2744 → 2770 (+1%) | 0 / 0 |
| nodes | 390 | 5775 → 4034 (-30%) | 3054 → 3054 (+0%) | 0 / 0 |
| agents | 390 | 1471 → 1471 (+0%) | 1054 → 1100 (+4%) | 0 / 0 |
| node-detail | 390 | 1376 → 1376 (+0%) | 1061 → 1061 (+0%) | 0 / 0 |
| agent-detail | 390 | 1723 → 1635 (-5%) | 1614 → 1473 (-9%) | 0 / 0 |
| agent-diagnostics | 390 | 3189 → 3213 (+1%) | 2344 → 2363 (+1%) | 0 / 0 |
| overview | 768 | 2225 → 2225 (+0%) | 2744 → 2779 (+1%) | 0 / 0 |
| nodes | 768 | 1561 → 1920 (+23%) | 3157 → 3157 (+0%) | 0 / 0 |
| agents | 768 | 900 → 900 (+0%) | 1072 → 1118 (+4%) | 0 / 0 |
| node-detail | 768 | 933 → 933 (+0%) | 1061 → 1061 (+0%) | 0 / 0 |
| agent-detail | 768 | 1185 → 1116 (-6%) | 1614 → 1473 (-9%) | 0 / 0 |
| agent-diagnostics | 768 | 1987 → 2010 (+1%) | 2344 → 2363 (+1%) | 0 / 0 |
| overview | 1280 | 1716 → 1716 (+0%) | 2744 → 2779 (+1%) | 0 / 0 |
| nodes | 1280 | 1387 → 1676 (+21%) | 3157 → 3157 (+0%) | 0 / 0 |
| agents | 1280 | 901 → 901 (+0%) | 1072 → 1118 (+4%) | 0 / 0 |
| node-detail | 1280 | 901 → 901 (+0%) | 1061 → 1061 (+0%) | 0 / 0 |
| agent-detail | 1280 | 1117 → 1088 (-3%) | 1614 → 1473 (-9%) | 0 / 0 |
| agent-diagnostics | 1280 | 1737 → 1760 (+1%) | 2344 → 2363 (+1%) | 0 / 0 |
| overview | 1440 | 1716 → 1716 (+0%) | 2744 → 2770 (+1%) | 0 / 0 |
| nodes | 1440 | 1306 → 1506 (+15%) | 3157 → 3157 (+0%) | 0 / 0 |
| agents | 1440 | 901 → 901 (+0%) | 1072 → 1118 (+4%) | 0 / 0 |
| node-detail | 1440 | 901 → 901 (+0%) | 1061 → 1061 (+0%) | 0 / 0 |
| agent-detail | 1440 | 1117 → 1088 (-3%) | 1614 → 1473 (-9%) | 0 / 0 |
| agent-diagnostics | 1440 | 1737 → 1760 (+1%) | 2344 → 2363 (+1%) | 0 / 0 |

Reading the numbers:

- **Nodes, 390**: the five stacked label/value sections per card become a grouped card (name; Network
  and Identity exception; Health beside Freshness; Head / Sync beside Details): about a third shorter
  with exactly the same text.
- **Nodes, 768 and above**: taller. The Node and Network links used to reach 44px only through negative
  margins that overlapped each other's hit areas; they are now two stacked 44px targets with no overlap,
  which is the honest cost of keeping both targets. The five columns are unchanged.
- **Agent detail Overview**: the joined list of every recorded finding becomes a count, the highest
  item and `Open Diagnostics`; the Overview no longer repeats the credential validity counts (the header
  keeps `N active · M total` with the single `Manage credentials` entry). `agent-diagnostics` keeps
  every finding; none was removed (its ~1% difference is time-dependent text between the two captures).
- **Agents / Overview**: a few more characters, because the summary now says `Critical ·` /
  `Not observed ·` in words and the folded counts name criticals per group.

Behaviour (summary selection, counts, hit testing, phone grouping, expansion, sections, drafts, deep
links) is asserted in the unit tests and `e2e/admin-layering.spec.ts`, not by these images.
