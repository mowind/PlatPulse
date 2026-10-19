# Admin usability round — matched captures

Captured with `ADMIN_USABILITY_EVIDENCE=before|after npx playwright test e2e/admin-usability-evidence.spec.ts --project=desktop-1280`
against the local seeded e2e Server (`e2e/start-server.sh`). `before` is a098cc0, `after` is this round.
States the seed cannot produce (an empty inventory, a first-use Overview, Audit pages with a failing older
page) are local Playwright route fixtures; no production service or credential is involved.

| Capture | Shows |
|---|---|
| `node-detail-tabs-*` | Node B detail: the Diagnostics marker keeps the identity mismatch's existing error tone |
| `agent-detail-tabs-*` | Agent Manage tab with an unsaved draft: `Unsaved` is neutral, no longer the warning tint |
| `nodes-filter-chips-*` | Visibility and Lifecycle chips while More filters is collapsed; each chip now reserves its 44px clear target |
| `nodes-filtered-empty-*` | Filters that exclude every Node: the count of excluded Nodes and `Clear filters` (sort stays `Node name`) |
| `nodes-inventory-empty-390-*` | An empty inventory: waiting for an accepted Agent Report, `Open Agents` |
| `overview-setup-*` | First-use guide: no Settings link; `Add an Agent`; the local Node Inventory step is text |
| `audit-deep-link-390-light`, `audit-older-failed-*` | `#event-42` outside the loaded events, then a failed older page reported below the kept list with a same-cursor retry |

The `before` phase has no 1280 Audit pair: at a098cc0 the failed older page removed the `Load older events`
button, so the before run could not continue at that width (the defect this round fixes).
Behaviour is asserted by `e2e/admin-usability.spec.ts` and the unit tests, not by these images.
