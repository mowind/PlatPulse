# Admin information layering — matched before/after evidence

Captured by `e2e/admin-layering-evidence.spec.ts` against the local seeded e2e Server
(`e2e/start-server.sh`, a fresh state directory for each phase). No production service is touched and
no credential is captured.

- `before/`: baseline `main` @ `c51382a`.
- `after/`: this round (webui.md §8.7).

Both phases apply the same local route fixture, which lengthens two display names in the real Server
response (Node C → a long operator name; Agent `…0021` → a long Agent name), so the data set covers:
normal (Node A), stale + RPC error + identity mismatch (Node B), never-observed Unknown (Nodes C, E, F,
G, P), retired (Node D), long Network/Node names (Home convergence fixtures), long IDs (UUIDs, 66-char
Genesis hash, 130-char P2P keys), non-zero recorded history (Agent `…0011`: 2 gap intervals, 2
security events, retained delivery error and dropped range), and a refresh failure
(`overview-refresh-error-1280-light.png`, the Overview refetch answered with 503 while the last good
values stay visible).

Files: `<page>-<width>-<theme>.png` for `overview`, `nodes`, `agents`, `validators`, `networks`,
`node-detail` and `agent-detail` (the default Overview section) at 390/768/1280/1440 in light and dark,
plus `metrics.json` per phase.

## Default-view weight (light theme)

`height` is the full page height in CSS px, `columns` the visible table header cells in `main`,
`text` the visible text length of `main`; `overflow` is page-level horizontal overflow (before / after).

| Page | Width | Height | Columns | Text | Overflow |
|---|---|---|---|---|---|
| overview | 390 | 8913 → 3024 (-66%) | 12 → 0 | 5520 → 2735 (-50%) | 0 / 0 |
| nodes | 390 | 8329 → 5775 (-31%) | 8 → 5 | 3426 → 3054 (-11%) | 0 / 0 |
| agents | 390 | 2066 → 1471 (-29%) | 6 → 5 | 1668 → 1054 (-37%) | 0 / 0 |
| validators | 390 | 8834 → 3003 (-66%) | 14 → 8 | 5884 → 1916 (-67%) | 0 / 0 |
| networks | 390 | 1216 → 1010 (-17%) | 7 → 5 | 688 → 587 (-15%) | 0 / 0 |
| node-detail | 390 | 7878 → 1376 (-83%) | 5 → 0 | 9451 → 1061 (-89%) | 0 / 0 |
| agent-detail | 390 | 8008 → 1723 (-78%) | 0 → 0 | 8086 → 1614 (-80%) | 0 / 0 |
| overview | 768 | 3327 → 2225 (-33%) | 12 → 0 | 5621 → 2735 (-51%) | 0 / 0 |
| nodes | 768 | 2636 → 1561 (-41%) | 8 → 5 | 3581 → 3157 (-12%) | 0 / 0 |
| agents | 768 | 1102 → 900 (-18%) | 6 → 5 | 1703 → 1072 (-37%) | 0 / 0 |
| validators | 768 | 3194 → 1366 (-57%) | 14 → 8 | 5973 → 1940 (-68%) | 0 / 0 |
| networks | 768 | 900 → 900 (0%) | 7 → 5 | 695 → 594 (-15%) | 0 / 0 |
| node-detail | 768 | 4780 → 933 (-80%) | 5 → 0 | 9451 → 1061 (-89%) | 0 / 0 |
| agent-detail | 768 | 5404 → 1185 (-78%) | 0 → 0 | 8086 → 1614 (-80%) | 0 / 0 |
| overview | 1280 | 2941 → 1716 (-42%) | 12 → 0 | 5630 → 2744 (-51%) | 0 / 0 |
| nodes | 1280 | 1690 → 1387 (-18%) | 8 → 5 | 3581 → 3157 (-12%) | 0 / 0 |
| agents | 1280 | 901 → 901 (0%) | 6 → 5 | 1703 → 1072 (-37%) | 0 / 0 |
| validators | 1280 | 2242 → 918 (-59%) | 14 → 8 | 5973 → 1940 (-68%) | 0 / 0 |
| networks | 1280 | 901 → 901 (0%) | 7 → 5 | 695 → 594 (-15%) | 0 / 0 |
| node-detail | 1280 | 4288 → 901 (-79%) | 5 → 0 | 9451 → 1061 (-89%) | 0 / 0 |
| agent-detail | 1280 | 4838 → 1117 (-77%) | 0 → 0 | 8086 → 1614 (-80%) | 0 / 0 |
| overview | 1440 | 2817 → 1716 (-39%) | 12 → 0 | 5630 → 2735 (-51%) | 0 / 0 |
| nodes | 1440 | 1419 → 1306 (-8%) | 8 → 5 | 3581 → 3157 (-12%) | 0 / 0 |
| agents | 1440 | 901 → 901 (0%) | 6 → 5 | 1703 → 1072 (-37%) | 0 / 0 |
| validators | 1440 | 1976 → 918 (-54%) | 14 → 8 | 5973 → 1940 (-68%) | 0 / 0 |
| networks | 1440 | 901 → 901 (0%) | 7 → 5 | 695 → 594 (-15%) | 0 / 0 |
| node-detail | 1440 | 4132 → 901 (-78%) | 5 → 0 | 9451 → 1061 (-89%) | 0 / 0 |
| agent-detail | 1440 | 4758 → 1117 (-77%) | 0 → 0 | 8086 → 1614 (-80%) | 0 / 0 |

The Overview drops its two diagnostic tables (12 header cells → none) for two five-item preview lists;
Node and Agent detail open on their `Overview` section, so history, diagnostics and danger zones are no
longer stacked below it. Agents and Networks heights are bounded by the 900px viewport at 768px and above;
their visible text still drops (about 38% and 15%). Nodes keep all 13 rows by design (the inventory is the page); the row loses the short ID, endpoint, revision, Identity/Visibility/Lifecycle columns and per-row teaching copy.

Behaviour (expansion, section switching with preserved drafts, deep links, filter chips, overflow) is
asserted in `e2e/admin-layering.spec.ts` and the unit tests, not by these images.
