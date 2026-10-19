# Admin interaction closure round — matched captures

Captured with `ADMIN_CLOSURE_EVIDENCE=before|after npx playwright test e2e/admin-closure-evidence.spec.ts --project=desktop-1280`
against the local seeded e2e Server (`e2e/start-server.sh`). `before` is 5f388ff, `after` is this round. Audit pages
(including the failing refresh and the slow first-page retry) and the long Network name are local Playwright route
fixtures; no production service or credential is involved.

Only the areas whose rendering changed are captured:

| Capture | Shows |
|---|---|
| `node-header-*`, `node-header-long-*` | Node detail header: the Network and Agent links keep their 44px boxes in the line instead of reaching over the Node ID `Show full` control (and, wrapped, over each other); the line is taller by the space those boxes already claimed |
| `node-header-from-results-*` | The same header opened from `/admin/nodes?health=unhealthy&sort=name`: `Back to results` replaces `All Nodes` |
| `node-unavailable-*` | `Node unavailable` now carries its page-local return (`All Nodes` when opened directly) |
| `audit-refresh-failed-*` | Two loaded pages whose shown page fails its refresh: before, nothing said so; after, one alert above the kept list with `Try refreshing again` |
| `audit-initial-retry-*` | A first-page retry in flight: before, the error row was replaced by the page loading row and focus fell to the page; after, the row stays with `Trying again…` holding focus |

The navigation drawer fix (ending the drawer at the sidebar breakpoint) changes no rendering of either layout and has
no capture. The tall phone Audit captures show the sticky Admin header where Playwright scrolled the element
screenshot; it is a capture artifact in both phases. Behaviour is asserted by `e2e/admin-closure.spec.ts` and the
unit tests, not by these images.
