# PlatPulse WebUI Design and Current Routed Surface

**Status:** Current routed WebUI contract, reconciled with `platpulse-web/src/App.tsx` and the Server DTOs.

**Scope:** The production React SPA surface currently registered in `platpulse-web`, plus the read-only Public projections it consumes. Server/API extensions that have no SPA route are documented as available-but-unrouted, not silently treated as pages.

**Primary sources:**

- `CONTEXT.md` for domain vocabulary;
- `docs/design/platpulse.md` for Server, Agent, API, security, and deployment boundaries;
- generated OpenAPI artifacts for DTOs, operations, error envelopes, and client behavior.

This document is the WebUI UX and interaction authority for the current routed surface. It does not replace OpenAPI and does not define Server policy. The current Home consumes country-only Geo Insight, aggregate Peer Insight/selected-Node Peer History, and read-only Validator Activity/history/analytics; the Owner selects the Geo provider (Disabled, Local MMDB, IPinfo, or GeoJS) on Settings. Server-side Alert, Notification, Retention, Backup/Restore, Doctor, Node Transfer, People, Validator management, and Agent Enrollment/Recovery/Rotation operations exist, but their management pages are not registered in the current SPA; older page drafts are historical and must not be linked as live routes.

## 1. Purpose and non-goals

PlatPulse WebUI presents operational truth from the Server and gives the Owner safe, audited configuration. Home is the read-only, Node-first monitoring surface — readable by anonymous Guests when Site Access Mode is Public and by authenticated Owner or Viewer sessions when Private; Admin is the Owner-only overview and configuration surface. It is a monitoring and administration surface, not a remote-control terminal.

### 1.1 In scope

- Home Dashboard: read-only Public Projections, Network → Node → Node Detail, a compact overview combining the global statistics with the Peer country map, and a current block interval derived from the latest two consecutive retained Block Summaries;
- Admin Dashboard: Agent/Node/Network configuration, global history window, Site Access Mode, Sessions, and Audit;
- responsive behavior at 360×800, 390×844, 768×1024, and 1280×800;
- current aggregate Peer Insight and selected-Node Peer History; Peer Snapshot/Presence data is Server-side and redacted, while raw Peer identities are never displayed;
- independent collection, freshness, value, and authorization states;
- REST-authoritative data loading and SSE invalidation;
- accessible forms, tables/cards, confirmations, errors, conflicts, audit links, and session transitions;
- deterministic Playwright-oriented acceptance scenarios.

### 1.2 Out of scope

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
- Node Health Summary is not a WebUI-computed health color;
- an Agent that stops reporting does not retire its Nodes;
- an Agent-level page must not merge independent Node chain observations;
- a successful Peer Snapshot/aggregate with zero peers is authoritative, not Unknown; omitted or unsupported peer capability is not an empty snapshot;
- recent Block History is bounded by the Server window and is best-effort: normal missed blocks remain absent, while explicit bounded Gap Backfill may recover eligible heights; neither path synthesizes zeroes or fabricated summaries.

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
Live updates connected
Connecting to live updates
Live updates paused
You are offline
```

`Peer data current` is a scoped, low-weight healthy summary for Peer Insight; it does not replace the independent Collection, Freshness, or Value terms when those dimensions need explanation. `Online` and `N/A` are not generic replacements. Status communication always includes text and an icon or equivalent explanation; color is supplementary.

## 3. Surfaces and authorization

### 3.1 Home Dashboard

Home is a read-only surface for Public Projections. The current Public adapters consume Network/Node history, metric history, selected-Node Peer History, country-only Geo Insight, and Validator history/analytics where the DTO exposes them:

```text
Home
├── All Networks
├── Network Overview
│   └── Node list/cards
└── Node Detail
    ├── Health, Node status, Validator membership role, and process uptime
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

The Admin shell shares Home's accepted Emerald light visual language: a Slate-50-like canvas, quiet white surfaces, light borders, restrained shadow, high-contrast primary text, quiet secondary copy, and measured Emerald selection accents. The Owner-only shell keeps its management information architecture: a persistent desktop sidebar and an accessible tablet/phone drawer with focus entry, Tab trapping, Escape and scrim close, body scroll lock, and focus restoration. Header and navigation controls remain at least 44×44 CSS pixels.

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
| `PAGE-HOME-NETWORK` | `/networks/:networkKey` | Network overview, aggregate Peer/Geo/Validator modules, and Node cards | Anonymous Guest (Public mode); authenticated Owner/Viewer (Private mode) |
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
| `PAGE-ADMIN-AGENT-DETAIL` | `/admin/agents/:agentId` | Identity, credential status, liveness, inventory, diagnostics | Owner |
| `PAGE-ADMIN-NODES` | `/admin/nodes` | Node list, health summary, freshness, and legacy visibility filter | Owner |
| `PAGE-ADMIN-NODE-DETAIL` | `/admin/nodes/:nodeId` | Administrative view over the full AdminNodeDetail DTO; UI renders the approved diagnostic subset | Owner |
| `PAGE-ADMIN-NETWORKS` | `/admin/networks` | Network Registry metadata and Nodes | Owner |
| `PAGE-ADMIN-NETWORK-DETAIL` | `/admin/networks/:networkKey` | Expected identity, metadata, mismatch diagnostics | Owner |
| `PAGE-ADMIN-SETTINGS` | `/admin/settings` | Global Block History window and Site Access Mode configuration | Owner |

The table above is the complete set of concrete SPA page routes; unknown paths under `/admin` use the registered Admin wildcard fallback rather than a legacy page. The Server/Admin APIs additionally expose People, Validator management/links/analytics, Alerts, Notifications, Operations, Retention, Backups/Restore, Doctor, Node Transfer, and Agent enrollment/recovery/credential operations; these are available DTO/operation surfaces, not current SPA pages. Geo provider status and selection are consumed by the Settings page.

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

SSE connection status is visible but does not cover valid content. Public Home/Network/Node surfaces show `Connecting to live updates` while connecting, `Live updates connected` for an open stream, and `Live updates paused` after disconnect; browser/network loss additionally uses `You are offline` when the browser signal is authoritative. The compact Admin header retains its existing `Starting`/`Current` transport labels so this public wording change does not increase Admin information density. These transport labels never certify REST refresh success, observation freshness, Agent liveness, or Node Health. The current generated OpenAPI records the stream operation but does not fully describe every cursor/header/event field; runtime `realtime` behavior is the authority for replay.

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

- the primary Node card uses a compact Komari-inspired density with a neutral one-pixel outline and no coloured edge strip. It shows display name/Node ID, Server-owned Health, Node status, the separate `Validator` membership role, process uptime, compact PlatON process CPU/process-memory/Node Data progress, `Head / QC / Locked / Committed / Validator`, process start time, and last Agent report time. Validator Activity/history is a Network Overview module, not the Node hero's `Node status` label. Routine Healthy prose is omitted; an exceptional Server-owned health reason remains visible;
- Details contains exactly four equal-size one-minute metric cards: shared Host network receive/transmit rates, Peer connections, latest consecutive-block interval, and latest Block Summary transaction count. Network and Connections use line charts; Block time and Transactions use bar charts;
- the bounded Block History list and public-history export are not rendered on Node Detail. The history endpoint remains a Server boundary; the page reads retained summaries only to derive the consecutive-block interval, and missing/non-consecutive summaries remain `Unknown`;
- the Node Detail Network tab shows Peer Insight and the selected Node's aggregate Peer History; Network Overview shows network-level Peer Insight and Geo/Validator modules. No peer address or identity list is exposed;
- Host CPU and memory are never substituted for PlatON process CPU and memory, and shared Host network rates are labelled as Host observations;
- retired/deleted/unknown Nodes use the non-leaking unavailable semantics.

### 8.2 Admin Node Detail (`PAGE-ADMIN-NODE-DETAIL`)

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
| `SCN-SETTINGS-GEO-REFRESH` | With Local MMDB selected and an open Public Network view, the Owner triggers the global refresh, reads real terminal counts that are stated as IP lookups beside the separate Peer-record count, and sees the Public country buckets update without a reload and without a raw address or database path appearing on either surface; while Geo is Disabled the control stays disabled with the Server-owned reason and never reports a success that did not run; a Viewer cannot reach the card or the refresh route. |
| `SCN-GEO-BACKGROUND-RESOLUTION` | With Local MMDB, IPinfo, or GeoJS selected, the already-open Public Network view updates its Server-computed Known/Unknown country buckets through the existing realtime invalidation once the background resolution completes, without a reload and without exposing a Peer address, database path, or external request detail. |
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

An Attention Item is a current Server-derived prompt, not an Alert Incident, Notification, Audit Event, or browser-computed warning. The REST DTO keeps each problem independent with a stable `kind + subject` identity. The typed kinds are:

```text
agent_offline
agent_spool_fatal
agent_spool_overflow
agent_report_gap
agent_security_event
agent_shutdown_incomplete
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

The current presentation uses the shared Admin shell and compact Agents summary, with regression coverage across all retained Admin routes and isolation checks for Home. Preserve the Emerald brand and existing business rules; borrow compact spatial organization, not another product's dark theme, small text, or data model.

- In scope: Admin background, sidebar, header, content origin, heading scale, shrink/overflow boundaries, Agents summary organization, and existing Agent Detail access to secondary evidence.
- The current page boundary excludes Server/API expansion, new client-derived state or severity, global status renaming, a comprehensive restyle of unrelated controls, and restoration of removed routes or features. The integrated §8.6 pass covers the Settings/Audit/Overview/Agent Detail information-architecture changes explicitly; Server extensions without SPA routes remain available-but-unrouted.
- This section governs the shared Admin container and Agents presentation. Settings §8.3 and Overview §8.4 retain their documented internal composition and behavior; the shared shell is consistent across them. Home and Login presentation remain separate contracts.
- Authorization, Public/Admin separation, redaction, last-good semantics, REST authority, query namespaces, SSE invalidation/reset, URL/back-navigation context, and mutation contracts in §§3–7 remain in force. Do not introduce new API operations or optimistic business state.

#### 8.5.2 Shared shell and density

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

Admin uses a stable Slate-50-like background and quiet white panels, without the public gradient/grid crossing its reading surface. Emerald remains a measured accent. Sidebar selection uses a light Emerald background, stronger text, and a thin side marker; keyboard focus remains separately visible, for example through `:focus-visible`. Do not remove outlines without an accessible replacement. The current contract does not require recoloring every existing primary button.

Measure actual container bounds before changing CSS: source inspection found centering and maximum-width rules, but the Admin maximum width need not bind at 1280 CSS pixels. Do not assume every large gap has the same cause. Check combined sidebar reservation, margins, padding, and the correct Flex/Grid shrink boundaries. Apply `min-width: 0` where needed; do not globally break words. Table headers may wrap between words, not split letters. Necessary two-dimensional overflow belongs to the table container, never the page, heading, or action area.

Retain §2.1 vocabulary, including `Current`, with a visible, accurate dimension label. Place global realtime status in a compact shared status area near the header/page context, not an isolated pre-heading row. SSE connectivity does not certify data freshness, Agent liveness, Node health, or credential validity. Do not invent update timestamps or refresh capabilities.

#### 8.5.3 Agents summary and existing detail

The Owner scans Agent reporting, inventory, credentials, and diagnostic evidence, then opens the existing `/admin/agents/:agentId` route for investigation. Keep `/admin/agents` as the list route and preserve existing access checks, query/realtime behavior, safe return, and authoritative error handling. Use existing `AgentDiagnostic` data; this is a presentation-only change.

Desktop uses six summary columns:

| Column | Default content and limits |
|---|---|
| Agent | Shortened Agent ID linked to the existing detail route. The current DTO has no Agent display name or hostname; do not manufacture one from Node names. |
| Reporting status | Existing Server liveness with its dimension explicit; not Node health, browser connectivity, clock reliability, or credential status. |
| Last received | Server `last_received_at`, explicitly labelled as receipt time, with never-received/unknown preserved; not the report sequence. |
| Node Inventory | Total retained Nodes assigned to the Agent. `nodes.length` includes Active and Retired Nodes; do not label this Active, online, or healthy Nodes. |
| Credentials | Server-provided validity and necessary counts, never secrets. Active and explicitly revoked counts need not sum to total: expired credentials can be neither. Do not recalculate validity in the browser. |
| Diagnostics | Separately labelled historical counters and available spool/reporting evidence, not one healthy/failed rollup. |

Do not add a separate View-only action column. The identity link provides detail access. The Diagnostics column keeps its labelled historical counters and a concise evidence brief; complete safe diagnostic evidence stays reachable through an accessible in-row disclosure that opens a separate full-width row below the record (the Nodes inventory pattern), or on the existing detail sections. Full Agent ID must be viewable and copyable through keyboard and touch, not only a hover tooltip. Epoch, full boot/shutdown identifiers, and report sequence belong in the existing detail sections. Retain redaction: “complete” means the existing authorized, redacted evidence, not raw secret-bearing errors or report bodies.

The existing detail separates Identity, Liveness, Boot/report state, Inventory, Credentials, Diagnostics, and Audit. Preserve independent states and evidence reachability. Credential revocation stays there with its explicit warning, Confirm/Cancel flow, busy guard, conflict reload, and authoritative refetch; no destructive quick action is added to the list. No new gap timeline, resolve/reset operation, or recovery workflow is promised.

#### 8.5.4 Diagnostic evidence semantics

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

## 11.1 Accepted Home and Node Detail visual contract (Issues #75 and #97)

The accepted direction from Issue #75 and the compact Home contract from Issue #97 remain the production structural baseline for the public Home surface. The visual layer now adopts the supplied Emerald reference and the inspected Tokinx/komari-theme-emerald source at commit b7baf4535939cfdda063d731943fc36e3ead4c51, without importing its server, pricing, or remote-control data model.

### Visual language

- Home, public Node Detail, and Login use a Slate-50-like light canvas with a restrained Emerald-to-Lime top gradient and low-contrast inclined geometric grid. The background is decorative, pointer-inert, absent from the accessibility tree, and fades before the lower reading surface. Admin retains the Emerald light brand but uses the stable, undecorated workbench background specified in §8.5; this exception does not change Home or Login.
- Surfaces are quiet white or translucent-white panels with approximately 8px radius, light borders, compact spacing, and minimal shadow. Default panels do not require `backdrop-filter`; menus, forms, dialogs, and other overlays may use more opaque surfaces.
- Primary numbers and page titles use high contrast and strong weight. Secondary labels, timestamps, identifiers, and explanatory copy are quieter without being reduced below readable body sizes. Important values use tabular numerals where appropriate.
- Emerald is a measured brand accent and selection cue, not a replacement for every primary action. Neutral primary controls, blue chart series, amber warnings, red destructive/error states, and neutral Unknown/Unsupported states remain distinct.
- Cards, pills, separators, fine 4px progress tracks, and focus states share one spacing and radius system. Clickable cards may use a 150–200ms hover lift of about 2px and a faint Emerald shadow only on hover-capable devices; static containers do not imply interactivity. Reduced motion removes the lift and non-essential transitions.
- The visual contract does not authorize fields that are absent from the Public Projection. Node Detail may show the monitored PlatON process CPU/memory, process start/uptime, last Agent report time, Node Data usage/capacity, and sampled Host network receive/transmit rates supplied by the Server; it does not add pricing, raw Peer identity, Host identity, or RPC Endpoint text.
- The default geometry is adapted from the Emerald repository's inline `Background.vue` SVG under its MIT license. Keep the copyright and license notice with the adaptation; do not copy Komari branding, logo assets, external flags, fonts, or theme-management behavior.

### Home composition (`PAGE-HOME-NETWORKS` and `PAGE-HOME-NETWORK`)

The two routed Home surfaces are separate: the root dashboard flattens the returned Active Node projection, while Network Overview loads one selected Network DTO and its network-level modules. Both remain read-only and never reorganize the view around Agent or Host topology.

**Root `/` (`PAGE-HOME-NETWORKS`):**

1. A compact header with the PlatPulse brand link at left and one Owner-only Admin link at right that pairs a gear icon with visible `Admin` text. The brand returns to Home; the Admin link enters the Admin Overview route and does not expose Admin data inside Home.
2. A page heading, Public Projection copy, and a Server-authoritative live/realtime indicator.
3. A compact overview group: a 2x2 statistics block (Active Nodes, Healthy Nodes, Attention, Networks) beside a transparent Peer country map, described in "Home overview and Peer country map" below. The four statistics are projections of already-loaded Public data, not new health policy or visibility filtering; they stay global while the Network filter and sort change only the Node list and the map scope.
4. Network filter pills and a labelled sort control (`Health`, `Name`, or `Current Head`).
5. Compact Active Node cards. Each whole root-dashboard card is one semantic link to Node Detail; the Network name is plain text and there is no nested Network link. Healthy Nodes omit routine prose; exceptional Nodes may show one short diagnostic line.
6. Card data ownership is explicit: PlatON process `CPU`/`Memory`, `Node data` size/capacity, Host transfer rates (`↑ Up`/`↓ Down`), Node `Head`/`Transactions`/`Peers`, and Consensus `QC`/`Locked`/`Committed`/`Validator`. Missing values remain unavailable rather than becoming zero.
7. The dashboard can show `No Active Nodes in this view` when the loaded projection/filter has no cards. This is distinct from a registered Network that the current Server endpoint omits or returns as `404` because it has no Active Node.

**Home overview and Peer country map (Issue #133):**

1. The overview keeps a stable Grid: about 42% for the 2x2 statistics and 58% for the transparent map. There is no fixed overview minimum height: removing auxiliary content must also shrink the parent and move filters and Node cards upward. The desktop map footprint targets 220–260px, adjusted to its available width without stretching the world or clipping marker labels. A small reserved corner-credit strip is allowed; large internal chart margins are not. The transparent Home logo bar is the shell's own row above this band at every viewport: like the Emerald reference, whose map is a content grid item beneath its bar, no part of the map, its markers, or its corner indicator is layered behind the brand, and the band never occupies the bar's strip.
2. The statistics keep their global semantics and accepted order: Active Nodes / Healthy Nodes, then Attention / Networks. The filter and sort never change these numbers. The former long global-scope explanation is in Map information, not a permanent fifth statistics row.
3. A normal map shows only one line, “Peer countries · n records”, two icon controls (Map information and Show full map / Collapse map), the world map, and a short necessary credit. The title count is the existing in-scope Peer-record denominator, not monitored Nodes or unique Peers. Unavailable values use a dash, never an invented zero. There are no routine Current badges, scope/resource/freshness rows, country chips, or standalone expansion-button row.
4. Map information opens by click, keyboard, or touch as a lightweight non-modal disclosure. It contains current scope, Known/non-zero Unknown counts, Server-provided unknown reasons, per-Node/not-deduplicated-by-IP basis, independent Geo/Peer observation/map-resource states, last-good/age/stale/error details, partial and Network-basis notes, and the global statistics/filter explanation. Escape, close, and outside pointer dismissal remain available, with appropriate focus restoration.
5. Every country with a count remains accessible in the disclosed country list by ISO code and country name, including missing representative-point/outline explanations. Hovering or tapping a highlighted country or its marker shows name/count; markers also support keyboard activation. No marker, random point, or [0, 0] fallback is fabricated. Unknown locations are separate from known countries whose quantity cannot be plotted. A missing outline alone does not warrant a missing-location warning when a valid quantity marker exists.
6. The basemap remains the fixed, locally hosted Natural Earth 1:110m Admin 0 Countries asset (public domain), projected in its own equirectangular space. No runtime map CDN, tiles, geolocation, or new data source is introduced. Full original basemap and Server-owned Geo attribution, source labels, and provider links stay accessible under Map credits. Only short source labels drawn from the supplied attribution are shown in the map's low-interference corner; these do not replace or rewrite the full authoritative attribution in details.
7. The default map shares the existing light-green Home wash: no white card shell, prominent border, shadow, or large title bar. Outlines remain light grey, observed countries take the existing restrained Emerald fill, and count labels retain contrast. Only the on-demand information disclosure has its own light surface. The one standing figure in the map's corner is the reference theme's pointer-inert chip: a pulsing dot with the total number of Peers the in-scope Active Nodes are linked to. It is the Server's own Peer-record denominator for that scope — the same records the country fills and markers are drawn from, counted per Node and never deduplicated by IP — so it can never disagree with the map beneath it, and a scope without a successful Peer Snapshot shows no figure rather than an invented zero.
8. Exceptional states add at most one compact notice, for example Data stale, Some locations not shown, or Map unavailable with Retry map. Non-zero unknown locations may share that notice without being counted as known unplottable countries; Unknown 0 is omitted. Loading, no observations, authoritative empty data, unavailable data, and last-good data remain distinct. Errors/staleness must not be masked by an authoritative retained zero. Geo Disabled remains a neutral local notice without loading geometry, and render failures remain contained by the existing map boundary.
9. Narrow layouts stack statistics above the complete world map and use its natural aspect ratio rather than a fixed-height blank canvas. The existing expanded/collapsed control keeps aria-label, tooltip, aria-expanded, aria-controls and 44px targets. Expansion increases actual available map width (using the Home gutters on narrow screens and the full overview on desktop), never shrinks a tablet map or merely adds vertical whitespace. SVG layout responds to container changes without a chart-library resize lifecycle. Neither state causes page-level horizontal scrolling.
10. No default wheel zoom, 3D, or continuous animation is added. Decorative background layers remain pointer-inert. Data aggregation, Network filtering, realtime updates, last-good semantics, error handling, backend interfaces, and Node-card information structure are unchanged.

**Network Overview `/networks/:networkKey` (`PAGE-HOME-NETWORK`):**

1. Breadcrumb, Network title/key, realtime state, and refetch error/last-good state.
2. Network-level aggregate Peer Insight leads with primary `Peers / Inbound / Outbound` counts and a secondary `Trusted / Static / Consensus` row. The counts come directly from the Public Projection; the browser does not aggregate Nodes or expose peer identities.
3. A fresh, successful Peer Snapshot uses one quiet `Peer data current` summary. Collection, freshness, and value remain independent: Unknown freshness never becomes Current, a successful empty snapshot is an authoritative zero, and omitted values remain Unknown.
4. Collection failure, Stale, Disabled, Unsupported, and Starting states remain visible without hiding a retained value. A retained value is annotated `Showing last successful snapshot`; when collection failure and Stale coexist, both are shown. Aggregate Peer Insight does not display a shared observation timestamp; it says `Observation time varies by Node`.
5. Country-only Geo Insight is composition-aware. When the Server reports Geo `Disabled`, render only the neutral single-line `Peer countries · Disabled by server` notice: no panel, prominent status badge, enable action, browser permission request, or Geo configuration. The Peer Insight takes the available overview width. When Geo is enabled, the Countries area remains substantive and distinct for Current data, authoritative empty data, Error, and Stale states; retained country counts, Server-provided reason/last-good/database-age metadata, and required attribution remain visible. It shows the Server-computed `Known n · Unknown n` buckets on the same Peer-record basis as the aggregate Peer Insight (never a browser-side subtraction), names the Server-provided non-zero Unknown reasons (never a browser subtraction), marks a retained last-good country `n retained as last-good Stale`, keeps a known country without a representative point in the accessible text list, and states `Partial scope` when some Active Nodes have no successful Peer Snapshot. No reliable denominator renders as Unknown text, never as zero. The Public Projection exposes country counts only, never peer addresses, IPs, or precise locations.
6. Validator cards with read-only Activity, history, and analytics when the Public DTO contains Validators.
7. An Active PlatON Nodes section. Each compact Network Overview card presents the Node name and Server-owned Health, then `Head`, `Peers` with inbound/outbound counts, and `Oldest component update`. The latter is the earliest Server receipt across RPC, Sync, and Consensus and is `Unknown` when the complete timestamp is unavailable; it shows relative time plus a complete UTC absolute time and a visible explanation. A quiet inline RPC/Sync/Consensus collection-state row follows, routine Healthy prose is omitted, exceptional Server-owned reasons wrap without truncation, and `View details →` navigates to Node Detail. Network Overview cards may contain nested Node links; they are not the root dashboard's whole-card-only contract.

   Validation note for issue #128: at the fixed 1280×800 desktop viewport, the same Healthy fixture measured approximately 452px before and 312px after when cards were sized intrinsically, an approximately 31% reduction. This is a comparable design observation, not a fixed height or pixel-level UI contract.
8. The current Server returns `404 not_found` when the selected Network has no Active Node, and `/api/public/v1/networks` omits Networks with no Active Node. The component contains an empty-array state for DTO compatibility, but clients must not assume every registered Network is returned as `200 {nodes: []}`.

### Node Detail composition (PAGE-HOME-NODE)

Node Detail freezes the accepted reference-inspired hierarchy:

1. One compact Komari-inspired Node card owns the page identity. It uses a quiet neutral outline, restrained radius and shadow, no coloured top/edge strip, and shows the Network back context, display name/Node ID, Server-owned Health, visible `Node status`, the separate `Validator` membership role, and process uptime. Validator Activity is a read-only Validator projection on Network Overview, not the hero's Node-status label. Routine Healthy prose is omitted, while exceptional Server-owned health reasons remain visible.
2. A compact resource row inside that card follows the Home Node-card hierarchy: PlatON process `CPU`, PlatON process `Memory`, and `Node data`, each with a current value and a progress track when a valid percentage exists. Node Data keeps its size and filesystem-capacity detail; unavailable values remain explicit rather than becoming zero.
3. A chain-specific consensus summary presents the parallel heights `Head / QC / Locked / Committed` and an independent `Validator` role; explicit membership renders `True` or `False`. Process start time and last Agent report time form the card footer. Missing or uncertified values remain `Unknown`.
4. A centred two-tab control defaults to Details and switches to Network without replacing the large Node card. The selected tab is exposed semantically and visually.
5. Details presents four equal-size, compact cards in this order: Host network upload/download rates, Peer connections, block interval (`latest block timestamp - previous consecutive block timestamp`), and latest Block Summary transaction count. Every card keeps its current value and a labelled 60-second chart with `1m` and `0s` time bounds; reduced padding, chart height, radius and shadow keep the deck close to Komari's information density.
6. Network renders upload and download as distinct line series; Connections renders inbound and outbound as distinct line series. Block interval and transaction count use Server-retained Block Summary samples as bars. The Server may include one last-good point immediately before the window as a line chart's starting value, but bar charts render only observations inside the window; neither Server nor WebUI fabricates intermediate samples or substitutes zero for unavailable data.
7. Details does not render Bounded Block History, History Gaps, public Validator analytics, or history export. The separate two-summary history request is used only for the current block-interval label; two missing or non-consecutive summaries produce `Unknown`, never a fabricated zero. Missing or failed metric history leaves the current card value intact and renders an explicit chart state.
8. The Network tab presents the selected Node's Public Peer Insight and selected-Node aggregate Peer History. Network Overview presents network-level Peer Insight separately. Neither surface exposes peer addresses or a peer identity list.

The dashboard presents independent observation dimensions. One failed collection must not hide or rewrite another dimension, and one Agent's Nodes must never be merged into an Agent-level chain view.

### Responsive acceptance baseline

The fixed acceptance viewports are 360x800, 390x844, 768x1024, and 1280x800. At widths above 48rem, enabled Geo keeps Peer Insight and the substantive Countries area side by side; Server-disabled Geo switches the composition to one full-width Peer Insight followed by the neutral notice. At 48rem and below, the enabled Peer/Countries areas stack in reading order, and long state, attribution, and identity text wraps without horizontal overflow. Keyboard focus and touch targets remain usable in every project.

- At 1280x800, Home puts the 2x2 statistics block beside the Peer country map in the compact overview (about 42:58) and uses a two-column Node grid. Node Detail uses the current Home content width, keeps its three resource metrics on one row, and lays the four equal metric cards in a two-by-two deck; acceptance checks actual readable width rather than a fixed legacy cap.
- At 768x1024, Home keeps the 2x2 statistics, stacks the Peer country map below them, and uses a single-column Node grid when the content width requires it. Node Detail retains the two-column, equal-height compact metric deck.
- At 360x800 and 390x844, Home keeps the compact 2x2 statistics, stacks the compact (expandable) map below them, uses a single-column Node grid, keeps `Head / Transactions / Peers` together, and reflows the process-resource, Node-data, transfer-rate, and Consensus rows to two columns. Filter pills scroll within their own control rather than causing page overflow. Node Detail keeps the three resource metrics and three status facts in compact rows, reflows consensus into a three-column grid, stacks the four equal-height metric cards, and keeps the tabs full-width touch controls. At 768x1024 the page composition may be single-column, but the shared table-to-priority-card transform begins below 47.9rem; Admin tables therefore retain their table/local-wrapper behavior at the 768 fixture.
- At every viewport, long Node names, Node IDs, Network keys, status reasons, and values wrap or truncate with an accessible full value. No critical state requires primary horizontal page scrolling.
- Touch targets are at least 44x44 CSS pixels. Portrait, landscape, 200% zoom, and reduced-motion settings remain usable.

### State and realtime acceptance

The UI keeps collection state, freshness state, value state, and authorization state independent. It renders the fixed user-facing vocabulary from this document: Starting, Current, Stale, Error, Unknown, Disabled, Unsupported, Empty, Live updates connected, Connecting to live updates, Live updates paused, and You are offline.

- Initial route loads show a meaningful Starting/loading state and do not fabricate values.
- A successful observation may show Current or an authoritative empty value. A successful Peer Snapshot/aggregate of zero is displayed as zero, not Unknown; omitted/unsupported peer collection remains distinct.
- An Error or Stale observation may retain LastGood data, but the UI must show the error/stale reason and age/freshness supplied by the Server. It must never convert Unknown, stale, never-observed, Disabled, or Unsupported into 0, false, or Healthy.
- Node, history, metric-history, peer-history, and validator requests fail independently. A failed optional module does not erase the Node summary or unrelated successful modules.
- A normal SSE invalidation preserves the currently displayed Node and view context while the exact Public resource is refetched. A reset, authorization transition, Node ID change, or access recheck clears affected sensitive projection state before the next render and may show a revalidation state.
- SSE carries invalidation/reset signals only. REST remains authoritative for all displayed business values. A disconnected stream announces Live updates paused; browser-offline state may additionally announce You are offline.
- Retired, deleted, forbidden, or unknown public Nodes use non-leaking unavailable copy and never reveal whether a protected record exists.

### Navigation and accessibility acceptance

- The PlatPulse brand is a keyboard-focusable link to `/`. Its accessible name identifies PlatPulse and its destination is stable from Home and Node Detail.
- The Admin link is a keyboard-focusable link to /admin with a gear icon, visible `Admin` text, and an explicit accessible name. Home does not show other text navigation or a Home logout action in this header.
- Whole-card Node links, Node Detail Network links, the Network back link, and Details/Network tabs are reachable by keyboard in a predictable order. Browser back/forward preserves route context.
- Tabs use tab/list semantics with a single selected tab, a labelled panel, visible focus, and keyboard activation. Switching tabs preserves Node identity and summary state.
- Pages expose one logical h1, ordered headings, semantic lists/tables where appropriate, meaningful empty/error regions, and polite live regions only for meaningful transitions.
- Status uses text plus icon, shape, or an equivalent explanation. Focus rings remain visible against the light canvas and composed surfaces. Reduced motion removes non-essential transitions and does not remove state information.

### Exploration disposition and production boundary

The three throwaway variants (Signal stack, Mission control, Evidence ledger)
remain historical exploration evidence only. Issue #89 completed the cleanup:
the production WebUI contains no variant switcher, prototype route branch, or
prototype-only module, and historical `variant` query parameters do not alter
the production Home or Node Detail routes. The supplied Emerald reference image remains local evidence and is not bundled, imported, or referenced by runtime code. The accepted production contract and its routed regression
coverage are the only supported Home and Node Detail implementation.

### Production seam and test intent

The highest-value external seam is the routed public Home shell and its child page modules, exercised through the typed Public API adapter and a controllable realtime invalidation source. Tests cross this seam with real Public DTO-shaped responses and explicit transport/error transitions; they do not reach into CSS selectors, private helpers, or implementation-only state. The same seam covers Home filtering/sorting, Node Detail hero/metric content and tabs, block-interval derivation, Logo/Admin navigation, independent module failures, reset behavior, and last-good refresh preservation.

SCN-HOME-NODE-DETAIL is expanded with the visual and state assertions above. Add focused scenarios for SCN-HOME-FILTER-SORT, SCN-HOME-NAVIGATION, SCN-NODE-TABS, SCN-NODE-BLOCK-INTERVAL, SCN-NODE-INDEPENDENT-STATES, SCN-NODE-LAST-GOOD-REFRESH, and SCN-HOME-RESPONSIVE-ACCESSIBILITY. Each scenario must assert semantic content at all four fixed viewports; screenshots may supplement but cannot replace those assertions.

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
| `SCN-HOME-GEO-MAP` | 2x2 global statistics beside the transparent Peer country map at 1440/1280 and stacked compact/expanded map on 360/375/390/768; statistics unchanged by filter and sort; Server-owned Geo state, scope, Peer-observation freshness, and basemap state stated separately; per-Node Peer-record basis and accessible country statistics; Disabled provider, unavailable basemap, and retry degradation without losing Home; no raw Peer address, database path, or fabricated marker (see §11.1) |
| `SCN-HOME-NODE-DETAIL` | compact Komari-density Node card with no coloured edge strip, compact PlatON process CPU/process memory/Node Data progress, separate Node status and Validator role, four equal-height compact detail cards containing two 60-second line charts and two bar charts backed by real retained samples, neutral card borders, no rendered Bounded Block History, derived consecutive-block interval, and selected-Node aggregate Peer History in the Network tab |
| `SCN-HOME-UNAVAILABLE-NODE` | non-leaking unavailable copy for retired/unknown; no internal detail |
| `SCN-OVERVIEW-FRESH` | Attention precedes four linked summary cards, priority Node rows remain independently scoped, Agent cards show Host observations once, and Server Health/freshness/timestamps remain authoritative |
| `SCN-OVERVIEW-STALE-LAST-GOOD` | last-good remains, Error/Stale reason and age visible, no zero substitution, and failed refetch does not clear valid REST content |
| `SCN-OVERVIEW-UNKNOWN-UNSUPPORTED` | Unknown/Unsupported/Disabled/Empty remain distinct; Starting grace does not become Offline or premature attention |
| `SCN-OVERVIEW-ATTENTION-GROUPING` | typed critical/warning items remain independent, group by Subject without loss, show issue and Subject counts, preserve safe known-route actions, expose unknown-kind fallback, and label each item's own Server observation time (`Observation time unknown` when absent) without substituting the snapshot time |
| `SCN-OVERVIEW-PARTIAL-FAILURE` | Overview, Nodes, and Agents fail/retry independently; the Overview response keeps Attention and summary together but does not promise a cross-resource point-in-time snapshot |
| `SCN-OVERVIEW-EMPTY-SETUP` | authoritative zero/Empty values remain visible, safe Networks/Settings guidance appears, and no remote setup or fake-data action exists |
| `SCN-OVERVIEW-RESPONSIVE` | fixed 360/390/768/1280 layouts, summary transformation, Node cards, Agent stacking, touch/focus/Escape, 200% zoom, reduced motion, and no primary horizontal overflow |
| `SCN-SITE-ACCESS-PUBLIC` | from `/admin/settings`, switch to Public, allow anonymous Home reads, keep Admin Owner-only, clear affected state, discard stale responses, and record an Audit Event |
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
| Home compact overview (2x2 global statistics + transparent Peer country map), locally hosted Natural Earth basemap with offline generator, map-local degradation, and `SCN-HOME-GEO-MAP` | Issue #133, parent Issue #130; see this document §11.1 |

Changes to a settled contract require a new decision record and must update the affected `PAGE-*`, `PATTERN-*`, and `SCN-*` references together. OpenAPI or Server policy changes do not silently change WebUI semantics; they require an explicit design review when the user-visible contract changes.
