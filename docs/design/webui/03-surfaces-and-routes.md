Part of the [PlatPulse WebUI design](../webui.md): §3–§4. Sections keep their numbers; cite them as `webui.md §X.Y`.

## 3. Surfaces and authorization

### 3.1 Home Dashboard

Home is a read-only surface for Public Projections. The current Public adapters consume the Network list, Node detail, Node history, metric history, selected-Node Peer History, and the country Geo data behind the Peer country map, where the DTO exposes them:

```text
Home
├── All Networks
│   └── Node list/cards
└── Node Detail
    ├── Shared identity Header, evidence-backed role/Activity, report age and Node ID copy
    ├── Six operational KPIs and Node Peer Country View
    ├── Chain state, Process collection/start/Node Data, and shared Host resources
    ├── Validator performance and diagnostics (confirmed Validator only; stale qualified)
    └── Latest 60 seconds: six shared charts
```

Home is organized Network → PlatON Node, never Agent → Node. Agent and Host topology belongs to Admin.

When Site Access Mode is Public, anonymous Guests can read the allowed Home projection paths; when Private, Home routes require an authenticated Owner or Viewer session. Every Active Node returned by the Public projection appears on Home. The Admin `visibility` field/filter is a legacy compatibility surface and is not used by current Public SQL to hide a Node.

Validator Activity is copied from the Server Public DTO, not inferred in React. `empty`/`not_found` is `observing`; a successful canonical value is `current` or `stale` according to Server freshness; an error with last-good Activity is canonical Activity with `stale`; success without Activity and `unsupported` are `unknown`. Canonical values include `candidate`, `active`, `producing`, `exiting`, `exited`, `verifying`, and `locked`.

For retired, deleted, forbidden, or unknown Nodes, Public routes use non-leaking unavailable semantics such as “This Node is no longer available.” Admin routes may distinguish forbidden from not-found.

### 3.2 Admin Dashboard

Admin requires an authenticated Owner session and is rejected for Viewer sessions. Human roles are Owner and Viewer; Guest is anonymous access, not a role or credential. A Viewer can log in and read Home in Private mode but sees the non-leaking Owner-required Admin outcome.

Admin groups:

1. Overview;
2. Agents;
3. Nodes;
4. Networks;
5. Validators (issue #218);
6. Settings;
7. Incidents (the Alerts group; issue #203);
8. Sessions and Audit.

Admin covers configuration and diagnostics; it must not duplicate Home's full Node Detail card/chart deck. The Admin Node endpoint returns the full administrative `AdminNodeDetail` DTO (health, Node Currency, process/data/RPC/Sync/Consensus/Peer diagnostics, identity, high-watermark/resync, and transfer context); the current page renders the approved administrative/diagnostic subset and does not turn it into a second Home view.

Every Admin render begins with `Checking access…` when authorization is unresolved. It never flashes data from a previous session.

The Admin shell shares Home's accepted Emerald visual language in both themes: a Slate-50-like canvas with quiet light panels in Light, the near-#141923 canvas with quiet dark panels in Dark, restrained outlines and shadow, high-contrast primary text, quiet secondary copy, and measured Emerald selection accents. It keeps its undecorated workbench background — the public gradient/grid never crosses its reading surface (the no-grid exception is explicit in §8.5.2 and §11.1). The Owner-only shell keeps its management information architecture: a persistent desktop sidebar and an accessible tablet/phone drawer with focus entry, Tab trapping, Escape and scrim close, body scroll lock, and focus restoration. Header and navigation controls remain at least 44×44 CSS pixels.

The first shared-theme proof is `PAGE-ADMIN-OVERVIEW` at `/admin`. It presents the Server-owned attention queue, Node Health Summary, and Agent inventory as independent Admin query/realtime surfaces. Starting, Empty, Error, Stale, last-good, Unknown, never-observed, Disabled, and Unsupported states remain explicit in text plus an icon/shape or equivalent explanation; no intended state is represented only by color or converted to a zero, false, or Healthy value. The implementation caveat in §8.4.5 still applies to database failures that are currently masked by some Admin handlers.

### 3.3 Authorization generation

Authorization changes create a new access generation:

1. stop the old Admin/Public stream as applicable;
2. abort old requests;
3. clear affected sensitive cache;
4. discard responses from older generations;
5. reload authoritative REST state under the new authorization.

Session revoke, expiry, and Site Access Mode changes (Public ↔ Private) all use this sequence. Tokens and business DTOs are never transferred between tabs; tabs synchronize only an access-generation signal.

A Session probe has three answers, not two. Only an HTTP `401` whose error envelope carries `auth_required` means the reader is not signed in: that answer, and only that answer, closes the Admin/Public stream, clears the cache and moves the tab to Guest (Admin then routes to `/login` with the `sessionExpired` notice). Every other failure — a `502`/`503`/`504` from an intermediary or from the Server's own database-unavailable answer, a network error, an unreadable or empty body — is the Server being unreachable, not the Session being refused: the tab keeps the reader signed in with the Session it already held, leaves the streams and the access generation untouched, states `The Server cannot confirm your Session right now. Retrying automatically.` beside the reading it already holds (and `The Server is temporarily unavailable. Retrying automatically.` when it holds no Session to keep), and repeats the probe with a bounded backoff (`1 s`, `2 s`, `4 s`, `8 s`, `15 s`, then every `30 s`) until it is answered. A first probe that is never answered reaches the same unavailable state and declares no Guest, so a Server restart never presents the login form to a reader who is still signed in; once the Server answers with the same Session the shell resumes reading under a new access generation, and a Session it no longer accepts becomes Guest at that point. A successful sign-in ends the notice as well: it answers the question the probe was waiting on and cancels the retries, so nothing may keep claiming a retry is coming. A probe that was not answered is never an authorization change.

The Server keeps the same line: a Session query the database cannot answer is `503 unavailable`, never `401 auth_required`, and a bound Admin/Public stream whose Session re-check cannot read the database stays connected and re-checks on the next second instead of closing with a `reset` (issue #267). Staying connected is not an authorization, though: while the answer is missing the stream sends no invalidation events (the cursor does not move, so no event is lost), and the absolute lifetime verified when the stream was opened still ends it — an unreadable database must not extend a Session past its `expires_at`.

## 4. Route and page inventory

Each page has a stable ID. IDs are semantic and do not prescribe React filenames.

### 4.1 Public/Home pages

| Page ID | Route | Purpose | Actors |
|---|---|---|---|
| `PAGE-HOME-NETWORKS` | `/` | Active Node dashboard, Network filters, summary cards, and sort | Anonymous Guest (Public mode); authenticated Owner/Viewer (Private mode) |
| `PAGE-HOME-NODE` | `/nodes/:nodeId` | Public Node detail and independent observation dimensions | Anonymous Guest (Public mode); authenticated Owner/Viewer (Private mode) |
| `PAGE-HOME-UNAVAILABLE` | public fallback route/state | Non-leaking retired/deleted/unknown response | Guest, Owner, or Viewer according to access mode |

### 4.2 Authentication and access pages

| Page ID | Route | Purpose | Actors |
|---|---|---|---|
| `PAGE-AUTH-LOGIN` | `/login` | Human login with internal router-state `from` redirect | Owner or Viewer |
| `PAGE-ACCESS-SESSIONS` | `/admin/access/sessions` | Coarse session review and revoke | Owner |
| `PAGE-ACCESS-AUDIT` | `/admin/access/audit` | Immutable redacted Audit review | Owner |
| `PAGE-AUTH-REVOKED` | route-preserving login/revalidation state | Explain access generation transition | Owner/Viewer/expired |
| `PAGE-AUTH-FORBIDDEN` | protected route state | Explain insufficient access without leaking data | Viewer on Admin or unauthenticated Guest |

### 4.3 Admin pages

| Page ID | Route | Purpose | Actors |
|---|---|---|---|
| `PAGE-ADMIN-OVERVIEW` | `/admin` | Owner attention queue and operational overview | Owner |
| `PAGE-ADMIN-AGENTS` | `/admin/agents` | Agent inventory, liveness, spool diagnostics | Owner |
| `PAGE-ADMIN-AGENT-DETAIL` | `/admin/agents/:agentId` | Identity, credential status, liveness, inventory, diagnostics, Server-owned display name and notes | Owner |
| `PAGE-ADMIN-ENROLL` | `/admin/agents/enroll` | Add Agent: one-time Enrollment Token and local onboarding guidance; no placeholder Agent | Owner |
| `PAGE-ADMIN-NODES` | `/admin/nodes` | Node list, health summary, freshness, and legacy visibility filter | Owner |
| `PAGE-ADMIN-NODE-DETAIL` | `/admin/nodes/:nodeId` | Administrative view over the full AdminNodeDetail DTO; UI renders the approved diagnostic subset | Owner |
| `PAGE-ADMIN-NODE-INVESTIGATION` | `/admin/nodes/:nodeId/investigation` | One UTC window over a Node's own evidence: the seven families as of their own time bases, their coverage, grains, holes, boundaries, retention and the Server's own answer paths | Owner |
| `PAGE-ADMIN-NETWORKS` | `/admin/networks` | Network Registry metadata and Nodes | Owner |
| `PAGE-ADMIN-NETWORK-DETAIL` | `/admin/networks/:networkKey` | Expected identity, metadata, mismatch diagnostics | Owner |
| `PAGE-ADMIN-SETTINGS` | `/admin/settings` | Global Block History window and Site Access Mode configuration | Owner |
| `PAGE-ADMIN-INCIDENTS` | `/admin/alerts/incidents` | Alert Incident history with state, severity, subject, evidence and durable Owner acknowledgment | Owner |
| `PAGE-ADMIN-INCIDENT-DETAIL` | `/admin/alerts/incidents/:incidentId` | One Incident occurrence: occurrence, acknowledgment, evaluation, suppressions and evidence | Owner |
| `PAGE-ADMIN-RULES` | `/admin/alerts/rules` | Typed Alert Rule catalog with baseline version, state, severity and evaluated counts | Owner |
| `PAGE-ADMIN-RULE-DETAIL` | `/admin/alerts/rules/:ruleKey` | One Rule: baseline, effective inheritance and overrides, typed edit, preview, immutable versions and subject state | Owner |
| `PAGE-ADMIN-SILENCES` | `/admin/alerts/silences` | Time-bounded delivery-suppression policies: matching scope, status and effective window, create and confirmed cancel | Owner |
| `PAGE-ADMIN-MAINTENANCE` | `/admin/alerts/maintenance` | Time-bounded Maintenance Windows for an Agent/Node/Network scope and expected Rule keys, with confirmed cancel | Owner |
| `PAGE-ADMIN-NOTIFICATIONS` | `/admin/notifications` | Notification section overview: masked channel status, delivery semantics, and the request-reconciliation lookup | Owner |
| `PAGE-ADMIN-NOTIFICATION-EVENTS` | `/admin/notifications/events` | Notification Event history with kind filter, cursor paging, and each Event’s Delivery outcome | Owner |
| `PAGE-ADMIN-NOTIFICATION-EVENT-DETAIL` | `/admin/notifications/events/:eventId` | One Notification Event: incident/subject context and its Delivery attempts | Owner |
| `PAGE-ADMIN-NOTIFICATION-DELIVERIES` | `/admin/notifications/deliveries` | Delivery history with state filter, attempt history, and retryable state | Owner |
| `PAGE-ADMIN-NOTIFICATION-DELIVERY-DETAIL` | `/admin/notifications/deliveries/:deliveryId` | One Delivery: channel attempt history, provider result, Audit entry, and the audited retry command | Owner |
| `PAGE-ADMIN-NOTIFICATION-CHANNELS` | `/admin/notifications/channels` | Configured channel status, the controlled test with its inspectable result, and the request-id lookup | Owner |
| `PAGE-ADMIN-OPERATIONS` | `/admin/operations` | Recorded Operation ledger with kind/status filters, live progress, cancel-requested state, an explicit cancel request, and the read-only storage capacity and low-space protection panel | Owner |
| `PAGE-ADMIN-OPERATION-DETAIL` | `/admin/operations/:operationId` | One recorded Operation: Server-owned status and progress, issues, recorded outcome and result payload, and the audited cancel request | Owner |
| `PAGE-ADMIN-DOCTOR` | `/admin/doctor` | Doctor overview with the last recorded report and its checks, any in-flight run, and the audited Run Doctor command | Owner |
| `PAGE-ADMIN-BACKUPS` | `/admin/backups` | Recorded Backup artifacts with the manifest facts the Server wrote and each artifact’s last recorded verification outcome | Owner |
| `PAGE-ADMIN-BACKUP-DETAIL` | `/admin/backups/:artifactId` | One recorded artifact: manifest, SHA-256, data window and creating task, the recorded verification outcome and its reason, and the audited verification request | Owner |
| `PAGE-ADMIN-VALIDATORS` | `/admin/validators` | Automatic Validator identity coverage per Node and the resolved Validator registry with each identity's current status | Owner |
| `PAGE-ADMIN-VALIDATOR-DETAIL` | `/admin/validators/:validatorId` | One resolved Validator: current status, Provider evidence, staking metrics, and every Node association including ended intervals | Owner |
| `PAGE-ADMIN-RETENTION` | `/admin/retention` | Retention policy table with each supported family's bounds and safety floor, the version-safe policy edit with its read-only impact preview, the Server-authoritative preview bound to policy versions, scope and fixed cutoff, and the preview-bound run request | Owner |

The table above is the complete set of concrete SPA page routes; unknown paths under `/admin` use the registered Admin wildcard fallback rather than a legacy page. The Server/Admin APIs additionally expose People, Validator management/links/analytics, Restore, Node Transfer, and Agent recovery/credential operations; these are available DTO/operation surfaces, not current SPA pages. The Retention policy surface is routed as the Retention page listed above (issue #210, §15.13): it reads the supported families with their bounds and safety floors, saves a policy only within those bounds and only under the version the operator read, composes a Server-authoritative impact preview bound to the policy versions, the scope and a fixed per-family cutoff, and queues a run by that preview id. This surface executes no deletion of its own beyond the Server's recorded bounded Operation, and it never presents an estimate as a frozen row set. Operations (list, detail, cancel) and Doctor (overview, run) are routed as the Operations section pages listed above (issue #208, §15.11), and that same Operations page carries the read-only storage capacity and low-space protection panel (issue #212, §15.15) that shows the recorded protection intervals and the optional samples they skipped. The Backup artifact surface is routed as the Backups pages listed above (issue #209, §15.12): it lists recorded artifacts, reads one artifact’s recorded detail, and requests the Server’s read-only verification. Creating, restoring, and deleting an artifact remain offline commands on the Server host (ADR 0008), so no page offers them. Alert Rule management is routed as read/edit/preview with Network and Node overrides and version-safe saves (issue #204, §15.7); Silence and Maintenance Window management are routed as create/cancel lists (issue #205, §15.8); the Notification test, Event/Delivery history, and command-reconciliation surfaces are routed as one Notifications section (issue #206, §15.9). The Incident history pages above (issue #203) only read Incidents and record an Owner acknowledgment. The retention-execution and cancellation surface is routed with the Retention and Operation detail pages (issue #211, §15.14), the Node detail page's raw/tier `Metric history` (issues #213 and #214, §15.16) and the shared `Host metric history` read by the Node detail and Agent detail pages (issue #215, §15.17) are routed, as is the storage mount list of the Agent detail page (issue #216, §15.18), which reads the mount paths that Agent reported so a storage series is reachable by the path it was reported under, and, as the recorded sync and consensus state history of the Node detail page (issue #217, §15.19), which reads the states the Server itself recorded for a Node so a change is proven by the row that recorded it and a stretch without a record stays a silence instead of being inferred from a height. Geo provider status and selection are consumed by the Settings page.

The Validator identity coverage and current-status surfaces are routed as the Validators pages listed above (issue #218, §15.20): the coverage page reads `GET /api/admin/v1/validator-identities` and the registry reads the existing Admin Validator endpoints, and one Validator's page shows its current status, its Provider evidence, its staking metrics and every Node association the Server still records for it, including the intervals a key change ended; a Node Purge deletes the purged Node's own intervals together with the Node, so the table keeps the associations that still exist instead of reconstructing a deleted one. No page on this section offers a manual identity or role entry.

The current SPA has no generic `returnTo`/`return_to` mutation contract. When a protected Home route sends a Guest to `/login`, it carries the internal router pathname as `location.state.from`; a successful login navigates back to that pathname, or `/` when absent. Admin mutations stay on their current route and invalidate/refetch authoritative data.
