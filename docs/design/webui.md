# PlatPulse WebUI Design and Current Routed Surface

**Status:** Current routed WebUI contract with the explicitly approved Node Detail presentation target in [§11.1](#node-detail-composition-page-home-node), reconciled with [App.tsx](../../platpulse-web/src/App.tsx) and the Server DTOs, plus an explicitly **accepted, not implemented** evolution in [§15](#accepted-management-ui-target). Existing route tables describe today's SPA, not delivery of the new controls.

**Scope:** The production React SPA surface currently registered in `platpulse-web`, plus the read-only Public projections it consumes. Server/API extensions that have no SPA route are documented as available-but-unrouted, not silently treated as pages.

**Primary sources:**

- `CONTEXT.md` for domain vocabulary;
- `docs/design/platpulse.md` for Server, Agent, API, security, and deployment boundaries;
- generated OpenAPI artifacts for DTOs, operations, error envelopes, and client behavior.

This document is the WebUI UX and interaction authority for the current routed surface. It does not replace OpenAPI and does not define Server policy. The current Home consumes the Network list, Node detail with its history and metrics, the Peer country map, and selected-Node Peer Insight/Peer History; the Owner selects the Geo provider (Disabled, Local MMDB, IPinfo, or GeoJS) on Settings. Server-side Alert, Notification, Retention, Backup/Restore, Doctor, Node Transfer, People, Validator management, and Agent Enrollment/Recovery/Rotation operations exist, but their management pages are not registered in the current SPA; older page drafts are historical and must not be linked as live routes.

## 1. Purpose and non-goals

PlatPulse WebUI presents operational truth from the Server and gives the Owner safe, audited configuration. Home is the read-only, Node-first monitoring surface — readable by anonymous Guests when Site Access Mode is Public and by authenticated Owner or Viewer sessions when Private; Admin is the Owner-only overview and configuration surface. It is a monitoring and administration surface, not a remote-control terminal.

### 1.1 In scope

- Home Dashboard: read-only Public Projections, Node → Node Detail, a compact six-statistic overview following the current Network selection with the Peer country map, and a current block interval derived from the latest two consecutive retained Block Summaries;
- Admin Dashboard: Agent/Node/Network configuration, global history window, Site Access Mode, Sessions, and Audit;
- responsive behavior at 360×800, 390×844, 768×1024, and 1280×800;
- current aggregate Peer Insight and selected-Node Peer History; Peer Snapshot/Presence data is Server-side and redacted, while raw Peer identities are never displayed;
- independent collection, freshness, value, and authorization states;
- REST-authoritative data loading and SSE invalidation;
- accessible forms, tables/cards, confirmations, errors, conflicts, audit links, and session transitions;
- deterministic Playwright-oriented acceptance scenarios.

### 1.2 Out of scope

These are current-surface exclusions. §15 specifically accepts future Agent enrollment guidance, editable Agent metadata, Agent Removal, Node Purge, and Agent Attention Acknowledgment; it does not add remote control or the other deferred management pages.

- Agent, Server, SQLite, or evaluation implementation;
- a duplicated full Node Detail inside Admin;
- RPC Endpoint editing, RPC Endpoint failover, remote commands, restart, upgrade, Docker control, or terminal access;
- TUI, arbitrary scripts, SQL/DSL alert rules, or remote-control UI;
- Validator/Alert/Notification/Retention/Backup/Restore/Doctor/Node Transfer/People/Enrollment/Recovery/Rotation management pages (their Server/API operations exist but are not currently routed); Geo provider selection and the global Geo refresh are both routed on Settings;
- raw Peer identity/addresses, complete Peer Snapshot browsing, multi-tenant, HA, PostgreSQL, SSO/OIDC/TOTP/WebAuthn;
- runtime theme/script injection or a second frontend framework.

## 2. Authorities and vocabulary

Use the exact domain terms in `CONTEXT.md`. Current terms include: Host, Agent, PlatON Node, Node ID, RPC Endpoint, Network, Network Identity, Network Registry, Node Inventory, Active Node, Retired Node, Component Observation, Agent Report, Report Receipt, Current Projection, Block Summary, Peer Snapshot, Peer Insight, Peer History, Host Observation, Node Process Observation, Node Chain Observation, Node Observation, Node Health Summary, Validator, Validator Activity, Geo Provider, Geo Database, Geo Location Cache, Geo Insight, Attention Item, Public Projection, Site Access Mode, Invalidation Event, Home Dashboard, Admin Dashboard, Audit Event, Owner, Viewer, and Guest.

The WebUI must not invent synonyms that blur boundaries. In particular:

- Home is not “public admin”;
- Node Health Summary is not a WebUI-computed health color; the two-state Node marker on Home and Node Detail only reflects the Server-owned health word (§5.4);
- an Agent that stops reporting does not retire its Nodes;
- an Agent-level page must not merge independent Node chain observations;
- a successful Peer Snapshot/aggregate with zero peers is authoritative, not Unknown; omitted or unsupported peer capability is not an empty snapshot;
- recent Block History is bounded by the Server window and is best-effort: normal missed blocks remain absent, while explicit bounded Gap Backfill may recover eligible heights; neither path synthesizes zeroes or fabricated summaries.

### Approved Emerald composition refinements

- Home keeps the entire page at the existing **1280px cap** and preserves the Node grid’s `repeat(auto-fill, minmax(300px, 1fr))` column rules. At **1024px and above**, the overview allocates its statistics/map tracks about **4:3**: six equal summary cards in three columns by two rows at left, a complete contained **2:1** map at right. The 4:3 split holds each tile at the width the original four-card 2x2 grid gave it instead of stretching it, and the tiles keep its compact height: a 44px header row carrying the title and either the metric marker or the Breakdown control, then the value over a shared one-line stat slot — no footer row. The two cumulative tiles fill that slot with their own scope, coverage denominator and Partial marker, so all six tiles still share one height and one value baseline. Below 1024px, statistics precede the map; they use three columns from 640px and two on phones, keeping the same six-item DOM order. This human-approved presentation supersedes the earlier 37:61/232px band rather than widening the page or changing Node columns.
- Login displays the PlatPulse brand/Home link and the theme control in the same 56px Emerald header treatment and 1280px content column as Home. It exposes no Admin controls and does not mount the Home data shell. The existing form, authentication and redirect behavior are unchanged.
- Checkbox and radio visuals use 16px indicators inside 44px native input targets. Native grouping, labels, keyboard behavior, form state and disabled fieldsets remain authoritative; the visible indicator carries focus and invalid states. Forced-colors mode restores native rendering instead of relying on decorative colors.

### 2.1 Fixed status vocabulary

Use these terms in labels, accessible text, filters, and tests:

```text
Starting
Current
Stale
Error
Unknown
Disabled
Unsupported
Empty
Peer data current
Connecting to live updates
Live updates paused
You are offline
```

`Peer data current` is a scoped, low-weight healthy summary for Peer Insight; it does not replace the independent Collection, Freshness, or Value terms when those dimensions need explanation. `Online` and `N/A` are not generic replacements. Status communication always includes text and an icon or equivalent explanation; color is supplementary. An open SSE stream renders no transport notice: the positive transport state is deliberately silent because it changes nothing the visitor should do or believe. `Live updates connected` is therefore not part of this vocabulary and must not be reintroduced; only `Connecting to live updates` and `Live updates paused` are rendered and announced.

## 3. Surfaces and authorization

### 3.1 Home Dashboard

Home is a read-only surface for Public Projections. The current Public adapters consume the Network list, Node detail, Node history, metric history, selected-Node Peer History, and the country Geo data behind the Peer country map, where the DTO exposes them:

```text
Home
├── All Networks
│   └── Node list/cards
└── Node Detail
    ├── Two-state Node Health marker, Validator membership role, and process uptime
    ├── Head, QC, Locked, Committed, and Validator membership
    ├── Process start and last report times
    ├── Process CPU and memory percentages
    ├── Node Data size/capacity progress
    ├── Host network rates and Peer connections
    └── Current block interval and latest transaction count
```

Home is organized Network → PlatON Node, never Agent → Node. Agent and Host topology belongs to Admin.

When Site Access Mode is Public, anonymous Guests can read the allowed Home projection paths; when Private, Home routes require an authenticated Owner or Viewer session. Every Active Node returned by the Public projection appears on Home. The Admin `visibility` field/filter is a legacy compatibility surface and is not used by current Public SQL to hide a Node.

Validator Activity is copied from the Server Public DTO, not inferred in React. `empty`/`not_found` is `observing`; a successful canonical value is `current` or `stale` according to Server freshness; an error with last-good Activity is canonical Activity with `stale`; success without Activity and `unsupported` are `unknown`. Canonical values include `active`, `producing`, `exiting`, `exited`, `verifying`, and `locked`.

For retired, deleted, forbidden, or unknown Nodes, Public routes use non-leaking unavailable semantics such as “This Node is no longer available.” Admin routes may distinguish forbidden from not-found.

### 3.2 Admin Dashboard

Admin requires an authenticated Owner session and is rejected for Viewer sessions. Human roles are Owner and Viewer; Guest is anonymous access, not a role or credential. A Viewer can log in and read Home in Private mode but sees the non-leaking Owner-required Admin outcome.

Admin groups:

1. Overview;
2. Agents;
3. Nodes;
4. Networks;
5. Settings;
6. Sessions and Audit.

Admin covers configuration and diagnostics; it must not duplicate Home's full Node Detail card/chart deck. The Admin Node endpoint returns the full administrative `AdminNodeDetail` DTO (health, freshness, process/data/RPC/Sync/Consensus/Peer diagnostics, identity, high-watermark/resync, and transfer context); the current page renders the approved administrative/diagnostic subset and does not turn it into a second Home view.

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
| `PAGE-ADMIN-NETWORKS` | `/admin/networks` | Network Registry metadata and Nodes | Owner |
| `PAGE-ADMIN-NETWORK-DETAIL` | `/admin/networks/:networkKey` | Expected identity, metadata, mismatch diagnostics | Owner |
| `PAGE-ADMIN-SETTINGS` | `/admin/settings` | Global Block History window and Site Access Mode configuration | Owner |

The table above is the complete set of concrete SPA page routes; unknown paths under `/admin` use the registered Admin wildcard fallback rather than a legacy page. The Server/Admin APIs additionally expose People, Validator management/links/analytics, Alerts, Notifications, Operations, Retention, Backups/Restore, Doctor, Node Transfer, and Agent recovery/credential operations; these are available DTO/operation surfaces, not current SPA pages. Geo provider status and selection are consumed by the Settings page.

The current SPA has no generic `returnTo`/`return_to` mutation contract. When a protected Home route sends a Guest to `/login`, it carries the internal router pathname as `location.state.from`; a successful login navigates back to that pathname, or `/` when absent. Admin mutations stay on their current route and invalidate/refetch authoritative data.

## 5. Shared state model

The WebUI never reduces component collection, data age, available value, and authorization into one boolean.

### 5.1 Collection state

```text
Starting | Ok | Error | Disabled | Unsupported
```

- `Starting`: no usable observation for the current request yet;
- `Ok`: collection completed, with a value or authoritative empty result;
- `Error`: collection failed; last-good value may remain visible;
- `Disabled`: deliberately not collected/configured;
- `Unsupported`: capability is not available.

### 5.2 Freshness state

```text
Fresh | Stale | Unknown
```

Freshness, `observedAt`, `receivedAt`, `staleSince`, and reason come from Server REST DTOs. The WebUI formats them but never derives business freshness from `Date.now()`.

### 5.3 Value state

```text
Current | LastGood | AuthoritativeEmpty | None
```

- Error + LastGood remains visible with explicit error and age;
- Missing, Unknown, never-observed, and Disabled or Unsupported data without a retained successful value never render as `0`, `false`, or Healthy;
- a retained successful value, including an authoritative `0`, remains visible under collection Error, Starting, Disabled, Unsupported, or Stale freshness with an explicit last-successful qualifier;
- authoritative empty is not Unknown;
- recent Block History is bounded by the Server window and best-effort: absent blocks stay absent, never synthetic zeroes;
- host observation is collected once per Agent and referenced by Node views.

### 5.4 Node Health Summary

Severity and primary reasons are Server-owned. The WebUI presents the Summary and dimension reasons; it does not reimplement health policy or merge Node observations at Agent level.

Where Home or Node Detail mark a Node by name, a two-state marker sits before that name: Healthy is green and every other state - Unhealthy, Unknown, and never-observed - is grey. The marker carries the Server-owned health word as its accessible name, so the compact presentation never drops Healthy, Unhealthy, or Unknown from the accessibility tree; it does not replace the Summary, and an exceptional reason stays visible as text. Admin keeps the full Server-owned severity presentation (§8.4.3).

### 5.5 Server-side state machines without current pages

Alert Rule/Incident/Silence/Maintenance evaluation and long-running Operation states are implemented in the Server and exposed through Admin APIs, but the current SPA has no routed management pages for them. They remain independent of the WebUI's Node Health Summary; a future page must consume the typed DTOs rather than recreate those state machines in the browser.

## 6. REST, query cache, and SSE

### 6.1 Production seam

```text
Generated OpenAPI client
  → typed API adapter
  → query/mutation and SSE invalidation layer
  → page/view components
```

The generated client is imported through the project client singleton. Its `{data, error}` result is handled explicitly; bodyless 204 responses are treated according to generated types and do not assume `null`.

The adapter owns:

- request ID propagation;
- session/CSRF handling;
- typed error-envelope normalization;
- access generation tagging;
- response revision comparison;
- transport versus domain error distinction.

### 6.2 Query namespaces

Public and Admin caches must not share query keys or cache objects:

```text
public:<resource>:<scope>
admin:<resource>:<scope>
```

The exact implementation key format may vary, but the namespace boundary is mandatory. Site Access Mode and session transitions clear affected keys before new responses are rendered.

### 6.3 Realtime flow

```text
route open
  → REST query
  → connect surface-specific EventSource
  → receive invalidation/reset
  → discard older revision
  → invalidate exact query key
  → REST refetch
```

SSE contains invalidation/resource identity or collection reset, not authoritative business DTOs. One stream exists per surface shell per browser tab. High-frequency invalidations are coalesced. Hidden tabs reduce non-critical refetch while visible critical changes remain prompt.

The Public/Admin event endpoints accept cursor recovery through the `after` query parameter or the `Last-Event-ID` header (the reconnect header takes precedence when both are supplied). Responses are `text/event-stream`; each emitted event uses the `invalidation` event name, a numeric event id/cursor, and JSON `data` with `version`, `eventId`, `resource`, optional `resourceId`, `revision`, and optional `reset`. A first connection without a cursor starts after the current buffered sequence because REST has already supplied the snapshot; the browser should send the last received cursor when reconnecting and discard/reconcile events older than its current authorization generation.

SSE connection status is visible but does not cover valid content. Public Home and Node Detail surfaces show `Connecting to live updates` while connecting and `Live updates paused` after disconnect; an open stream renders no transport notice at all, because a connected stream changes nothing the visitor should do or believe. Browser/network loss additionally uses `You are offline` when the browser signal is authoritative. The compact Admin header retains its existing `Starting`/`Current` transport labels so public transport wording does not increase Admin information density. These transport labels never certify REST refresh success, observation freshness, Agent liveness, or Node Health. The current generated OpenAPI records the stream operation but does not fully describe every cursor/header/event field; runtime `realtime` behavior is the authority for replay.

SSE updates must preserve filters, sorting, scroll, expansion, and ordinary drafts. They do not reorder a list merely because a timestamp changed.

### 6.4 Mutation flow

- no optimistic business state;
- no automatic mutation retry or replay;
- success immediately invalidates and refetches authoritative REST;
- failure preserves drafts and shows field/page errors;
- conflicts refetch current authoritative state without overwriting drafts;
- access changes abort old requests and discard old responses.

## 7. Shared patterns

Stable semantic pattern references:

| Pattern ID | Contract |
|---|---|
| `PATTERN-STATUS-DIMENSIONS` | Collection, freshness, and value are displayed independently. |
| `PATTERN-ACCESS-CHECK` | First protected render is `Checking access…`; no old-data flash. |
| `PATTERN-AUTH-GENERATION` | Close old streams, abort requests, clear cache, discard old generation. |
| `PATTERN-CONFIRMATION` | High-risk actions use explicit confirmation and no optimistic result. The current History Window and Site Access DTOs accept `{confirmed: bool}`; a typed phrase may be a client-side friction guard but is not Server-authoritative security until a phrase field is added and validated by the Server. |
| `PATTERN-RESPONSIVE-TABLE` | Desktop table becomes priority cards; detail remains available without primary horizontal scroll. Agents uses the summary/detail split in §8.5. |
| `PATTERN-ADMIN-WORKBENCH` | Admin-scoped left alignment, available-width lists, contextual realtime status, independent selection/focus, and no page-level horizontal overflow (§8.5). |
| `PATTERN-LIVE-REGION` | Announce meaningful transitions only; do not announce high-frequency SSE. |
| `PATTERN-CONFLICT-RELOAD` | Show current server state and preserve user draft. |
| `PATTERN-REDACTED-DETAIL` | RPC Endpoints, credentials, raw Peer addresses, tokens, and complete bodies remain redacted. |

## 8. Page contract requirements

Every `PAGE-*` entry must specify the following before production coding:

1. user task and success outcome;
2. route and safe return behavior;
3. actor/permission boundary;
4. REST operations and generated DTO references;
5. query-key namespace and URL state;
6. SSE invalidations and reset behavior;
7. loading, empty, stale, error, Unknown, Disabled, Unsupported, forbidden, expired, conflict, and partial states;
8. mutation confirmation, refetch, and Audit behavior;
9. redaction and non-leaking copy;
10. desktop/tablet/mobile transformation;
11. heading, form, table, focus, live-region, zoom, and reduced-motion requirements;
12. Playwright scenario IDs.

### 8.1 Home Node Detail (`PAGE-HOME-NODE`)

- the page is one continuous reading surface in the order: an uncarded identity block (Home `All Networks` back context; two-state Server-owned Node Health marker; display name; Node ID; abnormal health reason; last Agent report time); a four-tile key summary; three parallel observation panels; the labelled real latest-60-second chart section; collapsible Peer diagnostics; collapsible low-frequency identifiers and technical details. The former Details/Network tabs are removed rather than kept as a competing layout;
- the identity block is page furniture rather than a card: it carries no bordered hero surface and no card container, and the summary tiles, the three observation panels, and the chart/disclosure cards are the page's only card containers;
- the key summary presents current `Head`, sync state and progress from the Server Observed Network Head with its confidence, current Peer Count, and PlatON process uptime in four summary tiles that fall from four to two columns with the viewport; a tile whose observation is unavailable, stale, or failed keeps its honest `Unknown`/last-good text and available explanation in place rather than fabricating `0`, `false`, or a Healthy value. Progress is asserted only from a high-confidence reference; a low or unknown confidence is shown as context, never as authoritative progress;
- three observation panels carry those groups as real cards side by side at desktop width. The first covers Node chain/consensus (the current Node's `QC`/`Locked`/`Committed`, `Validator` membership, resync, and reference context). The second covers PlatON process resources together with the Node Data directory (`CPU`, `Memory`, process start, process state; the directory percentage with used / hosting-filesystem-capacity bytes, never whole-Host disk usage). The third covers the shared Host aggregate resources (CPU, memory, storage, and sampled upload/download rates) explicitly labelled as collected once per Agent and shared by every Node it monitors. Every panel keeps one data item per row with its value at the right and its explanation below, and the panels stack in reading order below 48rem. Process uptime is never Host uptime, and process CPU/memory are never substituted by Host CPU/memory;
- the chart section is one labelled real latest-60-second window whose bounds come from the existing metrics response (`from`, `to`, `windowSeconds`). It carries the final six equal-size metric charts in this order: process CPU %, process memory %, shared Host upload/download rates, Peer inbound/outbound count, block interval, and transactions per block. The first four are line charts and the final two are bar charts; direction series carry readable labels/legends/units and are distinguished by blue/cyan plus text, never colour alone. Missing or failed metric history leaves every current value intact and renders an explicit per-chart state, so one failed or empty series degrades only its own chart;
- the bounded Block History list and public-history export are not rendered on Node Detail. The history endpoint remains a Server boundary; the page reads retained summaries only to derive the consecutive-block interval, and missing/non-consecutive summaries remain `Unknown`;
- selected-Node Public Peer Insight and aggregate Peer History live in a keyboard/touch-operable disclosure with the redaction note. No peer address or identity list is exposed;
- retired/deleted/unknown Nodes use the non-leaking unavailable semantics.

### 8.2 Admin Node Detail (`PAGE-ADMIN-NODE-DETAIL`)

The accepted future permanent-delete interaction is in §15.2; it is not implemented by the current metadata/lifecycle controls described here.

- consumes the full administrative `AdminNodeDetail` DTO, which includes display name, redacted RPC Endpoint diagnostics, Node Inventory/lifecycle (Active/Retired), health/freshness, process/data/RPC/Sync/Consensus/Peer diagnostics, identity/high-watermark/resync, and transfer context;
- the current page renders the approved administrative/diagnostic subset and must not reproduce Home's full observation cards/chart deck; it links to the shared `/admin/access/audit` page separately rather than treating Audit links as DTO fields;
- every mutation is audited; transfer fields are visible as data where supplied but no transfer management page is currently routed.

### 8.3 Settings (`PAGE-ADMIN-SETTINGS`)

- renders one Settings heading with an ordered History Window, Site Access Mode, then Geo provider module inside one left-aligned constrained surface (see §8.6);
- each card loads, mutates, and reports success or errors independently;
- History Window shows the current window, default, min/max bounds, and last update;
- History Window requires an integer in the Server bounds, a successful Server-authoritative impact preview, and the current mutation body `{confirmed: bool}` before mutation; values are rejected rather than clamped. A typed phrase, if displayed, is client-side friction only; it is not a Server security field;
- History Window copy states that shortening asynchronously deletes expired history and lengthening cannot recover deleted or missed history; success includes its Audit Event identifier;
- Site Access Mode uses text plus icon/equivalent semantics for Public or Private: Public permits anonymous Home reads, while Private requires authenticated Owner or Viewer; its current mutation body likewise uses `{confirmed: bool}`;
- switching Site Access Mode requires confirmation, records Audit, and performs the Public access-generation transition by closing affected streams, aborting old requests, clearing sensitive caches, discarding older responses, and reloading authoritative state;
- Geo provider offers only the providers this Server actually implements: Disabled, Local MMDB, IPinfo, and GeoJS. Disabled schedules no lookups at all; Local MMDB resolves countries from the operator-provided GeoLite2 Country database on the Server; IPinfo sends each observed Peer public IP to the fixed third-party endpoint `https://ipinfo.io/{ip}/json` and keeps only the returned country code; GeoJS sends it to the fixed third-party endpoint `https://get.geojs.io/v1/ip/geo/{ip}.json` and likewise keeps only the returned two-letter code. The option list, the disclosure, and the availability reason all come from the Server rather than from a hardcoded client list;
- whether a provider sends Peer addresses off the Server is the Server-owned `sends_peer_addresses` flag on each option, so the WebUI never hardcodes which providers do it: a flagged option is labelled as sending observed Peer public IPs to a third party and the card renders the Server-owned `disclosure` sentence for it, which names that provider's fixed destination, before the Owner selects it, together with the fact that only the Owner can enable it, that no upgrade enables it, and that the Server never falls back to another provider. The browser composes no disclosure of its own, so the destination the Owner is shown and the endpoint a request is really sent to come from the same Server constant and cannot drift apart;
- Geo provider shows the effective state (Disabled/Current/Stale/Error), where the current provider sends Peer addresses, whether a local database is configured, the cached country count, the pending lookup count (not scheduled while Disabled), the last success time, and the Server-owned rate-limit window while an external provider is backing off; an option the Server reports unavailable is rendered disabled with the Server-provided reason and cannot be submitted; success reports the Audit Event identifier;
- the Geo provider card carries the Owner-only global refresh (issue #136): the button submits the Server-owned mutation and the card then renders the Server's real per-address progress rather than treating the accepted request as success. It states that the action re-resolves every public Peer IP any Network currently references even when a cached result is still valid, that lookups are counted per distinct IP address, and that the Peer country map counts Peer records per Node. While the Server reports the run `running` the button is disabled and the counts advance; a terminal run renders `completed` or `stopped early` with the Server-owned `abort_reason`, the resolved / without-a-country / failed counts, the rate-limited count when non-zero, and the number of Peer records referencing those addresses. An empty scope is rendered as having nothing to re-resolve, and a run made under a different provider than the current selection says so instead of appearing as the new provider's result. Disabled or otherwise unavailable Geo renders the refresh disabled with the Server's `refresh_unavailable_reason` instead of a fake success. Progress stays truthful without a live SSE stream because the diagnostic query polls only while a run is running;
- the Settings cards stack on narrow viewports, preserve 44×44 CSS pixel targets, and never cause primary horizontal page overflow.

The retired `/admin/history-window` and `/admin/site-access` routes are not redirected; they resolve through the Admin Section not found fallback. This is a page-level SPA outcome and does not guarantee an HTTP 404 response from the Server.

#### 8.3.1 Settings integration acceptance

The Settings route is the single canonical configuration surface. The accepted scenarios map to `/admin/settings` as follows:

| Scenario | Route and outcome |
|---|---|
| `SCN-SETTINGS-ROUTE` | `/admin/settings` renders one logical `Settings` h1, ordered `History Window`, `Site Access Mode`, then `Geo provider` sections, and no obsolete navigation entries; the retired URLs remain on the Admin Section not found fallback without redirecting. |
| `SCN-SETTINGS-GEO-PROVIDER` | Through the Geo provider card, select an available provider, submit, and read the audited result plus the refreshed Server status; an unavailable option stays disabled with its reason and is never submitted; IPinfo and GeoJS are offered unselected with their own fixed destination and third-party consequence stated, and no unimplemented provider is offered. A browser scenario never selects an External Geo Provider: the destination is deliberately not configurable, so selecting one would send a Peer address to the real third party. |
| `SCN-SETTINGS-GEO-REFRESH` | With Local MMDB selected and an open Public Home view, the Owner triggers the global refresh, reads real terminal counts that are stated as IP lookups beside the separate Peer-record count, and sees the Public country buckets update without a reload and without a raw address or database path appearing on either surface; while Geo is Disabled the control stays disabled with the Server-owned reason and never reports a success that did not run; a Viewer cannot reach the card or the refresh route. |
| `SCN-GEO-BACKGROUND-RESOLUTION` | With Local MMDB, IPinfo, or GeoJS selected, the already-open Public Home view updates its Server-computed Known/Unknown country buckets through the existing realtime invalidation once the background resolution completes, without a reload and without exposing a Peer address, database path, or external request detail. |
| `SCN-HISTORY-WINDOW-SHORTEN` | Through the History Window card, show Server bounds and impact, require explicit confirmation (the current Server body is `{confirmed: bool}`), report the returned Audit Event, and retain asynchronous deletion consequences. |
| `SCN-HISTORY-WINDOW-BOUNDS` | Through the History Window card, reject blank, non-integer, and out-of-bounds values with field-level errors; never clamp or submit an invalid value. |
| `SCN-SITE-ACCESS-PUBLIC` | Through the Site Access Mode card, confirm and apply Public, clear affected Public state, reload the new authorization generation, and permit anonymous Home reads while Admin remains Owner-only. |
| `SCN-SITE-ACCESS-PRIVATE` | Through the Site Access Mode card, confirm and apply Private, close affected Public streams, clear old Public state, and require authenticated Owner or Viewer login for Home reads. |

Each Settings card keeps independent loading, mutation, success, field-error, page-error, confirmation, and recovery states. Browser back/forward preserves the canonical route context, while the Public/Admin DTO, cache, realtime, and Owner authorization boundaries remain separate.

### 8.4 Overview (`PAGE-ADMIN-OVERVIEW`)

The Overview is the Owner's triage surface, not a second copy of the Nodes or Agents inventory and not a remote-control console. It borrows Komari's compact scanning density, light operational canvas, restrained borders, strong primary numbers, and quiet secondary copy without importing Komari's database, subscription, traffic-ranking, pricing, latency-probe, or remote-operation model. The accepted order is `Attention -> Summary -> Node Health -> Agent inventory`.

#### 8.4.1 Composition and navigation

1. `Attention queue` is the first full-width panel. It presents current Server-derived Attention Items and their safe next actions.
2. Four compact summary cards follow: `Agents`, `Active Nodes`, `Retired Nodes`, and `Networks`. The cards use explicit counts and text legends, optionally with an accessible segmented bar; they do not use a single online/healthy percentage or circular score that can hide Unknown or Retired state.
3. `Node Health Summary` presents at most the ten highest-priority Active Nodes. The complete inventory remains at `/admin/nodes`.
4. `Agent inventory` presents at most the six highest-priority Agents. The complete inventory remains at `/admin/agents`.

Each summary card is one semantic link with a visible focus state and no nested action:

```text
Agents        -> /admin/agents
Active Nodes  -> /admin/nodes?lifecycle=active
Retired Nodes -> /admin/nodes?lifecycle=retired
Networks      -> /admin/networks
```

The primary values are:

```text
Agents:       total; online / offline / unknown
Active Nodes: active; healthy / unhealthy / unknown
Retired Nodes: retired
Networks:     total; with Network Identity Mismatch
```

`Active Nodes = healthy + unhealthy + unknown` and `total Nodes = active + retired`. Retired Nodes are excluded from live health buckets and Attention Items. `AdminOverviewSummary` has no `published` metric. Node list/detail DTOs and the Nodes page retain `visibility` as a legacy compatibility/diagnostic field and filter; current Public SQL does not use it to hide Home Nodes, so Site Access Mode remains the effective site-wide anonymous-access authority.

#### 8.4.2 Attention queue

**Baseline versus target:** the current kind + subject IDs and unacknowledged queue below are implemented behavior. §15.3 adds durable Agent Attention Acknowledgment and occurrence/evidence boundaries; the current IDs alone cannot permanently dismiss a type without also swallowing future occurrences.

An Attention Item is a current Server-derived prompt, not an Alert Incident, Notification, Audit Event, or browser-computed warning. The REST DTO keeps each problem independent with a stable `kind + subject` identity. The typed kinds are:

```text
agent_offline
agent_spool_fatal
agent_spool_overflow
agent_report_gap
agent_security_event
agent_shutdown_incomplete
agent_inventory_rejected
node_unhealthy
node_health_unknown
node_resync
node_identity_mismatch
```

Typed severities are `critical` and `warning`. Critical means a confirmed availability, data-integrity, or security risk:

```text
agent_spool_fatal
agent_spool_overflow
agent_security_event
agent_inventory_rejected
node_unhealthy
node_identity_mismatch
```

Warning means investigation is required without a confirmed critical loss:

```text
agent_offline
agent_report_gap
agent_shutdown_incomplete
node_health_unknown
node_resync
```

A new Agent with no accepted report is Unknown rather than Offline. A new Active Node remains Starting during the Server-owned first-observation grace period and does not produce `node_health_unknown` until that grace period expires. Unsupported, Disabled, stale, never-observed, and other incomplete states do not automatically become Critical. Network Identity Mismatch remains distinct from RPC Error and produces its own critical item because mismatched Block History must not merge into the registered Network.

The Server orders individual items by severity (`critical`, then `warning`), most recent authoritative `observed_at`, stable subject label, and stable identity. The WebUI groups items visually by `subject_kind + subject_id` without discarding or recomputing any item. A group uses its highest severity, exposes its primary issue, and offers expansion for additional issues. The panel reports both counts, for example `6 issues across 3 subjects`. Group order is highest severity, latest authoritative observation, subject label, and stable subject identity.

The first six subject groups are visible by default. `Show N more` and `Collapse` reveal or hide the remainder without losing the current items. Safe navigation is derived only from typed subject kinds and known same-origin Admin routes; the Server does not supply arbitrary URLs. Unknown future kinds remain visible with an Unknown fallback and no guessed link.

#### 8.4.3 Node Health Summary

Each row/card is exactly one Active Node and preserves Node scope. An Agent monitoring multiple Nodes produces separate Node rows/cards; block, transaction, consensus, peer, and error observations never merge into an Agent-level chain view.

The compact row keeps these priority fields visible:

```text
Node display name and shortened Node ID
Network
Server-owned Node Health Summary and primary reason
Freshness
Current Head
Resync state (full Sync diagnostics are on Node Detail)
Show diagnostics
View Node
```

The default priority is unhealthy, unknown health, stale, then healthy/current, with stable Network and Node-name tie-breakers. Timestamp-only invalidations do not reorder the list. At most ten Active Nodes are shown, followed by `Showing N of M Active Nodes` and `View all Nodes`. Retired Nodes remain in their summary card and the filtered Nodes inventory, not this live table.

`Show diagnostics` expands RPC, Sync, Consensus, Peers, Process, and Node Data in place; `View Node` opens the administrative Node Detail. Only one Node is expanded at a time, Escape collapses it, and an SSE-driven REST refetch preserves expansion by Node ID while that Node remains in the rendered set. Expansion shows collection, value, freshness, LastGood, time, and safe error context; it does not reproduce Home charts or the complete Home Node Detail.

#### 8.4.4 Agent inventory

Each compact inventory row represents one Agent and its one Host (see §8.6 for the summary-table delivery). Host Observation is shown once on that row and is never copied into every Node. The compact Node counts are joined from the already-loaded Admin Node list by stable `agent_id`; they preserve each Node's independent state rather than creating an Agent-level chain aggregate, and per-Node identity/health/freshness detail stays on the Nodes page and Agent Detail. The compact row may show:

```text
Agent identity and liveness
Last accepted report and sequence
Active / unhealthy / unknown Node counts
Compact Host CPU and memory values
Durable Spool queued reports, capacity, overflow, and fatal state
Clock status
Report sequence gaps and security-event count
Retained, active, unhealthy, and unknown Node counts
View Agent
```

Unknown Host CPU, memory, or Spool values remain Unknown rather than zero. CPU and memory may use small current-value progress tracks, but Overview has no Host rankings, 24-hour charts, or duplicated Host metrics. Normal Spool state stays quiet; fatal storage, discarded reports, delivery backlog, and other exceptional states are prominent. Boot IDs, complete credential state, shutdown evidence, complete Host observations, and detailed diagnostics belong to Agent Detail.

The default priority is Agents with critical diagnostics, then offline, unknown, and online Agents, with stable Agent ID tie-breaking. At most six cards are shown, followed by `View all Agents`.

#### 8.4.5 Data, time, and partial states

The page retains three independent Admin query surfaces:

```text
GET /api/admin/v1/overview -> Attention and summary response
GET /api/admin/v1/nodes    -> Node Health Summary
GET /api/admin/v1/agents   -> Agent inventory and Host diagnostics
```

Failure of one surface never hides successful data from another. Attention and summary are returned together by the Overview handler, but the current handler builds them with independent database reads and provides no cross-resource point-in-time/transaction snapshot guarantee. Initial loading uses explicit `Starting` text, optionally accompanied by static skeleton shapes. A refetch failure can preserve LastGood content and says that the last successful values remain visible. Each failed surface owns its own `Try again`. SSE carries only invalidation/reset; `Live updates paused` or `You are offline` never clears valid REST content.

Implementation caveat: the current `GET /api/admin/v1/agents` handler maps its initial list-query failure to `200 []`, and Agent Detail maps some nested query failures to `0`, `[]`, or `None`. These are degraded implementation behaviors, not authoritative Empty/zero values; the UI must avoid presenting them as healthy or complete, and Server-side error propagation remains follow-up work. Runtime Public/Admin handlers can also return typed 503/500 errors not yet enumerated in every generated OpenAPI operation; clients must handle generic non-2xx responses.

Visible timestamps are relative, such as `2 minutes ago`, with an accessible absolute UTC value. Agent reports may show `Report #128 - 5 seconds ago`. The WebUI formats Server timestamps but never uses browser time to derive Freshness, liveness, grace periods, Health, or Attention severity.

When the loaded Agents, Nodes, and Networks projections are empty, the page retains authoritative zero/Empty states and adds a compact setup guide: register the expected Network identity, provision and start an Agent, configure its local Node Inventory outside the WebUI, and wait for the first accepted Agent Report. The current Settings link only manages Server-wide History Window/Site Access Mode; it does not edit an Agent's local inventory. The page cannot configure an RPC Endpoint, start an Agent, create a local Node Inventory, expose an Enrollment workflow, enable fake data, or offer one-click initialization.

#### 8.4.6 Page boundaries

Overview owns triage, compact counts, priority subsets, and safe navigation. The Nodes page owns the full inventory, lifecycle/Network/health/freshness filters, stable sorting, inventory revision, identity disposition, redacted RPC Endpoint diagnostics, metadata editing, and Audit links. Admin Node Detail owns full administrative Component diagnostics and must not reproduce Home's observation-card and chart deck.

The Agents page owns the complete Agent inventory, epoch, boot/report state, Node Inventory, credential state, Spool diagnostics, clock diagnostics, sequence gaps, and security events. Agent Detail owns full identity, credential, Host, Inventory, diagnostic evidence, and Audit context. Overview never displays complete credentials, complete RPC Endpoints, Boot IDs, internal paths, stack traces, or remote controls.

#### 8.4.7 Responsive and visual acceptance

The visual balance is PlatPulse's Emerald light system first and Komari-inspired density second: Slate-50-like background, translucent-white surfaces, quiet one-pixel borders, restrained 8-10px radii, no default blur, minimal shadows, high-contrast counts, quiet labels, neutral primary controls, measured Emerald selection accents, and semantic green/amber/red/blue/neutral status treatments. No status depends on color alone. The current production UI remains English; localization is a separate whole-application capability rather than a mixed-language Overview.

At `1280x800`, the Admin sidebar is persistent, Attention and Node Health are full-width, summary cards form four columns, and the Agent inventory summary table spans the full width. At `768x1024`, navigation uses the accessible drawer, summary cards form a two-by-two grid, and Node/Agent content is single-column while the tables retain their table/local-wrapper layout because the priority-card transform begins below 47.9rem. At `360x800` and `390x844`, summary cards remain a compact two-by-two grid when legible and may fall to one column when content requires it; Node, Audit, and Agent inventory tables become priority cards. Health/Freshness and Head/Sync remain paired where those fields exist, secondary evidence moves into expansion, controls remain at least 44x44 CSS pixels, and no primary horizontal page scrolling is allowed. The page remains functional at 200% zoom, in portrait and landscape, and with reduced motion.

Overview acceptance scenarios include:

```text
SCN-OVERVIEW-FRESH
SCN-OVERVIEW-STALE-LAST-GOOD
SCN-OVERVIEW-UNKNOWN-UNSUPPORTED
SCN-OVERVIEW-ATTENTION-GROUPING
SCN-OVERVIEW-PARTIAL-FAILURE
SCN-OVERVIEW-EMPTY-SETUP
SCN-OVERVIEW-RESPONSIVE
```

### 8.5 Compact Admin workbench — current presentation contract (`PAGE-ADMIN-AGENTS`, `PAGE-ADMIN-AGENT-DETAIL`)

**Status:** Current presentation contract, integrated from the accepted `grill-with-docs` review and reconciled with the routed Admin shell and current Agent pages. The listed scenario IDs remain acceptance coverage; this document does not claim that every visual/browser check has been run.

#### 8.5.1 Scope and precedence

The current presentation uses the shared Admin shell and compact Agents summary, with regression coverage across all retained Admin routes and isolation checks for Home. Preserve the Emerald brand, the shared Auto/Light/Dark theme foundation (§11.1), and existing business rules; borrow compact spatial organization, not another product's small text or data model.

- In scope: Admin background, sidebar, header, content origin, heading scale, shrink/overflow boundaries, Agents summary organization, and existing Agent Detail access to secondary evidence.
- The current page boundary excludes Server/API expansion, new client-derived state or severity, global status renaming, a comprehensive restyle of unrelated controls, and restoration of removed routes or features. The integrated §8.6 pass covers the Settings/Audit/Overview/Agent Detail information-architecture changes explicitly; Server extensions without SPA routes remain available-but-unrouted.
- This section governs the shared Admin container and Agents presentation. Settings §8.3 and Overview §8.4 retain their documented internal composition and behavior; the shared shell is consistent across them. Home and Login presentation remain separate contracts.
- Authorization, Public/Admin separation, redaction, last-good semantics, REST authority, query namespaces, SSE invalidation/reset, URL/back-navigation context, and mutation contracts in §§3–7 remain in force. Do not introduce new API operations or optimistic business state.

#### 8.5.2 Shared shell and density

Current authority: ADR 0003 supersedes the initial Emerald implementation’s
centered 1280px Admin column. The shell owns page padding and a shared 13.5rem
sidebar/header-brand partition; pages fill the remaining width. Keep Emerald
BackgroundDecoration, colors, typography and primitives as adopted in ADR 0002;
the historical workbench surface descriptions below do not authorize replacing
that theme or removing its background.

Use an Admin-scoped layout rather than modifying the shared public page container. Desktop content begins approximately 24 CSS pixels after the sidebar; page headings, filters, and primary lists share one left alignment line. Wide lists use the remaining horizontal space, including on ultrawide screens, rather than a uniformly centered, capped Admin wrapper. Settings keeps a reasonable inner form width aligned left beneath its normally aligned page heading.

Suggested geometry is a baseline, not a rigid height constraint or a measurement inferred from screenshots:

| Element | Baseline |
|---|---|
| Desktop sidebar | 208–224 CSS px |
| Header | 48–56 CSS px, without clipping controls |
| Content padding | 24 CSS px desktop; 16 CSS px narrow |
| Page title | 24–28 CSS px |
| Body / primary table text | About 14 CSS px |
| Secondary text | 12–13 CSS px with sufficient contrast |
| Ordinary desktop content controls | About 36 CSS px visual height |
| Header/navigation and primary touch controls | At least 44×44 CSS px hit targets |
| Panel | 16–20 CSS px padding, about 8 CSS px radius |
| Module spacing | 16–24 CSS px |
| Typical rows | About 48 CSS px single-line / 64 CSS px two-line, growing for important content |

Admin uses a stable, undecorated workbench surface in both themes — a Slate-50-like canvas with quiet light panels in Light, and the near-#141923 canvas with quiet dark panels in Dark — without the public gradient/grid crossing its reading surface. Emerald remains a measured accent. Sidebar selection uses a light Emerald background, stronger text, and a thin side marker; keyboard focus remains separately visible, for example through `:focus-visible`. Do not remove outlines without an accessible replacement. The current contract does not require recoloring every existing primary button.

Issue #148 gives the workbench one Admin-scoped surface role per element (shell, header, navigation, panels, tables, forms, and the compact management flows) so Light and Dark resolve from the same theme mechanism without a second Admin theme state, and no Admin route acquires the public grid, chart deck, or Home layout. Loading, empty, failure, disabled, and danger states keep their existing text-plus-semantic-color treatment in both themes; the retained Admin routes, management forms and confirmations, mobile drawer behavior, and the Owner-only boundary are unchanged by the visual slice.

Measure actual container bounds before changing CSS: source inspection found centering and maximum-width rules, but the Admin maximum width need not bind at 1280 CSS pixels. Do not assume every large gap has the same cause. Check combined sidebar reservation, margins, padding, and the correct Flex/Grid shrink boundaries. Apply `min-width: 0` where needed; do not globally break words. Table headers may wrap between words, not split letters. Necessary two-dimensional overflow belongs to the table container, never the page, heading, or action area.

Retain §2.1 vocabulary, including `Current`, with a visible, accurate dimension label. Place global realtime status in a compact shared status area near the header/page context, not an isolated pre-heading row. SSE connectivity does not certify data freshness, Agent liveness, Node health, or credential validity. Do not invent update timestamps or refresh capabilities.

#### 8.5.3 Agents summary and existing detail

The no-new-fields/no-new-actions constraints below describe this earlier presentation-only delivery. §15.1's Add Agent enrollment guidance and Server-backed name/notes editor are now routed (`PAGE-ADMIN-ENROLL`, `PAGE-ADMIN-AGENT-DETAIL`, issue #169), and explicit Agent Removal is routed on the Agent Detail danger zone (`PAGE-ADMIN-AGENT-DETAIL`, issue #171). Do not fabricate fields or actions the Server DTO does not provide.

The Owner scans Agent reporting, inventory, credentials, and diagnostic evidence, then opens the existing `/admin/agents/:agentId` route for investigation. Keep `/admin/agents` as the list route and preserve existing access checks, query/realtime behavior, safe return, and authoritative error handling. Use existing `AgentDiagnostic` data; this is a presentation-only change.

Desktop uses six summary columns:

| Column | Default content and limits |
|---|---|
| Agent | Server-owned display name when set, otherwise the shortened Agent ID, linked to the detail route. Never manufacture a name from Node or diagnostic fields; the stable Agent ID stays reachable and copyable. |
| Reporting status | Existing Server liveness with its dimension explicit; not Node health, browser connectivity, clock reliability, or credential status. |
| Last received | Server `last_received_at`, explicitly labelled as receipt time, with never-received/unknown preserved; not the report sequence. |
| Node Inventory | Total retained Nodes assigned to the Agent. `nodes.length` includes Active and Retired Nodes; do not label this Active, online, or healthy Nodes. |
| Credentials | Server-provided validity and necessary counts, never secrets. Active and explicitly revoked counts need not sum to total: expired credentials can be neither. Do not recalculate validity in the browser. |
| Diagnostics | Separately labelled historical counters and available spool/reporting evidence, not one healthy/failed rollup. |

Do not add a separate View-only action column. The identity link provides detail access. The Diagnostics column keeps its labelled historical counters and a concise evidence brief; complete safe diagnostic evidence stays reachable through an accessible in-row disclosure that opens a separate full-width row below the record (the Nodes inventory pattern), or on the existing detail sections. Full Agent ID must be viewable and copyable through keyboard and touch, not only a hover tooltip. Epoch, full boot/shutdown identifiers, and report sequence belong in the existing detail sections. Retain redaction: “complete” means the existing authorized, redacted evidence, not raw secret-bearing errors or report bodies.

The existing detail separates Identity, Liveness, Boot/report state, Inventory, Credentials, Diagnostics, and Audit. Preserve independent states and evidence reachability. Credential revocation stays there with its explicit warning, Confirm/Cancel flow, busy guard, conflict reload, and authoritative refetch; no destructive quick action is added to the list. No new gap timeline, resolve/reset operation, or recovery workflow is promised.

#### 8.5.4 Diagnostic evidence semantics

§15.3 refines prominence after explicit Owner acknowledgment: acknowledged Agent evidence leaves the prominent warning area, but actual liveness/health and expandable evidence remain available. Being online alone still never dismisses recorded evidence or implies recovery.

- `sequence_gap_count` counts recorded sequence-gap intervals, not missing reports, currently unresolved gaps, or active failures. Label the historical interval count explicitly.
- `security_event_count` is an accumulated recorded-event count without per-event resolution/timestamps in this summary. It does not prove an ongoing incident or credential failure.
- Queue, in-flight, fatal, dropped-range, and delivery-error evidence remain distinguishable. The latest Host snapshot can retain a last error or historical dropped range; that alone does not prove a current failure. Use available observation metadata without inventing per-field recovery/freshness or client severity rules.
- No Host observation, spool not yet observed, and an authoritative zero queued count are different states. Unknown is never converted to zero or normal.
- Keep concise indications of important diagnostic evidence in the summary; move long safe error text and secondary details to the existing detail route. Do not suppress critical evidence because reporting status is `Current`.
- Historical counters remain visibly historical. Do not infer “resolved” from a recent successful report or present stale evidence as a current fault. Multiple important findings may increase row height; a two-line limit must not hide them.

Information hierarchy follows decision relevance: key state and restrictions remain visible; consequences and confirmation requirements stay beside their operation; implementation mechanisms and complete safe evidence move to help/detail. This does not weaken Settings preview/typed confirmation or Site Access Mode confirmation, and Audit stays immutable and read-only.

#### 8.5.5 Narrow-screen behavior

Reuse and improve the existing priority-card transformation rather than rendering duplicate desktop and mobile copies. A phone user must identify the Agent, read its reporting status, and enter detail without horizontal scrolling or hover. Card order is identity/detail link; reporting status and last receipt; important diagnostic evidence or explicit unknown; Node Inventory and credential summary. Full identifiers and long evidence remain accessible through detail.

Choose the table/card breakpoint from available content width, not blind preservation of the current breakpoint. Explicitly test 768 CSS pixels, where the existing UI uses a table and a navigation drawer. Preserve table/card semantics, predictable keyboard order, visible focus, touch targets, and the existing drawer's focus trap, Escape/scrim close, scroll lock, and focus restoration.

#### 8.5.6 Acceptance and implementation checks

The scenario IDs below specify required coverage. The repository's current WebUI lint, typecheck, unit tests, and production build pass; fixed-viewport browser coverage is a separate verification step:

- `SCN-ADMIN-WORKBENCH-LAYOUT`: every retained Admin route has aligned headings/content, no unexplained sidebar-to-content gap, usable remaining width, and no page-level horizontal overflow. Settings retains its left-aligned inner width; Home has no shared-style regression.
- `SCN-AGENTS-PRIORITY-SUMMARY`: six-column summary, accurate retained-Node and credential counts, shortened identity with full-value access, correct receipt-time semantics, and secondary evidence reachable through the summary disclosure and the existing detail. Include an expired-but-not-revoked credential and mixed Active/Retired inventory.
- `SCN-AGENTS-DIAGNOSTIC-EVIDENCE`: distinguish historical gap intervals/security counters from reporting liveness; cover no Host observation, unobserved spool, explicit zero, retained delivery errors/dropped ranges, stale evidence, long safe error strings, and multiple important findings without false normal/current-failure rollups. The summary keeps labelled counters and a concise brief; the complete findings open in a cross-column row below the record.
- `SCN-AGENTS-RESPONSIVE-DETAIL`: identify/status/detail without primary horizontal scroll on phones, tablet table/card usability, accessible full-ID view/copy, and preserved detail privacy and explicit revocation confirmation.

Run coverage at the fixed 360×800, 390×844, 768×1024, and 1280×800 projects; additionally inspect an ultrawide desktop and actual 200% browser zoom/reflow. A viewport-shrinking helper alone is not evidence of actual browser zoom verification. Check portrait/landscape, keyboard and touch, contrast (at least 4.5:1 ordinary text and 3:1 large text), focus, and reduced motion. Use long identifiers, long errors, populated, empty, Starting, Error, Stale, Unknown, and last-good cases. Clipping or an overflow helper passing is not proof of readable columns or reachable controls.

Existing Agent summary tests assert the priority summary and retained detail evidence; preserve that coverage when changing the UI. Preserve independent-state, authorization, redaction, and confirmation coverage. Shared shell tests cover every retained route, not only Agents. Verify the generated operation/DTO references and current query/reset wiring when changing the implementation rather than adding new API behavior.

**Current verification boundary:** the repository checks cover the routed SPA contract and pass in the current tree; this documentation update does not claim a fresh Playwright run at every fixed viewport or a production deployment smoke test. Server/API extensions without SPA routes, legacy `visibility` compatibility fields, and redacted diagnostic limitations remain intentional boundaries. Do not restore removed routes or features merely to satisfy historical drafts or external tests.

### 8.6 Admin information architecture pass (`PAGE-ACCESS-AUDIT`, `PAGE-ADMIN-SETTINGS`, `PAGE-ADMIN-AGENT-DETAIL`, `PAGE-ADMIN-OVERVIEW`, `PAGE-ADMIN-NETWORKS`, `PAGE-ADMIN-NODES`)

**Status:** Integrated current information architecture accepted by explicit product direction after the §8.5 shell review. Settings/Audit restructuring and Overview module ordering below are part of the current routed contract. The shared Admin shell, authorization, REST/cache/SSE, redaction, confirmation, last-good, and mutation contracts are unchanged.

- `PAGE-ACCESS-AUDIT` renders the immutable redacted events as one compact table with Time, Event, Actor, Target, and Details columns. Redacted details stay collapsed behind an accessible `Show details` disclosure (`aria-expanded`/`aria-controls`, per-event region label) and open in an independent full-width row below the record (the Nodes inventory pattern), never inside the action cell; the listing, Server-side filters, cursor pagination, and append-on-load-older behavior are unchanged. Event-kind and Target filters size to their content instead of spanning the row, and the table becomes priority cards below the responsive breakpoint rather than a clipped or shrunk table.
- `PAGE-ADMIN-SETTINGS` keeps one left-aligned constrained surface with two independent modules (History Window, then Site Access Mode). Current value, default, and the allowed range are grouped; the numeric input keeps its label, bounds, and 44px target at a natural width; action buttons keep normal width. All risk copy, the Server-authoritative impact preview, explicit `{confirmed: bool}` confirmation, CSRF, error isolation, and submit behavior are unchanged; any typed phrase is client-side friction, not a Server security boundary.
- `PAGE-ADMIN-AGENT-DETAIL` opens with a key summary (shortened ID with copy control, Server liveness, boot status, credential counts, Epoch, receipt time, report sequence, declared Node count, and classified important warnings) followed by Overview, Runtime and reporting, Credentials, Diagnostics, and Audit categories. Every pre-existing field remains reachable; summary warnings distinguish current state, recorded history, and unknown values and never replace Server liveness, health, freshness, or attention policy.
- `PAGE-ADMIN-OVERVIEW` Agent inventory is a compact summary table (Agent, Reporting, Last received, Host resources, Evidence, Nodes) instead of large half-width detail cards. It keeps the six-Agent priority limit, the `Showing N of M Agents` line, `View all Agents`, independent query failure, and raw Server values; per-Node identity/health/freshness detail stays on the Nodes page and Agent Detail.
- Attention items label their own Server-supplied observation time ("Last observed …"); when the Server reports no observation timestamp the item reads `Observation time unknown`. The snapshot `generated_at` is labelled only as `Last good snapshot` and is never presented as an event or observation time.
- `PAGE-ADMIN-NETWORKS` renders zero reported mismatches as plain `No mismatch reported` text, never as `Current` or as verified identity; freshness, identity state, and mismatch count remain separate dimensions. Network detail and the Nodes inventory share one column order (identity-relevant fields before lifecycle and head) and one focus-visible treatment for sort buttons, row toggles, and sidebar selection.
- Shared formatting: CPU uses `formatPercent`, memory and byte rates use the binary byte units (`formatBytes`/`formatBytesUnknown`, `formatBytesPerSecond`), timestamps use the existing UTC `formatObservedAt`, and stable IDs use `formatIdentifier` with the complete value kept in the DOM and title. Unknown values remain `Unknown` rather than `0`, `false`, or a blank cell.

## 9. Content, privacy, and redaction

The WebUI must not display or store in browser state:

- Agent credentials or any one-time provisioning material;
- session tokens or CSRF values in URLs;
- passwords, TLS private keys, or pepper values;
- raw Peer addresses or identity lists. Peer Snapshots and Presence exist behind Server projections, but Public/Admin WebUI surfaces show only redacted current/aggregate data;
- complete RPC Endpoints, complete request bodies, stack traces, or internal paths.

Errors name the failed user task and next safe action. They do not expose stack traces, secret contents, or internal paths. Confirmation copy states what will and will not change — in particular for the history window: shortening deletes data; lengthening cannot recover it.

Relative times can expand to absolute UTC or selected timezone. Server timestamps remain authoritative.

## 10. Responsive and accessibility contract

### 10.1 Navigation

- desktop: persistent Admin sidebar and context;
- tablet: collapsible navigation with context preserved;
- mobile: accessible drawer or equivalent navigation;
- drawer opening moves focus inside, traps Tab focus, closes on Escape, restores focus to opener, and locks body scroll;
- no critical action depends on hover;
- the theme control's accessible name states the current mode and the next action, and it is at least a 44×44 target in every shell;
- browser back/forward preserves URL filters and detail context;
- Admin page-group links carry one leading decorative glyph that inherits the label colour. The glyph is `aria-hidden`, so the link's accessible name remains the visible page-group label; it never carries status meaning (§10.3) and never replaces the label. The retained MVP mapping is:

| Page group | Glyph | Code point |
|---|---|---|
| Overview | ▦ | U+25A6 |
| Agents | ◈ | U+25C8 |
| Nodes | ◉ | U+25C9 |
| Networks | ⬡ | U+2B21 |
| Settings | ⚙ | U+2699 |
| Sessions | ◫ | U+25EB |
| Audit | ☷ | U+2637 |

### 10.2 Layout

- forms are single-column at narrow widths;
- 44×44 CSS pixel target is the baseline for touch controls;
- tables become priority cards/rows on narrow screens;
- critical fields remain visible: status, subject/Node, head/sync, freshness, primary reason, next action;
- detail/evidence uses expansion or a dedicated route;
- sticky action areas never cover errors, fields, or keyboard focus;
- 200% zoom, landscape, and portrait remain functional.

### 10.3 Semantics

- one logical `h1` per page and ordered headings;
- semantic `form`, `label`, `table`, and `caption` where applicable;
- visible focus on keyboard navigation;
- status has text plus icon/equivalent, never color only;
- validation has field-level messages and page summary;
- live regions are polite and limited to meaningful transitions;
- reduced motion removes non-essential animation;
- touch tooltips have a non-hover alternative.

## 11. Mock and prototype contract

The accepted shell prototype is a visual/state primary source, not a production dependency:

```text
prototype/ui-shell-variants  (see the change log in §14)
```

A production mock adapter, if needed for component tests or development, must match the typed API adapter:

```text
mock operation → response DTO → error cases → invalidation → expected refetch → scenario ID
```

Scenario IDs:

```text
SCN-ADMIN-WORKBENCH-LAYOUT
SCN-AGENTS-PRIORITY-SUMMARY
SCN-AGENTS-DIAGNOSTIC-EVIDENCE
SCN-AGENTS-RESPONSIVE-DETAIL
SCN-AUTH-OWNER-LOGIN
SCN-AUTH-SESSION-REVOKED
SCN-SETTINGS-ROUTE
SCN-SITE-ACCESS-PRIVATE
SCN-HOME-NETWORK-LIST
SCN-HOME-NODE-DETAIL
SCN-HOME-UNAVAILABLE-NODE
SCN-OVERVIEW-FRESH
SCN-OVERVIEW-STALE-LAST-GOOD
SCN-OVERVIEW-UNKNOWN-UNSUPPORTED
SCN-OVERVIEW-ATTENTION-GROUPING
SCN-OVERVIEW-PARTIAL-FAILURE
SCN-OVERVIEW-EMPTY-SETUP
SCN-OVERVIEW-RESPONSIVE
SCN-SITE-ACCESS-PUBLIC
SCN-HISTORY-WINDOW-SHORTEN
SCN-HISTORY-WINDOW-BOUNDS
```

Scenario state is memory-only. No credentials, secrets, production API origins, local persistence, or prototype-only branches are allowed in production pages.

## 11.1 Accepted Home and Node Detail visual contract (Issues #75, #97, and #151)

The accepted direction from Issue #75 and the compact Home contract from Issue #97 remain the production structural baseline for the public Home surface. The visual layer adopts the supplied Emerald reference and the inspected Tokinx/komari-theme-emerald source at commit c2c5e88ea19c7cbe18d14a50414e10deca3cc66e, without importing its server, pricing, or remote-control data model.

### Visual language

- The shared theme foundation offers Auto, Light, and Dark (see "Theme behavior" below). Light uses a Slate-50-like canvas; Dark uses a coordinated near-#141923 slate canvas with the same restrained Emerald/Lime top atmosphere. Home, public Node Detail, and Login carry one shared top atmosphere: the Emerald default background at `c2c5e88ea19c7cbe18d14a50414e10deca3cc66e` ported faithfully in both themes — the full-viewport neutral base, the emerald/lime atmosphere with its `farthest-side` top radial mask at `opacity: 0.4`, and the low-contrast inclined SVG grid with the upstream `black/40` fill, `black/50` stroke and `mix-blend-overlay` treatment over four local filled cells, plus the Dark variants (`emerald/30`-`lime/30`, `white/2.5` fill, `white/5` stroke, vertical mask). The one deliberate deviation is width: the atmosphere spans the full viewport instead of the upstream fixed 1300px, so it never reads as a partial band on a wide viewport. The layer is decorative, pointer-inert, absent from the accessibility tree, shared rather than per-page, and fades before the lower reading surface. Admin retains the Emerald brand but uses the stable, undecorated workbench background in both themes as specified in §8.5; this exception does not change Home or Login.
- Public display cards follow Emerald default surfaces at `c2c5e88ea19c7cbe18d14a50414e10deca3cc66e`: borderless white in Light and `oklch(0.141 0.005 285.823)` in Dark, both at 60% opacity at rest. Home display cards retain their existing opaque-on-hover behavior on hover-capable devices; static Node Detail summary/observation/Linked Validator/chart/disclosure surfaces keep their resting opacity on hover. Home Node cards explicitly opt into interactive-card feedback and alone gain the emerald-600/10 glow and 2px lift; statistics and detail/chart/disclosure cards have no decorative shadow or lift. This does not change the page canvas, atmosphere, layout, internal separators, or semantic status colors. No custom background image or default `backdrop-filter` is introduced. Admin, Login, menus, forms, dialogs, and overlays retain their existing surfaces.
- Public Home and Node Detail use Emerald’s system font stack (`system-ui, -apple-system, "Segoe UI", Roboto, sans-serif`), including navigation and controls. Card typography follows the corresponding Emerald heading, metric, and responsive detail tiers while retaining tabular numerals and overflow safeguards. No font binaries are copied or downloaded; Admin and Login retain Inter.
- Primary numbers and page titles use high contrast and strong weight. Secondary labels, timestamps, identifiers, and explanatory copy are quieter without being reduced below readable body sizes. Important values use tabular numerals where appropriate.
- Emerald is a measured brand accent and selection cue, not a replacement for every primary action. Neutral primary controls, blue chart series, amber warnings, red destructive/error states, and neutral Unknown/Unsupported states remain distinct.
- Cards, pills, separators, fine 4px progress tracks, and focus states share one spacing and radius system. Clickable cards may use a 150–200ms hover lift of about 2px and a faint Emerald shadow only on hover-capable devices; static containers do not imply interactivity. Reduced motion removes the lift and non-essential transitions.
- The visual contract does not authorize fields that are absent from the Public Projection. Node Detail may show the monitored PlatON process CPU/memory, process start/uptime, last Agent report time, Node Data usage/capacity, and sampled Host network receive/transmit rates supplied by the Server; it does not add pricing, raw Peer identity, Host identity, or RPC Endpoint text.
- The default geometry is adapted from the Emerald repository's inline `Background.vue` SVG under its MIT license. Keep the copyright and license notice with the adaptation; do not copy Komari branding, logo assets, external flags, fonts, or theme-management behavior.

### Home composition (`PAGE-HOME-NETWORKS`)

The root dashboard flattens the returned Active Node projection. It remains read-only and never reorganizes the view around Agent or Host topology.

**Root `/` (`PAGE-HOME-NETWORKS`):**

1. A compact header with the PlatPulse brand link at left and one Owner-only Admin link at right that pairs a gear icon with visible `Admin` text. The brand returns to Home; the Admin link enters the Admin Overview route and does not expose Admin data inside Home.
2. A page heading, Public Projection copy, and a Server-authoritative live/realtime indicator.
3. A compact six-card overview beside a transparent Peer country map. Reading/DOM order is Active Nodes, Healthy Nodes, Cumulative blocks, Attention, Networks, Cumulative rewards. The original four statistics retain their implemented Network-filter scope: Active counts selected Active Nodes, Healthy counts their Server-owned Healthy state, Attention counts all remaining selected Nodes including Unknown, and Networks counts selected Public Network groups, not the entire Network Registry. Sorting only changes Node order. The two cumulative cards use the approved Cross-Network Validator Overview described below.
4. Network filter pills and a labelled sort control (`Health`, `Name`, or `Current Head`).
5. Compact Active Node cards use natural heights without stretching shorter cards to a neighbour. A stretched semantic Node-name link opens Node Detail; copy/Details controls remain independent buttons, never nested in the link. The two-state Server-owned Node Health marker stays before the Node name. The header Node Role expresses consensus membership on one line (`Node: Non-validator` when space allows), separate from Linked Validator staking validity and Provider data freshness and never confused with it. The Network name and process uptime share a compact secondary line directly under the name (about 6–8px, with about 12px before the resource block); identity details remain keyboard/touch-accessible. Healthy Nodes omit routine prose; exceptional Nodes may show one short diagnostic line.
6. Card data ownership is explicit: PlatON process CPU/Memory, Node data size/capacity, shared Host transfer rates, Node Head/Txs/Peers, and Consensus QC/Locked/Committed/membership. Ordinary parameters use a compact left-label/right-value key-value line with no fixed two-line label height, no per-row frame and no dotted leader; only the few cumulative metrics keep the emphasized label-over-value form. Resource rows retain their progress and used/capacity explanations; on a wide card the Node data used / total caption joins the title line and a narrow card keeps it as its own caption. PlatPulse owns no occupancy percentage threshold for these bars, so the ordinary fill is the theme's informational colour rather than the default primary (white in the dark theme); no success/warning/error condition is invented and a Server-owned rule can replace it. Head/QC/Locked/Committed pair into two columns when the card itself is wide enough, and fall back to one full-width key-value line each on a narrow card; Txs/Peers keep their pair with a separate full-width fallback for unusually large counts. The width switch is a CSS container query on the card, not a viewport breakpoint or a JavaScript size listener. Linked Validator metrics show two emphasized cumulative cells (Cumulative blocks, Cumulative rewards) plus four directly visible compact parameters (Network rank, Production rate, PlatScan 24h rate, Delegation reward share) that pair into two columns on a wide card and stay four full-width lines on a narrow one; each short display name keeps its full field name as an accessible name and in Details, and only the all-missing/currently-confirmed-Not-Validator exception folds them per the [Validator presentation contract](validator-metrics.md#accepted-node-presentation-target-and-edge-cases). A readable identity fingerprint, a two-line identity/data/identifier/action area, full-identifier copy, and accessible Details retain exact values/provenance/reasons. Exceptionally long full-number values stay on one line and fall the whole group back to full-width rows instead of shrinking, clipping or splitting digits. Missing values remain explicitly Unknown rather than zero; historical values, real zero, Unranked, Not applicable and independent stale/error dimensions must survive compaction.
7. The dashboard can show `No Active Nodes in this view` when the loaded projection/filter has no cards. This is distinct from a registered Network that the current Server endpoint omits or returns as `404` because it has no Active Node.

**Home overview and Peer country map (Issue #133):**

1. The overview keeps two non-overlapping tracks at desktop widths >=1024px, about 4:3 for statistics and map, within the unchanged 1280px page cap. That ratio keeps each of the six tiles at the width the earlier four-card grid used; a wider statistics track is not required. The left has six equal, compact summary cards in three columns by two rows, each a title plus marker (or the Breakdown control) over a value with no footer row; the right map has a contained 2:1 aspect ratio and must not crop, stretch, or overflow behind summaries, controls, or the shell header. There is no legacy fixed 232px band or compensating empty gap. Below 1024px, statistics precede the proportional map, using three columns from 640px and two below it. All widths retain the same six-item reading order. The Node grid below keeps its independent existing auto-fill/minimum-300px column rules.
2. All six statistics follow the Home Network filter; sort does not alter totals. Cumulative blocks and rewards are exact numerical sums of already-deduplicated Server Network summaries, not sums of repeated Node metrics. The human-approved Cross-Network Validator Overview explicitly supersedes the previous no-sum display rule while leaving Server membership, Network-scoped identity and per-Network summaries unchanged. Rewards add native-unit numbers without conversion; the DTO does not prove unit identity, so this is not a single-asset balance or monetary valuation. Each cumulative card shows its own scope, independent known/eligible and stale coverage and a Partial marker directly under the value, plus an accessible Breakdown dialog containing exact sums, per-Network blocks/rewards/coverage, partial/Unknown explanations, and separate unlinked Nodes. Missing Network summaries cannot become zero or complete coverage. This replaces the standalone Validator totals region; an effective Link to an exited/historical Validator still contributes available lifetime metrics.
3. A normal map shows only one line, “Peer countries · n records”, two icon controls (Map information and Show full map / Collapse map), the world map, and a short necessary credit. The title count is the existing in-scope Peer-record denominator, not monitored Nodes or unique Peers. Unavailable values use a dash, never an invented zero. There are no routine Current badges, scope/resource/freshness rows, country chips, or standalone expansion-button row.
4. Map information opens by click, keyboard, or touch as a lightweight non-modal disclosure. It contains current scope, Known/non-zero Unknown counts, Server-provided unknown reasons, per-Node/not-deduplicated-by-IP basis, independent Geo/Peer observation/map-resource states, last-good/age/stale/error details, partial and Network-basis notes, and the shared Network-filter explanation. Escape, close, and outside pointer dismissal remain available, with appropriate focus restoration.
5. Every country with a count remains accessible in the disclosed country list by ISO code and country name, including missing representative-point/outline explanations. Hovering or tapping a highlighted country or its marker shows name/count; markers also support keyboard activation. No marker, random point, or [0, 0] fallback is fabricated. Unknown locations are separate from known countries whose quantity cannot be plotted. A missing outline alone does not warrant a missing-location warning when a valid quantity marker exists.
6. The basemap remains the fixed, locally hosted Natural Earth 1:110m Admin 0 Countries asset (public domain), projected in its own equirectangular space. No runtime map CDN, tiles, geolocation, or new data source is introduced. Full original basemap and Server-owned Geo attribution, source labels, and provider links stay accessible under Map credits. Only short source labels drawn from the supplied attribution are shown in the map's low-interference corner; these do not replace or rewrite the full authoritative attribution in details.
7. The default map shares the existing light-green Home wash: no white card shell, prominent border, shadow, or large title bar. Outlines remain light grey, observed countries take the existing restrained Emerald fill, and count labels retain contrast. Only the on-demand information disclosure has its own light surface. The one standing figure in the map's corner is the reference theme's pointer-inert chip: a pulsing dot with the total number of Peers the in-scope Active Nodes are linked to. It is the Server's own Peer-record denominator for that scope — the same records the country fills and markers are drawn from, counted per Node and never deduplicated by IP — so it can never disagree with the map beneath it, and a scope without a successful Peer Snapshot shows no figure rather than an invented zero.
8. Exceptional states add at most one compact notice, for example Data stale, Some locations not shown, or Map unavailable with Retry map. Non-zero unknown locations may share that notice without being counted as known unplottable countries; Unknown 0 is omitted. Loading, no observations, authoritative empty data, unavailable data, and last-good data remain distinct. Errors/staleness must not be masked by an authoritative retained zero. Geo Disabled remains a neutral local notice without loading geometry, and render failures remain contained by the existing map boundary.
9. Narrow layouts stack statistics above the complete world map and use its natural aspect ratio rather than a fixed-height blank canvas. The existing expanded/collapsed control keeps aria-label, tooltip, aria-expanded, aria-controls and 44px targets. Expansion increases actual available map width (using the Home gutters on narrow screens and the full overview on desktop), never shrinks a tablet map or merely adds vertical whitespace. SVG layout responds to container changes without a chart-library resize lifecycle. Neither state causes page-level horizontal scrolling.
10. No default wheel zoom, 3D, or continuous animation is added. Decorative background layers remain pointer-inert. Server aggregation, Network filtering, realtime updates, last-good semantics, error handling, and backend interfaces are unchanged; the separately approved cross-Network overview and compact Node presentation change display only.

### Node Detail composition (PAGE-HOME-NODE)

The user-approved refinement below is the current Node Detail presentation target, not a claim of implementation or test completion. It preserves the 1280px shell, Public Projection, independent state semantics and existing six-chart data contract. ADR 0006 remains the historical Home Node-card Activity decision; this scope explicitly extends its existing badge semantics to Node Detail without rewriting that decision or changing other pages.

1. The page identity remains an uncarded heading block, with the Home `All Networks` back context, the two-state Server-owned Node Health marker before the display name, and the exceptional health reason — no bordered hero, coloured edge strip or observation wrapper. Routine Healthy prose is omitted. The title area shows Node Health plus Activity only. The Linked Validator overview header independently presents the `Validator` badge (Current Validator Status / valid staking identity), the `Current` cue (Provider freshness/state), and `Activity` (Validator Provider activity). None is inferred from either of the others or from Node Health; consensus membership remains in Chain info. Reuse the existing Activity badge semantics and accessible source/time/reason explanation in both the title area and Linked Validator overview, including Observing for unavailable evidence with its actual reason; preserve canonical last-good activity and stale annotation rather than inventing a new mapping.
2. Keep four summary cards: current `Head`, Sync, Peer Count and PlatON process uptime; four columns at `lg` (1024px) and above, two below. Head adds a signed `Head delta = Node Head - Observed Network Head`: negative means behind, positive ahead, zero equal. Compute it only with a current valid Node Head and a current valid, high-confidence Server Observed Network Head for the same Network. Missing, invalid, stale or low-confidence input renders a dash plus the specific reason, never zero or a last-good delta. The existing Public Node `freshness` field is an oldest-receipt timestamp, not a freshness enum. Without an API expansion, the comparison conservatively requires a valid receipt timestamp, `rpcState=ok` and Server `health=healthy` (which confirms current RPC/sync/consensus). Other health outcomes do not prove Head currency independently, so omit the delta with an explicit reason while retaining absolute observations; never infer a new frontend freshness threshold or change Activity. Sync progress likewise requires the high-confidence Server reference; it is not redefined by this display delta. Each card keeps its own honest Unknown/last-good explanation.
3. Keep the three observation panels: chain/consensus; PlatON process resources and Node Data; shared Host resources collected once per Agent. Preserve current fields, valid percentage tracks, and the distinction between process memory/uptime and Host memory/uptime, and between Node Data and whole-Host storage. Panels remain side by side at `lg` (1024px) and above and stack below `lg`. `QC`, `Locked` and `Committed` show their absolute heights plus a signed delta `height - current Node Head` only when both inputs are current and valid. Stale last-good consensus heights keep their annotated absolute value but no delta; unavailable/invalid/stale comparison inputs show a dash with a reason, never a fabricated zero.
4. The Linked Validator overview exposes all six metrics: Validator Lifetime Block Count, Validator Lifetime Rewards, Validator Rank, Validator Block Production Completion Rate, PlatScan 24-Hour Block Production Rate and Effective Delegation Reward Distribution Ratio. Use two shrinkable columns below `lg`, three at `lg` and above; preserve reading order, complete integer counts and all available source reward precision without abbreviating, rounding through binary floating point or reconstructing absent digits. Keep existing percentage formatting and source/not-applicable explanations. Ordinary noncompact metric captions wrap naturally rather than ellipsizing or reserving a compact one-line slot; long exact values remain readable without clipping or page overflow. Preserve genuine zero, Unknown, Unranked, not-applicable and last-good states independently.
5. The Node ID keeps its existing full UUID display. Only the linked Validator identifier is abbreviated in its overview position, with an explicit copy-full-identifier control and a keyboard/touch-operable full-identifier reveal (not hover-only). Copy always uses the complete identifier. Failure announces the failure and reveals the complete selectable text for manual copying, never a false success; revealed long identifiers wrap. Preserve accessible control names, visible focus and 44×44 targets.
6. The visible Last report line shows relative age, updating once per second while the page is visible, alongside a faint but readable exact UTC timestamp. Missing or invalid timestamps show Unknown; future timestamps retain their valid exact UTC value with an explicit future/clock warning, not a clamped `0s ago`. The display-only clock is isolated to this age presentation: pause it when hidden and recompute on visibility return; it must not drive query refetches, chart windows, Node Health, component freshness, Validator freshness or any other domain state, nor announce every tick to assistive technology.
7. The page reads continuously without Details/Network tabs. Peer diagnostics and low-frequency technical/Validator diagnostics use native `details`/`summary`, default collapsed, with keyboard/touch operation and visible focus. Component states, Provider states and detailed provenance (source, successful update time, source cutoff when available and unavailable/degraded reasons) remain available inside the relevant disclosure. Important independent errors, exceptional health reasons and relevant stale/degraded warnings remain visible beside the affected summary/panel/Linked Validator overview while collapsed; hiding diagnostics must not hide a Provider failure, rank failure, identity problem or unrelated collection error. Only authorized Public Projection evidence is exposed, never Peer addresses or a Peer identity list.
8. Preserve the real latest-60-second metrics window (`from`, `to`, `windowSeconds`) and the same six charts in order: process CPU %, process memory %, shared Host upload/download rates, Peer inbound/outbound count, block interval, transactions per block. The first four are line charts, the last two bar charts; retain current values, units, readable series labels/legends, blue/cyan plus text, and `60s`/`0s` bounds. The grid remains one column below `md` (768px), two columns from `md`, and three by two from `xl` (1280px); chart-height changes at `lg` do not change these grid breakpoints. Set both each plot container and its SVG height to `7.25rem` below `lg` and `6.25rem` at `lg` and above; changing only a wrapper must not leave an oversized SVG.
9. A line chart may use the Server's one last-good point just before the window; bars use only in-window observations. Never invent intermediate samples or substitute zero for unavailable data. Failures remain per-series; retained current values and unrelated charts remain visible. No Bounded Block History, History Gaps, public Validator analytics or history export is added. The existing two-summary history request only supplies the current block-interval label; missing/non-consecutive summaries mean Unknown.
10. Node Detail summary cards, observation panels, Linked Validator overview, chart cards and diagnostic surfaces are static: retain their resting Emerald surface opacity with no opacity change on hover, decorative glow or lift. Disclosure triggers, links, copy/reveal controls and other actual controls keep interactive feedback and visible focus. Home Node cards use an explicit interactive-card treatment and retain their existing whole-card navigation/hover/focus behavior; do not apply the Node Detail change globally to Home statistics, Admin, Login or other pages.

The dashboard presents independent observation dimensions. One failed collection must not hide or rewrite another dimension, and one Agent's Nodes must never be merged into an Agent-level chain view.

### Responsive acceptance baseline

The fixed acceptance viewports are 360x800, 390x844, 768x1024, 1280x800, and 1440x900. Long state, attribution, and identity text wraps without horizontal overflow, and keyboard focus and touch targets remain usable in every project.

- At 1280x800 and wider desktop viewports, Home retains its 1280px page cap, places the six statistics (three columns by two rows) beside the contained 2:1 map in about 4:3 tracks, which keeps each tile at the original four-card width, and fits Node columns using the existing auto-fill/minimum-300px rule; the overview does not redefine Node-card breakpoints or widen the page. Node cards have natural independent heights. Node Detail retains its four summary tiles, three observation panels and three-by-two chart deck, with three Linked Validator metric columns and 6.25rem plot/SVG heights; its shell remains capped at 1280px at both 1280 and 1440 widths.
- At 768x1024, Home puts its six statistics in three columns by two rows above the proportional map and keeps the existing two-column Node grid. Node Detail’s four summary tiles fall to two columns, its three observation panels stack in reading order, and its six equal-height chart cards fall to two columns. Linked Validator metrics remain two columns and plot/SVG heights are 7.25rem.
- At 360x800 and 390x844, Home puts the six statistics in two columns by three rows above the map, preserving DOM order, and uses a single-column Node grid. Chain/consensus and the four ordinary Linked Validator parameters become full-width compact key-value lines while the two Linked Validator cumulative cells keep their two-column emphasis and Txs/Peers stay paired; rare oversized full numerals use full-width lines without clipping or reduced type. Filter pills scroll within their control, not the page. Node Detail retains its two-column summary block and Linked Validator metrics, single-column observation panels and six-chart stack with 7.25rem plot/SVG heights and default-collapsed touch-operable disclosures. The Admin table-to-priority-card transform still begins below 47.9rem; Admin tables therefore retain their local-wrapper behavior at the 768 fixture.
- At every viewport, long Node names, Node IDs, Network keys, status reasons, and values wrap or truncate with an accessible full value. No critical state requires primary horizontal page scrolling.
- Touch targets are at least 44x44 CSS pixels. Portrait, landscape, 200% zoom, and reduced-motion settings remain usable.

Node Detail refinement acceptance (required checks, not results already run) covers all five viewports in Light/Dark, keyboard/touch, 200% zoom and reduced motion:

- Verify all six exact Linked Validator metrics, two columns at 360/390/768 and three at 1280/1440, wrapping noncompact captions, long IDs/values and full-precision rewards without clipping. Check independent Validator (valid staking)/Current (Provider freshness/state)/Activity in the Linked Validator header, Health plus Activity only in the title, and consensus membership in Chain info; preserve existing Activity semantics in both placements.
- Verify native diagnostics start collapsed and preserve complete public states/provenance when opened; independently fail Provider detail, rank, Peer and component data and confirm relevant errors/stale reasons remain visible while unrelated values survive.
- Keep the existing full Node UUID display; exercise full Validator ID reveal and full copy with keyboard and touch, including clipboard rejection/unavailability and selectable complete fallback text.
- Check Head deltas behind/equal/ahead and missing/invalid/stale/low-confidence references; check absolute QC/Locked/Committed and signed deltas against current Node Head, with no stale-input delta.
- Check Last report ticks every visible-page second without refetching or changing domain freshness/chart data, hidden-page pause/resume, faint readable exact UTC, missing/invalid Unknown and future-time warning; no per-second live announcement.
- Measure both plot and SVG heights (7.25rem at 360/390/768, 6.25rem at 1280/1440), unchanged six-chart order/grid and retained 60-second samples. Verify static Node Detail hover does not change opacity, and interactive controls plus Home Node-card feedback remain intact; other pages and the 1280px shell are unchanged.

### State and realtime acceptance

The UI keeps collection state, freshness state, value state, and authorization state independent. It renders the fixed user-facing vocabulary from this document: Starting, Current, Stale, Error, Unknown, Disabled, Unsupported, Empty, Connecting to live updates, Live updates paused, and You are offline.

- Initial route loads show a meaningful Starting/loading state and do not fabricate values.
- A successful observation may show Current or an authoritative empty value. A successful Peer Snapshot/aggregate of zero is displayed as zero, not Unknown; omitted/unsupported peer collection remains distinct.
- An Error or Stale observation may retain LastGood data, but the UI must show the error/stale reason and age/freshness supplied by the Server. It must never convert Unknown, stale, never-observed, Disabled, or Unsupported into 0, false, or Healthy.
- Node, history, metric-history, peer-history, and validator requests fail independently. A failed optional module does not erase the Node summary or unrelated successful modules.
- A normal SSE invalidation preserves the currently displayed Node and view context while the exact Public resource is refetched. A reset, authorization transition, Node ID change, or access recheck clears affected sensitive projection state before the next render and may show a revalidation state.
- SSE carries invalidation/reset signals only. REST remains authoritative for all displayed business values. A disconnected stream announces Live updates paused; an open stream announces nothing, because the positive transport state is silent; browser-offline state may additionally announce You are offline.
- Retired, deleted, forbidden, or unknown public Nodes use non-leaking unavailable copy and never reveal whether a protected record exists.

### Navigation and accessibility acceptance

- The PlatPulse brand is a keyboard-focusable link to `/`. Its accessible name identifies PlatPulse and its destination is stable from Home and Node Detail.
- The Admin link is a keyboard-focusable link to /admin with a gear icon, visible `Admin` text, and an explicit accessible name. Home does not show other text navigation or a Home logout action in this header.
- Whole-card Node links, the Node Detail `All Networks` breadcrumb link, and the Node Detail disclosures are reachable by keyboard in a predictable order. Browser back/forward preserves route context.
- The public observation panels are static labelled containers, not tabs. Node Detail retains native details/summary disclosures; Home adds explicit Breakdown/identity/Validator Details dialogs with labelled triggers, keyboard/touch operation, visible focus, Escape close and focus restoration, independently of the stretched Node link.
- Pages expose one logical h1, ordered headings, semantic lists/tables where appropriate, meaningful empty/error regions, and polite live regions only for meaningful transitions.
- Status uses text plus icon, shape, or an equivalent explanation. Focus rings remain visible against the light canvas and composed surfaces. Reduced motion removes non-essential transitions and does not remove state information.

### Theme behavior (Issues #146–#148)

The production theme lifecycle is a shared foundation, not a per-page option. It applies to public Home, the public Node Detail, Login, Admin direct entry, and reloads. Issue #146 delivered the lifecycle, the shared foundation, and the Login adaptation; Issue #147 completed the public Home and Node Detail adaptation as a visual-only slice; Issue #148 completed the Admin workbench and management-flow adaptation on the same foundation. Admin keeps its quiet undecorated workbench background and never adopts the public grid.

- **Three modes.** One control cycles Auto → Light → Dark → Auto. Its accessible name states the current choice and the action it performs (for example, "Theme: Auto. Switch to Light"), it is a 44×44 CSS-pixel target, and it keeps visible keyboard focus. Auto is the default when no valid preference exists.
- **Auto follows the system live.** A change to the operating-system preference takes effect immediately while Auto is selected. An explicit Light or Dark choice is never overridden by a system change.
- **Persistence is preference-only.** The selection is stored under a production-owned key (`platpulse.themeMode`) and is isolated from the throwaway prototype key. A missing or invalid value falls back to Auto; unavailable storage must not prevent in-session switching. The preference survives reload and cross-route navigation.
- **First paint is pre-mount.** The resolved theme, canvas, and browser `color-scheme` are applied by a synchronous same-origin head script (`public/theme-init.js`) before the application module runs, and reconciled when React mounts. It is external rather than inline because the Server enforces `script-src 'self'`. This must hold on direct entry and reload of Home, Login, and Admin, and must not depend on a post-mount correction.
- **Shared surfaces.** Dark mode overrides the shared shell tokens and the shared shell, cards, forms, tables, navigation, and Login surface; semantic warning/error/success/unknown styling stays distinct rather than inverted, and color alone never carries status.
- **Login.** Login keeps the fixed Emerald reference at commit `c2c5e88ea19c7cbe18d14a50414e10deca3cc66e` and the accepted A calibration: coordinated light/dark neutrals, the near-#141923 dark canvas, and the actual SVG grid geometry with layered masks rather than an approximate CSS line gradient, including the full-bleed width deviation defined under "Visual language" above. The form, loading, invalid-input, failure, and success-redirect states remain readable and operable in both themes, reusing the existing login API without changing Session, permission, Site Access Mode, or redirect rules.
- **Historical Public Home and Node Detail delivery (Issue #147).** This paragraph records that visual migration; the current Home composition and filter scope above supersede its four-tile layout. The two public surfaces complete the shared foundation without changing routes or information structure. The Home 2x2 statistics, the Network filter/sort controls, the Peer country map scope/denominator/states, the Server-owned Node Health Summary (Healthy green, every other state grey, with the abnormal reason in text), the last-good semantics, and the Node Detail grouped info and six-chart deck behave exactly as before; only the visual adaptation changes.
  - Information cards use the exact default Emerald surface and typography contract under "Visual language" above. Home Node cards alone show the 20px glow plus 1px shadow ring in emerald-600 at 10% and a 2px lift, with 200ms transitions. Home statistics retain their opaque-on-hover behavior without glow or lift. The current Node Detail refinement supersedes the historical blanket opacity-hover rule: its static summary/observation/Linked Validator panels, charts and diagnostic surfaces keep their resting opacity on hover, without glow or lift. Static cards gain no click handler, pointer cursor, or tab stop; reduced motion removes non-essential transitions and the lift. Whole-card Home Node links retain focus highlight parity, and interactive elements keep their independent visible keyboard focus outlines.
  - The shared public top atmosphere is the faithful Emerald `Background.vue` port described under "Visual language" above (full-viewport base, emerald/lime atmosphere, `farthest-side` radial mask, `mix-blend-overlay` grid, and the Dark variants); it stays pointer-inert and viewport-fixed in both themes and spans the full viewport width. The Node Detail observation panels and disclosures keep the shared card surface, hairline separators, and chart furniture readable in Dark. The transparent map uses a quiet slate basemap in Dark with the same Emerald observed-country fill and marker treatment, and Dark keeps group hairlines, chart grid lines, the secondary violet series, and Emerald link hover readable rather than inverting them.
  - Public loading, empty, error/unavailable, and last-good states use the same tokens in both themes, so unknown, stale, or failed observations never render as `0`, `false`, or Healthy.
- **Admin workbench and management flows (Issue #148).** The shared theme mechanism now covers the Admin shell, Overview, and the retained list/detail/settings routes without creating an independent Admin theme state. Admin uses the same Auto → Light → Dark preference and pre-mount first paint as the public pages on direct entry, reload, and cross-route navigation.
  - Admin keeps its quiet, undecorated workbench surface in both themes and never places the public gradient/grid across its tables or forms. Tables, forms, panels, filters, progress, code, and navigation resolve from Admin-scoped surface tokens per theme; semantic success/warning/error/unknown colors stay distinct rather than inverted, and color alone never carries status.
  - The compact workbench density, aligned content origin, priority-card table transform, and the mobile/tablet drawer's focus entry, Tab trapping, Escape and scrim close, body scroll lock, and focus restoration are unchanged. Loading, empty, failure, last-good, disabled, and danger/confirmation states keep their existing text-plus-icon/semantic treatment in both themes.
  - The retained Admin routes and their management operations, API calls, form validation, confirmation/cancel flows, and Owner-only boundary are unchanged; the slice adds no Admin behavior, data, navigation, or public-surface leakage.

### Exploration disposition and production boundary

The three throwaway variants (Signal stack, Mission control, Evidence ledger)
remain historical exploration evidence only, except for the accepted **A · continuous reading** calibration archived at `d4020316df380d695d80440fa4ce5b503ecf1980` on `prototype/emerald-a`, which is the binding visual benchmark for the public Node Detail container contract and the shared top atmosphere; the prototype module itself still does not enter the production bundle. Issue #89 completed the cleanup:
the production WebUI contains no variant switcher, prototype route branch, or
prototype-only module, and historical `variant` query parameters do not alter
the production Home or Node Detail routes. The supplied Emerald reference image remains local evidence and is not bundled, imported, or referenced by runtime code. The accepted production contract and its routed regression
coverage are the only supported Home and Node Detail implementation.

### Production seam and test intent

The highest-value external seam is the routed public Home shell and its child page modules, exercised through the typed Public API adapter and a controllable realtime invalidation source. Tests cross this seam with real Public DTO-shaped responses and explicit transport/error transitions; they do not reach into CSS selectors, private helpers, or implementation-only state. The same seam covers Home filtering/sorting, Node Detail identity, summary, panel, and chart content, block-interval derivation, Logo/Admin navigation, independent module failures, reset behavior, and last-good refresh preservation.

SCN-HOME-NODE-DETAIL is expanded with the visual and state assertions above. Add focused scenarios for SCN-HOME-FILTER-SORT, SCN-HOME-NAVIGATION, SCN-NODE-CONTAINERS, SCN-NODE-BLOCK-INTERVAL, SCN-NODE-INDEPENDENT-STATES, SCN-NODE-LAST-GOOD-REFRESH, and SCN-HOME-RESPONSIVE-ACCESSIBILITY. Each scenario must assert semantic content at all four fixed viewports; screenshots may supplement but cannot replace those assertions.

## 12. Playwright-oriented acceptance matrix

Use the existing `platpulse-web/playwright.config.ts` projects:

```text
phone-360-touch
phone-390-touch
tablet-768-touch
desktop-1280
```

| Scenario | Required assertions |
|---|---|
| `SCN-ADMIN-WORKBENCH-LAYOUT` | Required assertions and edge cases in §8.5.6; preserve shared access/state/confirmation contracts. |
| `SCN-AGENTS-PRIORITY-SUMMARY` | Required assertions and edge cases in §8.5.6; preserve shared access/state/confirmation contracts. |
| `SCN-AGENTS-DIAGNOSTIC-EVIDENCE` | Required assertions and edge cases in §8.5.6; preserve shared access/state/confirmation contracts. |
| `SCN-AGENTS-RESPONSIVE-DETAIL` | Required assertions and edge cases in §8.5.6; preserve shared access/state/confirmation contracts. |
| `SCN-AUTH-OWNER-LOGIN` | safe return, checking state, success, no password in URL/history |
| `SCN-AUTH-SESSION-REVOKED` | old stream closes, Admin data clears, no stale flash, login/revalidation path |
| `SCN-SITE-ACCESS-PRIVATE` | from `/admin/settings`, switch to Private, close public streams, require Home login, clear old Public cache, preserve Admin Owner-only access, and record an Audit Event |
| `SCN-SETTINGS-ROUTE` | canonical `/admin/settings` route, one h1, History Window before Site Access Mode, Settings-only navigation, removed-route fallback, and browser back/forward context |
| `SCN-HOME-NETWORK-LIST` | network list from Public Projection, all Active Nodes visible, anonymous access follows Site Access Mode |
| `SCN-HOME-GEO-MAP` | Six selection-scoped statistics beside a contained 2:1 map in 4:3 tracks at >=1024px, stacked statistics-first below; three summary columns from 640px, two on phones; unchanged 1280px cap and Node auto-fill/minimum-300px columns; Network filter updates all six and map, sort only changes Node order; cumulative exact Breakdown and independent coverage; Server Geo/Peer/map states, per-Node Peer-record basis, accessible country statistics and local degradation remain honest (see §11.1) |
| `SCN-HOME-NODE-DETAIL` | all Node Detail refinement acceptance checks above at 360/390/768/1280/1440 (independent header dimensions, exact six Validator metrics, ID copy/reveal failure, signed height deltas, isolated relative-age clock, collapsed diagnostics with visible errors, plot/SVG heights and scoped hover); continuous Node page on the shared full-bleed Emerald top atmosphere, with no coloured edge strip and no Details/Network tabs: an uncarded identity block with abnormal reason and last report, four key-summary tiles, three parallel chain/consensus, process-plus-Node-Data, and shared-Host observation panels, the real latest-60-second six-chart deck (process CPU/memory, Host upload/download, Peer inbound/outbound, block interval, transactions per block; four lines then two bars), and keyboard/touch Peer and technical disclosures, all backed by real retained samples with no Bounded Block History or fabricated zero |
| `SCN-HOME-UNAVAILABLE-NODE` | non-leaking unavailable copy for retired/unknown; no internal detail |
| `SCN-OVERVIEW-FRESH` | Attention precedes four linked summary cards, priority Node rows remain independently scoped, Agent cards show Host observations once, and Server Health/freshness/timestamps remain authoritative |
| `SCN-OVERVIEW-STALE-LAST-GOOD` | last-good remains, Error/Stale reason and age visible, no zero substitution, and failed refetch does not clear valid REST content |
| `SCN-OVERVIEW-UNKNOWN-UNSUPPORTED` | Unknown/Unsupported/Disabled/Empty remain distinct; Starting grace does not become Offline or premature attention |
| `SCN-OVERVIEW-ATTENTION-GROUPING` | typed critical/warning items remain independent, group by Subject without loss, show issue and Subject counts, preserve safe known-route actions, expose unknown-kind fallback, and label each item's own Server observation time (`Observation time unknown` when absent) without substituting the snapshot time |
| `SCN-OVERVIEW-PARTIAL-FAILURE` | Overview, Nodes, and Agents fail/retry independently; the Overview response keeps Attention and summary together but does not promise a cross-resource point-in-time snapshot |
| `SCN-OVERVIEW-EMPTY-SETUP` | authoritative zero/Empty values remain visible, safe Networks/Settings guidance appears, and no remote setup or fake-data action exists |
| `SCN-OVERVIEW-RESPONSIVE` | fixed 360/390/768/1280 layouts, summary transformation, Node cards, Agent stacking, touch/focus/Escape, 200% zoom, reduced motion, and no primary horizontal overflow |
| `SCN-SITE-ACCESS-PUBLIC` | from `/admin/settings`, switch to Public, allow anonymous Home reads, keep Admin Owner-only, clear affected state, discard stale responses, and record an Audit Event |
| `SCN-THEME-LIFECYCLE` | Auto → Light → Dark cycle and accessible names; refresh persistence; live system change under Auto; explicit Light/Dark override; invalid/unavailable storage; pre-mount first paint on direct entry; production key isolated from the prototype key; Login, Home, Admin, and the public Node Detail readable in both themes on the shared full-bleed Emerald top atmosphere with keyboard focus, 44×44 targets, no page overflow, and reduced motion; public card feedback (60% at rest; Home retains opaque-on-hover behavior and only explicitly interactive Home Node cards glow/lift; Node Detail static surfaces do not change opacity on hover; statistics and detail/chart/disclosure cards stay shadow-free and stationary; reduced motion removes the lift); Home filters/sorting remain operable in Dark (see §11.1) |
| `SCN-HISTORY-WINDOW-SHORTEN` | from `/admin/settings`, require confirmation, show old/new and impact, remove expired history asynchronously, and record an Audit Event |
| `SCN-HISTORY-WINDOW-BOUNDS` | out-of-bounds values rejected with field errors, bounds shown |

For each core scenario, test semantic content rather than screenshot alone, and verify no horizontal overflow at all four viewports. Test keyboard navigation, focus return, Escape, mobile drawer behavior, 200% zoom, reduced motion, accessible names, and preservation of URL/filter/scroll/expanded/draft state after refetch.

## 13. Implementation handoff checklist

A page is ready for production implementation only when:

- [ ] `PAGE-*` contract exists in this document;
- [ ] route and authorization boundary are explicit;
- [ ] OpenAPI operation and DTO references are verified;
- [ ] Public/Admin query namespace is assigned;
- [ ] SSE invalidations and reset behavior are specified;
- [ ] all required loading/empty/stale/error/Unknown/Disabled/Unsupported/access states are specified;
- [ ] mutation, confirmation, conflict, refetch, and Audit behavior is specified;
- [ ] redaction and non-leaking copy is specified;
- [ ] desktop/tablet/mobile transformation is specified;
- [ ] keyboard, focus, touch, zoom, reduced motion, and live-region behavior is specified;
- [ ] `SCN-*` Playwright scenarios are mapped;
- [ ] no prototype-only branch is required;
- [ ] implementation reviewer confirms the page contract before merging production code.

## 14. Decision/change log

| Decision | Source |
|---|---|
| Implemented slice: `PAGE-ADMIN-ENROLL` Add Agent guidance and Server-backed Agent display name/notes (`AgentDiagnostic.display_name`/`notes`) | Issue #169, child of the accepted §15 target |
| Implemented slice: explicit Agent Removal with Server-authoritative owned-Node preview, pending-Transfer block, credential revocation, and owned-Node cascade | Issue #171, child of the accepted §15 target |
| Accepted, not implemented: Agent enrollment/metadata/removal, permanent Node Purge, automatic Validator correspondence with one-time history reset, shared per-occurrence Agent Attention Acknowledgment | Confirmed management/Validator/acknowledgment `grill-with-docs` Q1–Q22 and final documentation approval; [main design §15](platpulse.md#accepted-management-target), [ADR 0004](../adr/0004-owner-removal-and-node-purge.md), [ADR 0005](../adr/0005-automatic-validator-identity.md), and this document §15. |
| Historical: Compact Admin workbench first delivery and six-column Agents summary; the later §8.6 information-architecture pass superseded the Settings/Audit deferral | Confirmed `grill-with-docs` Q1–Q12 and final documentation approval; retained as historical decision, not current route authority. |
| Admin visual shell and responsive baseline | Issue #35, accepted prototype branch `prototype/ui-shell-variants` |
| Home/Admin route and scope boundaries | Issue #34 |
| Shared freshness, realtime, and authorization | Issue #36 |
| Historical: Identity, lifecycle, access, and workflow contracts | Issue #37 — current Server supports Enrollment/Recovery/Rotation, while the SPA still has no management page for those operations. |
| Historical: Alert, maintenance, retention, backup/restore, Doctor page deferral | Issue #38 — Server/API capabilities now exist; the current SPA still has no corresponding routed pages. |
| Historical: Representative operations-loop prototype | Issue #39, branch `prototype/phase2-operations-loop` @ `58d6f9c` — prototype remains unrouted; Server Operations API is current. |
| Implementation handoff and acceptance contract | Issue #40 |
| Historical: scope convergence claimed no History Gap/Backfill, report-only Receipt, and Peer Count only | Superseded by the current AgentReport/Receipt, bounded Gap Backfill, Peer Snapshot/Presence, Geo, and Validator contracts in `docs/design/platpulse.md`. |
| Historical: site-level access mode with Owner-only principals and removed per-Node visibility | Superseded: human roles are Owner/Viewer, anonymous Guest is mode-gated, and per-Node `visibility` remains a legacy Admin compatibility field not used by Public SQL. |
| Accepted Home / Node Detail visual direction, responsive baseline, public-data contract, and production test seam | Issue #75 and accepted branch `prototype/home-node-detail` |
| Unified light Admin shell and Overview shared-theme foundation | Issue #110 plus Emerald visual refit |
| Unified Owner Settings page for History Window and Site Access Mode | Issue #111 |
| Admin visual convergence across retained pages | Issue #112 |
| Unified Admin experience integration contract, canonical Settings route, and fixed-viewport verification | Issue #113, parent Issue #109 |
| Historical: Komari-inspired Admin Overview triage hierarchy, typed Attention Items, shared Server Health policy, responsive limits, and page boundaries | Confirmed `grill-with-docs` design review; current code keeps typed Attention Items and responsive boundaries, but Public/Admin health precedence is route-specific rather than one shared evaluator. |
| Admin information architecture pass: Audit event table, Settings consolidation, Agent Detail summary and categories, Overview Agent inventory summary, Network mismatch wording, shared formatting/focus | Explicit product direction after §8.5; see this document §8.6 |
| Targeted Admin list/detail fix: Agents six-column cell alignment, Diagnostics summary plus cross-column disclosure, Audit cross-column detail row, explicit attention observation-time labels | Explicit product direction; see this document §8.6 |
| Prototype cleanup and production-only route boundary | Issue #89 |
| Admin navigation leading decorative glyphs (▦ ◈ ◉ ⬡ ⚙ ◫ ☷), `aria-hidden`, no route, authorization, page-title, or DTO change | Confirmed `grill-with-docs` decision; UI-only |
| Historical Home compact overview (four statistics + transparent Peer country map), local basemap and map-local degradation; six-card/filter-scope update supersedes its original composition | Issue #133, parent Issue #130; current accepted composition is in §11.1 |
| Production Auto/Light/Dark theme lifecycle, shared dual-theme foundation, and Login Emerald A adaptation | Issue #146, child of Issue #145; the final prototype calibration is archived at `d4020316df380d695d80440fa4ce5b503ecf1980` on `prototype/emerald-a` |
| Public Home and current Node Detail dual-theme adaptation with the calibrated ~60% card surfaces and hover/focus/disclosure feedback | Issue #147, child of Issue #145; visual-only slice of the #146 foundation |
| Public Node Detail continuous reading with grouped observations, a key summary, and collapsible Peer/technical diagnostics replacing the Details/Network tabs | Issue #149, child of Issue #145; supersedes the tabbed composition in §8.1/§11.1 |
| Public Node Detail real latest-60-second six-chart deck (process CPU/memory, Host upload/download, Peer inbound/outbound, block interval, transactions per block) with per-series degradation | Issue #150, child of Issue #145; final six-chart acceptance for the parent specification |
| Public Node Detail container realignment to the accepted Emerald **A · continuous reading** calibration (uncarded identity block, four summary tiles, three parallel observation panels) and the faithful Emerald `Background.vue` port of the shared top atmosphere with a full-bleed width deviation | Issue #151, correcting the visual drift in #146–#150; supersedes the single-hero-card, four-box-ban, metric-cell-grouping, and residual tab clauses while #149/#150 keep their semantic deliverables |

Changes to a settled contract require a new decision record and must update the affected `PAGE-*`, `PATTERN-*`, and `SCN-*` references together. OpenAPI or Server policy changes do not silently change WebUI semantics; they require an explicit design review when the user-visible contract changes.

<a id="accepted-management-ui-target"></a>

## 15. Accepted Agent/Node management and Attention Acknowledgment (partially implemented)

**Status:** Accepted through the management/Validator/acknowledgment design interview and final Owner approval; no implementation, route registration, generated API, data purge, or migration is delivered by this document update. The current routes and DTO limitations in §§4 and 8 remain factual baseline. This section supersedes their no-new-actions scope only for the accepted future controls below; it does not declare the deferred pages live. Implemented so far: the §15.1 Add Agent enrollment guidance and Server-backed display name/notes (`PAGE-ADMIN-ENROLL`, `PAGE-ADMIN-AGENT-DETAIL`, issue #169); the §15.2 explicit permanent Node Purge is delivered separately; the §15.2 explicit Agent Removal with owned-Node cascade is delivered (`PAGE-ADMIN-AGENT-DETAIL`, issue #171); and the §15.3 shared Agent Attention Acknowledgment is delivered (`PAGE-ADMIN-OVERVIEW`, `PAGE-ADMIN-AGENT-DETAIL`, issue #172). Automatic Validator identity and Current Validator Status are delivered by issue #173 (Public rendering consumes the Server-projected automatic correspondence with no manual role badge), and the one-time Validator model migration is delivered by issue #174 (the startup migration deletes the legacy manual Links and the old Validator snapshots/history/aggregates while preserving Node monitoring history, existing Incidents and Audit).

Server ownership, lifecycle, data boundaries, and acceptance are in [main design §15](platpulse.md#accepted-management-target). [ADR 0004](../adr/0004-owner-removal-and-node-purge.md) explains irreversible removal; [ADR 0005](../adr/0005-automatic-validator-identity.md) explains automatic identity and the one-time Validator history reset. Preserve the Emerald shell, mobile behavior, and public/admin separation; this is not another visual migration.

### 15.1 Agent enrollment and metadata

Applies to `PAGE-ADMIN-AGENTS` and `PAGE-ADMIN-AGENT-DETAIL`:

- Provide an Add Agent entry that creates a short-lived, single-use Enrollment Token and presents local enrollment instructions. Generating a token does not create an offline placeholder Agent; the list gains the Agent only after successful enrollment.
- Present the secret only in the authorized enrollment interaction; do not place it in URLs, logs, persistent browser storage, generic query caches, Audit payloads, or Public state. No remote installation/start/stop command is executed by the UI.
- Support Server-backed display name and notes editing. Keep the stable Agent ID visible/copyable; do not make the ID, actual Host identity, receipt-derived liveness, or local collection configuration editable. A name is not inferred from a Node or diagnostic field.
- Keep credential revocation separate from Delete Agent. This scope does not restore dedicated Recovery/Rotation or other deferred pages. Exact added API shapes and form placement remain implementation work, not fabricated extensions of today's DTO.

### 15.2 Permanent removal and clear consequences

Applies to Agent Detail and the Nodes inventory/Node Detail management surfaces (`PAGE-ADMIN-AGENT-DETAIL`, `PAGE-ADMIN-NODES`, `PAGE-ADMIN-NODE-DETAIL`):

- Agent deletion confirms the specific Agent and Server-authoritative list of owned Nodes, revocation of all credentials, and permanent removal of those Nodes' monitoring history. Pending Transfer blocks removal until handled. A changed ownership/impact set requires refetch and confirmation, not deletion of unseen newly assigned Nodes.
- Node deletion is explicit Owner judgment, available for invalid Active as well as Retired Nodes; do not invent an automatic offline threshold or label failure itself as a lifecycle. Retain existing rename; add no Admin create-Node or Endpoint editor.
- Use explicit destructive copy such as `Permanently delete Node`. Explain that the Node leaves Home/current monitoring, its observations/history/Links are deleted, the same Node ID cannot return, and re-monitoring requires a new locally configured Node ID. Minimal deletion identity, necessary Audit and existing Incident evidence remain; independent Validator history is not erased.
- Explain what will not happen: the remote Agent/Node process is not stopped, local configuration is not changed, shared Node/Host/Network/Validator data is not cleared, and no recovery is asserted. Agent removal includes the stated credential revocation and owned-Node purge; it is not merely hiding a row.
- Confirm/Cancel, busy guards, field/page errors, conflict recovery and authoritative completion are required. Do not optimistically remove records or call a queued/incomplete purge a completed deletion. On completion, invalidate/refetch affected Admin lists, overview and summaries; an open Public view must stop displaying the removed Node and use the existing non-leaking unavailable outcome for its detail URL.
- Preserve context for unaffected rows and filters. Failure leaves an actionable error and authoritative state, not an empty success screen. Server permission checks and Audit are mandatory; a typed phrase, if used for friction, is not the authorization boundary.

### 15.3 Shared confirmation of Agent Attention Items

Applies to `PAGE-ADMIN-OVERVIEW` and `PAGE-ADMIN-AGENT-DETAIL`:

- The product action is `Acknowledge` (确认提示), not `Resolve alert`, `Clear history`, or `Silence`. Production copy remains consistently English; the Chinese wording here identifies the confirmed intent, not a localization feature.
- Overview provides per-item acknowledgment for Agent subjects. Agent Detail provides individual acknowledgment and `Acknowledge current Agent items`. No Node acknowledgment or permanent type-disable action is added.
- A bulk action covers only the Agent items/evidence boundaries explicitly shown for that action, not all hidden diagnostics, child Node items, or new evidence arriving after the displayed snapshot. Server confirmation is shared across Owners, both surfaces, sessions and restarts; browser local storage is not the authority.
- On authoritative success, that occurrence leaves the prominent attention/warning area. Evidence remains available through deliberate diagnostic disclosure; actual liveness, health, freshness and component failures remain truthful. Even a continuing fault may be acknowledged without making an offline Agent look online or changing Incident/notification policy.
- Ordinary refreshes, unchanged reports, re-login and restarts never resurrect an acknowledged occurrence. New evidence or a genuinely recovered-then-recurring fault requires fresh attention. Unknown/stale data is not recovery. The Server must distinguish occurrences/evidence; the current `kind + subject` ID and presentation timestamp alone are insufficient.
- A failed acknowledgment keeps the item actionable. A stale request must not swallow newer unseen evidence. Successful concurrent acknowledgments must converge after authoritative refetch; don't replay stale responses into the queue or transfer business DTOs between tabs.
- Existing Agent Detail warning predicates include historical and current evidence outside the Overview queue. These surfaces must converge on Server-owned acknowledgment eligibility/evidence boundaries rather than independently recreating acknowledged warnings from cumulative counters. Raw safe diagnostics remain available but are not re-promoted as unacknowledged prompts.
- Audit exposes who acknowledged which Agent evidence and when, not secret-bearing raw diagnostics. The acknowledgment operation does not disable Alert Rules, change Incident state, delete evidence, pause notifications or create a recovery event.

### 15.4 Validator presentation and deleted-subject history

- Remove manual Validator selection, role editing and manual-link fallback from the target design. Validator detail and statistics remain independently meaningful; Public rendering consumes Server-projected automatic correspondence rather than guessing in React.
- Show Current Validator Status separately from Node Health and consensus participation. Preserve locked/exiting qualifiers when validity is confirmed, explicit non-current status after completed exit/authoritative absence, and Unknown/Stale when evidence cannot establish a fresh conclusion. Do not reintroduce manual role badges. Activity placement follows existing page composition; this does not authorize an unrelated Home redesign.
- The one-time model migration clears old Validator Link/history/snapshot/daily/monthly data, not Node monitoring history. A temporarily Unknown or lower selection total is legitimate; no client fallback to old manual results, fabricated full-month coverage, or forced zero lifetime totals. See [Provider](validator-provider.md) and [metrics](validator-metrics.md) for the target evidence contract and pending primary-source verification.
- Daily Node deletion does not clear independent Validator history, even after its last monitored Node is removed. Existing Incident evidence is retained and marked with deleted-subject context, excluded from current actionable problems, not relabeled as known recovery. Cancel unsent subject notifications; delivered messages cannot be recalled.
- Current problems disappear from Attention when their predicates genuinely clear; historical prompts can remain until acknowledgment. Sustained known recovery resolves a durable Incident without deleting it. The current SPA has no full Incident history page and this scope adds none; do not imply history is displayed on a route that remains a fallback.

### 15.5 State, accessibility and target acceptance

All added controls use Owner authorization, the existing CSRF/Origin boundary, Audit, generated-client contracts once implemented, separate Public/Admin query namespaces, and post-commit SSE invalidation plus authoritative REST. Loading, Empty, Unknown, Stale/LastGood, error, conflict, forbidden and busy states remain independent. New operations and occurrence DTOs require actual Server/OpenAPI implementation before controls are enabled; unavailable operations cannot report fake success.

Preserve 44×44 CSS pixel targets, keyboard-accessible confirmation/disclosure, focus trapping/restoration where appropriate, non-color status text, live feedback without excessive announcements, and no primary horizontal page overflow. A disappearing confirmed item restores focus to a safe adjacent item or panel heading. Use all five fixed Playwright projects (360, 390, 768, 1280, 1440 widths), Light/Dark, long labels/IDs, 200% zoom and reduced motion; no new server or theme is required.

The following extend the future acceptance matrix; they are not claims of implemented tests or a successful browser run:

| Scenario | Pages | Expected outcome |
|---|---|---|
| `SCN-AGENT-ENROLL-GUIDANCE` | Agents | Token and safe instructions; no placeholder or remote execution; successful enrollment subsequently appears |
| `SCN-AGENT-METADATA` | Agents / Agent Detail | Server-backed name/notes round trip; ID and observed Host/liveness unchanged; conflict/error/authorization handled |
| `SCN-AGENT-REMOVE` | Agent Detail | Confirm exact owned Nodes; Transfer blocks; credentials revoked and child purge completes; no remote stop claim |
| `SCN-NODE-PURGE` | Nodes / Node Detail / Home | Active or Retired purge is irreversible; no create/Endpoint control; removed Node unavailable publicly; shared data survives |
| `SCN-PURGE-NO-REAPPEARANCE` | Nodes / Overview | Test fixture submits subsequent valid Inventory; deleted ID stays absent and other Nodes continue reporting |
| `SCN-AGENT-ATTENTION-ACK` | Overview / Agent Detail | Same occurrence disappears on both surfaces and for a second Owner; diagnostics/health remain truthful after refresh and Server restart |
| `SCN-AGENT-ATTENTION-RECURRENCE` | Overview / Agent Detail | Unchanged reports don't resurface; new evidence or known recovery then recurrence does; Unknown alone does not rearm |
| `SCN-AGENT-ATTENTION-BULK-RACE` | Agent Detail | Only displayed Agent evidence acknowledged; newly arriving evidence, unseen items and Node prompts survive; failure/conflict is visible |
| `SCN-AUTO-VALIDATOR-PRESENTATION` | Home / Node diagnostics | Automatic correspondence, special states and stale/unknown remain explicit; no role/manual fallback or guessed lifetime zeros |
| `SCN-MANAGEMENT-ACCESS-RESPONSIVE` | All affected pages | Owner-only controls, safe Public state, fixed viewports/themes, keyboard/touch/focus, error recovery and no primary overflow |
