# PlatPulse WebUI Design and Implementation Handoff

**Status:** Accepted MVP design contract; production implementation follows this document.

**Scope:** PlatPulse Home Dashboard and Admin Dashboard for the MVP surface described in `docs/design/platpulse.md`.

**Primary sources:**

- `CONTEXT.md` for domain vocabulary;
- `docs/design/platpulse.md` for Server, Agent, API, security, and deployment boundaries;
- generated OpenAPI artifacts for DTOs, operations, error envelopes, and client behavior.

This document is the WebUI UX and interaction authority for the MVP. It does not replace OpenAPI and does not define Server policy. Deferred surfaces (Validator, Geo, Alerts, Notifications, Retention, Backup/Restore, Doctor, Node Transfer, Agent Recovery/Rotation) have no MVP page contracts; earlier drafts that covered them are superseded. Code comments referencing older section numbers of this document will be reconciled during the implementation-migration pass.

## 1. Purpose and non-goals

PlatPulse WebUI presents operational truth from the Server and gives the Owner safe, audited configuration. Home is the read-only, Node-first monitoring surface — readable by everyone when Site Access Mode is Public and login-required when Private; Admin is the authenticated overview and configuration surface. It is a monitoring and administration surface, not a remote-control terminal.

### 1.1 In scope

- Home Dashboard: read-only Public Projections, Network → Node → Node Detail, including a current block interval derived from the latest two consecutive retained Block Summaries;
- Admin Dashboard: Agent/Node/Network configuration, global history window, Site Access Mode, Sessions, and Audit;
- responsive behavior at 360×800, 390×844, 768×1024, and 1280×800;
- current Peer Count display (Peer Snapshots and Peer Presence Intervals are deferred);
- independent collection, freshness, value, and authorization states;
- REST-authoritative data loading and SSE invalidation;
- accessible forms, tables/cards, confirmations, errors, conflicts, audit links, and session transitions;
- deterministic Playwright-oriented acceptance scenarios.

### 1.2 Out of scope

- Agent, Server, SQLite, or evaluation implementation;
- a duplicated full Node Detail inside Admin;
- RPC Endpoint editing, RPC Endpoint failover, remote commands, restart, upgrade, Docker control, or terminal access;
- TUI, arbitrary scripts, SQL/DSL alert rules, or remote-control UI;
- Validator, Geo, Alerts/Incidents/Silences/Maintenance, Notifications, Retention, Backup/Restore, Doctor, Node Transfer, Recovery/Rotation, multi-tenant, HA, PostgreSQL, SSO/OIDC/TOTP/WebAuthn;
- runtime theme/script injection or a second frontend framework.

## 2. Authorities and vocabulary

Use the exact domain terms in `CONTEXT.md`. MVP-relevant terms include: Host, Agent, PlatON Node, Node ID, RPC Endpoint, Network, Network Identity, Network Registry, Node Inventory, Active Node, Retired Node, Component Observation, Agent Report, Report Receipt, Current Projection, Block Summary, Peer Count Observation, Host Observation, Node Process Observation, Node Chain Observation, Node Observation, Node Health Summary, Attention Item, Public Projection, Site Access Mode, Invalidation Event, Home Dashboard, Admin Dashboard, Audit Event, and Owner.

The WebUI must not invent synonyms that blur boundaries. In particular:

- Home is not “public admin”;
- Node Health Summary is not a WebUI-computed health color;
- an Agent that stops reporting does not retire its Nodes;
- an Agent-level page must not merge independent Node chain observations;
- a successful Peer Count Observation of zero is authoritative, not Unknown;
- recent Block History is bounded by the Server window and is best-effort: missing blocks are simply absent, never synthesized as zeroes or filled with gap fabrications.

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
Live updates paused
You are offline
```

`Online` and `N/A` are not generic replacements. Status communication always includes text and an icon or equivalent explanation; color is supplementary.

## 3. Surfaces and authorization

### 3.1 Home Dashboard

Home is a read-only surface for Public Projections:

```text
Home
├── All Networks
├── Network Overview
│   └── Node list/cards
└── Node Detail
    ├── Health, Validator Activity, and process uptime
    ├── Head, QC, Locked, Committed, and Validator membership
    ├── Process start and last report times
    ├── Process CPU and memory percentages
    ├── Node Data size/capacity progress
    ├── Host network rates and Peer connections
    └── Current block interval and latest transaction count
```

Home is organized Network → PlatON Node, never Agent → Node. Agent and Host topology belongs to Admin.

When Site Access Mode is Public, everyone can read Home without logging in; when Private, Home routes require Owner login. Every Active Node appears on Home; there is no per-Node visibility control.

For retired, deleted, forbidden, or unknown Nodes, Public routes use non-leaking unavailable semantics such as “This Node is no longer available.” Admin routes may distinguish forbidden from not-found.

### 3.2 Admin Dashboard

Admin is authenticated and Owner-authorized for management actions. Owner is the only human principal; there are no Viewer or Guest roles.

Admin groups:

1. Overview;
2. Agents;
3. Nodes;
4. Networks;
5. Settings;
6. Sessions and Audit.

Admin covers configuration and diagnostics; it must not duplicate Home's full Node Detail. The Admin Node page shows Server-owned administrative fields (display name, redacted RPC Endpoint diagnostics, Node Inventory/lifecycle, freshness summary) instead of the full Home observation cards.

Every Admin render begins with `Checking access…` when authorization is unresolved. It never flashes data from a previous session.

The Admin shell shares Home's accepted dark operational visual language: an immersive dark canvas, translucent glass surfaces, soft borders, restrained blur, high-contrast primary text, quiet secondary copy, and indigo/violet accents. The Owner-only shell keeps its management information architecture: a persistent desktop sidebar and an accessible tablet/phone drawer with focus entry, Tab trapping, Escape and scrim close, body scroll lock, and focus restoration. Header and navigation controls remain at least 44×44 CSS pixels.

The first shared-theme proof is `PAGE-ADMIN-OVERVIEW` at `/admin`. It presents the Server-owned attention queue, Node Health Summary, and Agent inventory as independent Admin query/realtime surfaces. Starting, Empty, Error, Stale, last-good, Unknown, never-observed, Disabled, and Unsupported states remain explicit in text plus an icon/shape or equivalent explanation; no state is represented only by color or converted to a zero, false, or Healthy value.

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
| `PAGE-HOME-NETWORKS` | `/` | Network list and public availability | Everyone (Public mode); Owner (Private mode) |
| `PAGE-HOME-NETWORK` | `/networks/:networkKey` | Network overview and Node list/cards | Everyone (Public mode); Owner (Private mode) |
| `PAGE-HOME-NODE` | `/nodes/:nodeId` | Public Node detail and independent observation dimensions | Everyone (Public mode); Owner (Private mode) |
| `PAGE-HOME-UNAVAILABLE` | public fallback route/state | Non-leaking retired/deleted/unknown response | Everyone (Public mode); Owner (Private mode) |

### 4.2 Authentication and access pages

| Page ID | Route | Purpose | Actors |
|---|---|---|---|
| `PAGE-AUTH-LOGIN` | `/login` | Human login with safe `returnTo` | Owner |
| `PAGE-ACCESS-SESSIONS` | `/admin/access/sessions` | Coarse session review and revoke | Owner |
| `PAGE-ACCESS-AUDIT` | `/admin/access/audit` | Immutable redacted Audit review | Owner |
| `PAGE-AUTH-REVOKED` | `/session-revoked` or route-preserving state | Explain access generation transition | Owner/expired |
| `PAGE-AUTH-FORBIDDEN` | protected route state | Explain insufficient access without leaking data | Owner |

### 4.3 Admin pages

| Page ID | Route | Purpose | Actors |
|---|---|---|---|
| `PAGE-ADMIN-OVERVIEW` | `/admin` | Owner attention queue and operational overview | Owner |
| `PAGE-ADMIN-AGENTS` | `/admin/agents` | Agent inventory, liveness, spool diagnostics | Owner |
| `PAGE-ADMIN-AGENT-DETAIL` | `/admin/agents/:agentId` | Identity, credential status, liveness, inventory, diagnostics | Owner |
| `PAGE-ADMIN-NODES` | `/admin/nodes` | Node list, health summary, freshness | Owner |
| `PAGE-ADMIN-NODE-DETAIL` | `/admin/nodes/:nodeId` | Administrative detail and diagnostics (never a duplicate of Home's Node Detail) | Owner |
| `PAGE-ADMIN-NETWORKS` | `/admin/networks` | Network Registry metadata and Nodes | Owner |
| `PAGE-ADMIN-NETWORK-DETAIL` | `/admin/networks/:networkKey` | Expected identity, metadata, mismatch diagnostics | Owner |
| `PAGE-ADMIN-SETTINGS` | `/admin/settings` | Global Block History window and Site Access Mode configuration | Owner |

Deferred groups (later phases only, no MVP page contracts): Agent enrollment/recovery/rotation, Node Transfer, Alerts and Operations, Data and Maintenance.

All mutation routes preserve a safe `returnTo` only after validation: same-origin, expected path, no credentials/secrets, and no external URL. Invalid or absent `returnTo` falls back to the owning list page.

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
- Unknown, stale, never-observed, disabled, and unsupported never render as `0`, `false`, or Healthy;
- authoritative empty is not Unknown;
- recent Block History is bounded by the Server window and best-effort: absent blocks stay absent, never synthetic zeroes;
- host observation is collected once per Agent and referenced by Node views.

### 5.4 Node Health Summary

Severity and primary reasons are Server-owned. The WebUI presents the Summary and dimension reasons; it does not reimplement health policy or merge Node observations at Agent level.

### 5.5 Deferred state machines

Alert evaluation/Incident state machines and long-running Operation states belong to later phases and have no MVP page contracts.

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

SSE connection status is visible but does not cover valid content. Disconnect shows `Live updates paused`; browser/network loss additionally uses `You are offline` when the browser signal is authoritative.

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
| `PATTERN-CONFIRMATION` | High-risk actions use explicit confirmation, typed phrases where required, and no optimistic result. |
| `PATTERN-RESPONSIVE-TABLE` | Desktop table becomes priority cards; detail remains available without primary horizontal scroll. |
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

- the primary Node card uses a compact Komari-inspired density with a neutral one-pixel outline and no coloured edge strip. It shows display name/Node ID, Server-owned Health, explicit Validator Activity (`Observing`, `Verifying`, `Producing`, `Active`, or another canonical activity), process uptime, compact PlatON process CPU/process-memory/Node Data progress, `HEAD / QC / LOCKED / COMMITTED / VALIDATOR`, process start time, and last Agent report time. Routine Healthy prose is omitted; an exceptional Server-owned health reason remains visible;
- Details contains exactly four equal-size one-minute metric cards: shared Host network receive/transmit rates, Peer connections, latest consecutive-block interval, and latest Block Summary transaction count. Network and Connections use line charts; Block time and Transactions use bar charts;
- the bounded Block History list and public-history export are not rendered on Node Detail. The history endpoint remains a Server boundary; the page reads retained summaries only to derive the consecutive-block interval, and missing/non-consecutive summaries remain `Unknown`;
- Network shows Peer Insight and aggregate Peer History only; no peer address or identity list is exposed;
- Host CPU and memory are never substituted for PlatON process CPU and memory, and shared Host network rates are labelled as Host observations;
- retired/deleted/unknown Nodes use the non-leaking unavailable semantics.

### 8.2 Admin Node Detail (`PAGE-ADMIN-NODE-DETAIL`)

- administrative fields only: display name, redacted RPC Endpoint diagnostics, Node Inventory/lifecycle (Active/Retired), freshness summary, and Audit links;
- must not reproduce Home's full observation cards;
- every mutation is audited.

### 8.3 Settings (`PAGE-ADMIN-SETTINGS`)

- renders one Settings heading with ordered History Window and Site Access Mode cards;
- each card loads, mutates, and reports success or errors independently;
- History Window shows the current window, default, min/max bounds, and last update;
- History Window requires an integer in the Server bounds, a successful Server-authoritative impact preview, and typed confirmation before mutation; values are rejected rather than clamped;
- History Window copy states that shortening asynchronously deletes expired history and lengthening cannot recover deleted or missed history; success includes its Audit Event identifier;
- Site Access Mode uses text plus icon/equivalent semantics for Public or Private: Public permits anonymous Home reads, while Private requires Owner login;
- switching Site Access Mode requires confirmation, records Audit, and performs the Public access-generation transition by closing affected streams, aborting old requests, clearing sensitive caches, discarding older responses, and reloading authoritative state;
- the Settings cards stack on narrow viewports, preserve 44×44 CSS pixel targets, and never cause primary horizontal page overflow.

The retired `/admin/history-window` and `/admin/site-access` routes are not redirected; they resolve through the Admin Section not found fallback. This is a page-level SPA outcome and does not guarantee an HTTP 404 response from the Server.

#### 8.3.1 Settings integration acceptance

The Settings route is the single canonical configuration surface. The accepted scenarios map to `/admin/settings` as follows:

| Scenario | Route and outcome |
|---|---|
| `SCN-SETTINGS-ROUTE` | `/admin/settings` renders one logical `Settings` h1, ordered `History Window` then `Site Access Mode` sections, and no obsolete navigation entries; the retired URLs remain on the Admin Section not found fallback without redirecting. |
| `SCN-HISTORY-WINDOW-SHORTEN` | Through the History Window card, show Server bounds and impact, require typed confirmation, report the returned Audit Event, and retain asynchronous deletion consequences. |
| `SCN-HISTORY-WINDOW-BOUNDS` | Through the History Window card, reject blank, non-integer, and out-of-bounds values with field-level errors; never clamp or submit an invalid value. |
| `SCN-SITE-ACCESS-PUBLIC` | Through the Site Access Mode card, confirm and apply Public, clear affected Public state, reload the new authorization generation, and permit anonymous Home reads while Admin remains Owner-only. |
| `SCN-SITE-ACCESS-PRIVATE` | Through the Site Access Mode card, confirm and apply Private, close affected Public streams, clear old Public state, and require Owner login for Home reads. |

Each Settings card keeps independent loading, mutation, success, field-error, page-error, confirmation, and recovery states. Browser back/forward preserves the canonical route context, while the Public/Admin DTO, cache, realtime, and Owner authorization boundaries remain separate.

### 8.4 Overview (`PAGE-ADMIN-OVERVIEW`)

The Overview is the Owner's triage surface, not a second copy of the Nodes or Agents inventory and not a remote-control console. It borrows Komari's compact scanning density, dark operational canvas, restrained borders, strong primary numbers, and quiet secondary copy without importing Komari's database, subscription, traffic-ranking, pricing, latency-probe, or remote-operation model. The accepted order is `Attention -> Summary -> Node Health -> Agent inventory`.

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

`Active Nodes = healthy + unhealthy + unknown` and `total Nodes = active + retired`. Retired Nodes are excluded from live health buckets and Attention Items. A compatibility-only published/visibility count is not a primary Overview metric; Site Access Mode is the site-wide Home authority and every Active Node appears on Home.

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
Current Head / Sync
Resync state
Show diagnostics
View Node
```

The default priority is unhealthy, unknown health, stale, then healthy/current, with stable Network and Node-name tie-breakers. Timestamp-only invalidations do not reorder the list. At most ten Active Nodes are shown, followed by `Showing N of M Active Nodes` and `View all Nodes`. Retired Nodes remain in their summary card and the filtered Nodes inventory, not this live table.

`Show diagnostics` expands RPC, Sync, Consensus, Peers, Process, and Node Data in place; `View Node` opens the administrative Node Detail. Only one Node is expanded at a time, Escape collapses it, and an SSE-driven REST refetch preserves expansion by Node ID while that Node remains in the rendered set. Expansion shows collection, value, freshness, LastGood, time, and safe error context; it does not reproduce Home charts or the complete Home Node Detail.

#### 8.4.4 Agent inventory

Each Agent card represents one Agent and its one Host. Host Observation is shown once on that card and is never copied into every Node. The compact Node rows are joined from the already-loaded Admin Node list by stable `agent_id`; they preserve each Node's independent state rather than creating an Agent-level chain aggregate. The compact card may show:

```text
Agent identity and liveness
Last accepted report and sequence
Active / unhealthy / unknown Node counts
Compact Host CPU and memory values
Durable Spool queued reports, capacity, overflow, and fatal state
Clock status
Report sequence gaps and security-event count
Separate compact rows for the Agent's Nodes
View Agent
```

Unknown Host CPU, memory, or Spool values remain Unknown rather than zero. CPU and memory may use small current-value progress tracks, but Overview has no Host rankings, 24-hour charts, or duplicated Host metrics. Normal Spool state stays quiet; fatal storage, discarded reports, delivery backlog, and other exceptional states are prominent. Boot IDs, complete credential state, shutdown evidence, complete Host observations, and detailed diagnostics belong to Agent Detail.

The default priority is Agents with critical diagnostics, then offline, unknown, and online Agents, with stable Agent ID tie-breaking. At most six cards are shown, followed by `View all Agents`.

#### 8.4.5 Data, time, and partial states

The page retains three independent Admin query surfaces:

```text
GET /api/admin/v1/overview -> Attention and the atomic summary snapshot
GET /api/admin/v1/nodes    -> Node Health Summary
GET /api/admin/v1/agents   -> Agent inventory and Host diagnostics
```

Failure of one surface never hides successful data from another. Attention and summary fail together because they are one atomic Overview snapshot. Initial loading uses explicit `Starting` text, optionally accompanied by static skeleton shapes. A refetch failure preserves LastGood content and says that the last successful values remain visible. Each failed surface owns its own `Try again`. SSE carries only invalidation/reset; `Live updates paused` or `You are offline` never clears valid REST content.

Visible timestamps are relative, such as `2 minutes ago`, with an accessible absolute UTC value. Agent reports may show `Report #128 - 5 seconds ago`. The WebUI formats Server timestamps but never uses browser time to derive Freshness, liveness, grace periods, Health, or Attention severity.

When Agents, Nodes, and Networks are all empty, the page retains authoritative zero and Empty states and adds a compact setup guide: register the expected Network identity, provision and start an Agent, configure its local Node Inventory, and wait for the first accepted Agent Report. It may link to Networks and Settings, but it cannot configure an RPC Endpoint, start an Agent, create a local Node Inventory, expose an Enrollment workflow, enable fake data, or offer one-click initialization.

#### 8.4.6 Page boundaries

Overview owns triage, compact counts, priority subsets, and safe navigation. The Nodes page owns the full inventory, lifecycle/Network/health/freshness filters, stable sorting, inventory revision, identity disposition, redacted RPC Endpoint diagnostics, metadata editing, and Audit links. Admin Node Detail owns full administrative Component diagnostics and must not reproduce Home's observation-card and chart deck.

The Agents page owns the complete Agent inventory, epoch, boot/report state, Node Inventory, credential state, Spool diagnostics, clock diagnostics, sequence gaps, and security events. Agent Detail owns full identity, credential, Host, Inventory, diagnostic evidence, and Audit context. Overview never displays complete credentials, complete RPC Endpoints, Boot IDs, internal paths, stack traces, or remote controls.

#### 8.4.7 Responsive and visual acceptance

The visual balance is PlatPulse's dark operational system first and Komari-inspired density second: near-black translucent surfaces, neutral one-pixel borders, restrained 10-14px radii, reduced blur and large shadows, high-contrast counts, quiet labels, indigo/violet navigation accents, and semantic green/amber/red/neutral status treatments. No status depends on color alone. The production UI remains English for the MVP; localization is a separate whole-application capability rather than a mixed-language Overview.

At `1280x800`, the Admin sidebar is persistent, Attention and Node Health are full-width, summary cards form four columns, and Agent cards form two columns. At `768x1024`, navigation uses the accessible drawer, summary cards form a two-by-two grid, and Node/Agent content is single-column. At `360x800` and `390x844`, summary cards remain a compact two-by-two grid when legible and may fall to one column when content requires it; Node tables become priority cards and Agent cards stack. Health/Freshness and Head/Sync remain paired, secondary evidence moves into expansion, controls remain at least 44x44 CSS pixels, and no primary horizontal page scrolling is allowed. The page remains functional at 200% zoom, in portrait and landscape, and with reduced motion.

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

## 9. Content, privacy, and redaction

The WebUI must not display or store in browser state:

- Agent credentials or any one-time provisioning material;
- session tokens or CSRF values in URLs;
- passwords, TLS private keys, or pepper values;
- raw Peer addresses (Peer Snapshots are deferred; no peer identity data exists in MVP);
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
- browser back/forward preserves URL filters and detail context.

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

The accepted direction from Issue #75 and the compact Home contract from Issue #97 are the production visual baseline for the public Home surface. It borrows the supplied Nezha references' operational hierarchy and dark glass treatment without importing their server, pricing, or remote-control data model.

### Visual language

- Home and public Node Detail use a dark, immersive shell with near-black translucent panels, soft borders, rounded corners, restrained blur, and indigo/violet accents.
- The visual treatment is subordinate to operational truth. Decorative artwork or gradients may sit behind the shell, but the interface remains readable when the artwork is absent, blocked, or reduced.
- Primary numbers and page titles use high contrast and strong weight. Secondary labels, timestamps, identifiers, and explanatory copy are visibly quieter.
- Green, amber, red, violet/indigo, and neutral tones communicate good, attention, error, contextual accent, and unavailable/unknown states respectively. Every status also has text or an equivalent accessible explanation; color is never the sole signal.
- Cards, pills, separators, progress bars, and focus states share one spacing and radius system. Hover elevation is optional decoration and must not be required to discover an action.
- The visual contract does not authorize fields that are absent from the Public Projection. Node Detail may show the monitored PlatON process CPU/memory, process start/uptime, last Agent report time, Node Data usage/capacity, and sampled Host network receive/transmit rates supplied by the Server; it does not add pricing, raw Peer identity, Host identity, or RPC Endpoint text.

### Home composition (PAGE-HOME-NETWORKS and PAGE-HOME-NETWORK)

The Home route is a read-only operational overview composed in this order:

1. A compact header with the PlatPulse brand link at left and one circular Admin icon link at right. The brand returns to Home; the Admin icon enters the Admin route and does not expose Admin data inside Home.
2. A page kicker, the Home heading, explanatory Public Projection copy, and a server-authoritative live/realtime indicator.
3. Four summary cards for Active Node count, Server-owned healthy Node count, Nodes needing attention, and registered Network count. These are projections of already-loaded Public data; they are not new health policy or per-Node visibility.
4. A toolbar containing Network filter pills and a clearly labelled sort control. Unsupported future views are not rendered as usable production actions.
5. A responsive collection of compact Active Node cards. Each whole card is one semantic link to Node Detail; the Network name is plain text and the card contains no nested Network link. The header shows Node identity, Validator Activity, and Node Health without `ACTIVITY` or `HEALTH` labels; missing Validator Activity is `Observing`. Healthy Nodes omit routine component rows, health prose, Last Observed, and no-op Resync copy; an exceptional Node may show one short diagnostic line.
6. Each compact card presents three ordered metric rows: sanitized Host `CPU / MEMORY / STORAGE / ↑ UP / ↓ DOWN`; Node `HEAD / TXS / PEERS`; and Consensus `QC / LOCKED / COMMITTED / VALIDATOR`. HEAD remains Sync Current Head, TXS is the transaction count from that Node's latest persisted Block Summary, and PEERS retains its independent observation-state cue. Missing values remain unavailable rather than becoming zero.
7. An explicit empty state when the selected Network has no Active Nodes, without implying that missing data is zero or healthy.

Network hierarchy remains Network -> PlatON Node -> Node Detail. Home never reorganizes the view around Agent or Host topology.

### Node Detail composition (PAGE-HOME-NODE)

Node Detail freezes the accepted reference-inspired hierarchy:

1. One compact Komari-inspired Node card owns the page identity. It uses a quiet neutral outline, restrained radius and shadow, no coloured top/edge strip, and shows the Network back context, display name/Node ID, Server-owned Health, canonical Validator Activity, and process uptime. Routine Healthy prose is omitted, while exceptional Server-owned health reasons remain visible.
2. A compact resource row inside that card follows the Home Node-card hierarchy: PlatON process `CPU`, PlatON process `MEMORY`, and `NODE DATA`, each with a current value and a progress track when a valid percentage exists. Node Data keeps its size and filesystem-capacity detail; unavailable values remain explicit rather than becoming zero.
3. A chain-specific consensus runway presents `HEAD / QC / LOCKED / COMMITTED / VALIDATOR`; Validator membership renders the explicit boolean text `True` or `False`. Process start time and last Agent report time form the card footer. Missing or uncertified values remain `Unknown`.
4. A centred two-tab control defaults to Details and switches to Network without replacing the large Node card. The selected tab is exposed semantically and visually.
5. Details presents four equal-size, compact cards in this order: Host network upload/download rates, Peer connections, block interval (`latest block timestamp - previous consecutive block timestamp`), and latest Block Summary transaction count. Every card keeps its current value and a labelled 60-second chart with `1m` and `0s` time bounds; reduced padding, chart height, radius and shadow keep the deck close to Komari's information density.
6. Network renders upload and download as distinct line series; Connections renders inbound and outbound as distinct line series. Block interval and transaction count use Server-retained Block Summary samples as bars. The Server may include one last-good point immediately before the window as a line chart's starting value, but bar charts render only observations inside the window; neither Server nor WebUI fabricates intermediate samples or substitutes zero for unavailable data.
7. Details does not render Bounded Block History, History Gaps, public Validator analytics, or history export. The separate two-summary history request is used only for the current block-interval label; two missing or non-consecutive summaries produce `Unknown`, never a fabricated zero. Missing or failed metric history leaves the current card value intact and renders an explicit chart state.
8. Network presents the Public Peer Insight and Public Peer History modules. It never exposes peer addresses or a peer identity list.

The dashboard presents independent observation dimensions. One failed collection must not hide or rewrite another dimension, and one Agent's Nodes must never be merged into an Agent-level chain view.

### Responsive acceptance baseline

The fixed acceptance viewports are 360x800, 390x844, 768x1024, and 1280x800.

- At 1280x800, Home uses four summary columns and a two-column Node grid. Node Detail centres its compact card deck within a 68rem maximum width, keeps its three resource metrics on one row, and lays the four equal metric cards in a two-by-two deck.
- At 768x1024, Home uses two summary columns and a single-column Node grid when the content width requires it. Node Detail retains the two-column, equal-height compact metric deck.
- At 360x800 and 390x844, Home keeps a compact two-column summary where it remains legible, uses a single-column Node grid, keeps `HEAD / TXS / PEERS` together, and reflows the four-column Host-resource and Consensus rows to two columns. Filter pills scroll within their own control rather than causing page overflow. Node Detail keeps the three resource metrics and three status facts in compact rows, reflows consensus into a three-column grid, stacks the four equal-height metric cards, and keeps the tabs full-width touch controls.
- At every viewport, long Node names, Node IDs, Network keys, status reasons, and values wrap or truncate with an accessible full value. No critical state requires primary horizontal page scrolling.
- Touch targets are at least 44x44 CSS pixels. Portrait, landscape, 200% zoom, and reduced-motion settings remain usable.

### State and realtime acceptance

The UI keeps collection state, freshness state, value state, and authorization state independent. It renders the fixed user-facing vocabulary from this document: Starting, Current, Stale, Error, Unknown, Disabled, Unsupported, Empty, Live updates paused, and You are offline.

- Initial route loads show a meaningful Starting/loading state and do not fabricate values.
- A successful observation may show Current or an authoritative empty value. A successful Peer Count Observation of zero is displayed as zero, not Unknown.
- An Error or Stale observation may retain LastGood data, but the UI must show the error/stale reason and age/freshness supplied by the Server. It must never convert Unknown, stale, never-observed, Disabled, or Unsupported into 0, false, or Healthy.
- Node, history, metric-history, peer-history, and validator requests fail independently. A failed optional module does not erase the Node summary or unrelated successful modules.
- A normal SSE invalidation preserves the currently displayed Node and view context while the exact Public resource is refetched. A reset, authorization transition, Node ID change, or access recheck clears affected sensitive projection state before the next render and may show a revalidation state.
- SSE carries invalidation/reset signals only. REST remains authoritative for all displayed business values. A disconnected stream announces Live updates paused; browser-offline state may additionally announce You are offline.
- Retired, deleted, forbidden, or unknown public Nodes use non-leaking unavailable copy and never reveal whether a protected record exists.

### Navigation and accessibility acceptance

- The PlatPulse brand is a keyboard-focusable link to `/`. Its accessible name identifies PlatPulse and its destination is stable from Home and Node Detail.
- The circular Admin icon is a keyboard-focusable link to /admin with an explicit accessible name such as Open Admin login. Home does not show text navigation or a Home logout action in this header.
- Whole-card Node links, Node Detail Network links, the Network back link, and Details/Network tabs are reachable by keyboard in a predictable order. Browser back/forward preserves route context.
- Tabs use tab/list semantics with a single selected tab, a labelled panel, visible focus, and keyboard activation. Switching tabs preserves Node identity and summary state.
- Pages expose one logical h1, ordered headings, semantic lists/tables where appropriate, meaningful empty/error regions, and polite live regions only for meaningful transitions.
- Status uses text plus icon, shape, or an equivalent explanation. Focus rings remain visible against the dark shell. Reduced motion removes non-essential transitions and does not remove state information.

### Exploration disposition and production boundary

The three throwaway variants (Signal stack, Mission control, Evidence ledger)
remain historical exploration evidence only. Issue #89 completed the cleanup:
the production WebUI contains no variant switcher, prototype route branch, or
prototype-only module, and historical `variant` query parameters do not alter
the production Home or Node Detail routes. The untracked Nezha reference
images remain local evidence and are not bundled, imported, or referenced by
runtime code. The accepted production contract and its routed regression
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
| `SCN-AUTH-OWNER-LOGIN` | safe return, checking state, success, no password in URL/history |
| `SCN-AUTH-SESSION-REVOKED` | old stream closes, Admin data clears, no stale flash, login/revalidation path |
| `SCN-SITE-ACCESS-PRIVATE` | from `/admin/settings`, switch to Private, close public streams, require Home login, clear old Public cache, preserve Admin Owner-only access, and record an Audit Event |
| `SCN-SETTINGS-ROUTE` | canonical `/admin/settings` route, one h1, History Window before Site Access Mode, Settings-only navigation, removed-route fallback, and browser back/forward context |
| `SCN-HOME-NETWORK-LIST` | network list from Public Projection, all Active Nodes visible, anonymous access follows Site Access Mode |
| `SCN-HOME-NODE-DETAIL` | compact Komari-density Node card with no coloured edge strip, compact process CPU/process memory/Node Data progress, four equal-height compact detail cards containing two 60-second line charts and two bar charts backed by real retained samples, neutral card borders, no rendered Bounded Block History, derived consecutive-block interval, Peer Count only |
| `SCN-HOME-UNAVAILABLE-NODE` | non-leaking unavailable copy for retired/unknown; no internal detail |
| `SCN-OVERVIEW-FRESH` | Attention precedes four linked summary cards, priority Node rows remain independently scoped, Agent cards show Host observations once, and Server Health/freshness/timestamps remain authoritative |
| `SCN-OVERVIEW-STALE-LAST-GOOD` | last-good remains, Error/Stale reason and age visible, no zero substitution, and failed refetch does not clear valid REST content |
| `SCN-OVERVIEW-UNKNOWN-UNSUPPORTED` | Unknown/Unsupported/Disabled/Empty remain distinct; Starting grace does not become Offline or premature attention |
| `SCN-OVERVIEW-ATTENTION-GROUPING` | typed critical/warning items remain independent, group by Subject without loss, show issue and Subject counts, preserve safe known-route actions, and expose unknown-kind fallback |
| `SCN-OVERVIEW-PARTIAL-FAILURE` | Overview, Nodes, and Agents fail/retry independently while Attention and summary remain one atomic snapshot |
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
| Admin visual shell and responsive baseline | Issue #35, accepted prototype branch `prototype/ui-shell-variants` |
| Home/Admin route and scope boundaries | Issue #34 |
| Shared freshness, realtime, and authorization | Issue #36 |
| Identity, lifecycle, access, and workflow contracts | Issue #37 — MVP subset retained (preconfigured credentials); Recovery/Rotation deferred |
| Alert, maintenance, retention, backup/restore, Doctor | Issue #38 — superseded for MVP: deferred to later phases |
| Representative operations-loop prototype | Issue #39, branch `prototype/phase2-operations-loop` @ `58d6f9c` — deferred |
| Implementation handoff and acceptance contract | Issue #40 |
| MVP scope convergence: Komari-like Home/Admin separation, no Admin duplicate Node Detail, Server-only bounded Block History, no History Gap/Backfill, report-level Receipt, Peer Count only | Confirmed design review; see `docs/design/platpulse.md` |
| Site-level access mode (Komari-like), Owner-only principals, per-Node visibility removed | Confirmed design review; see `docs/design/platpulse.md` |
| Accepted Home / Node Detail visual direction, responsive baseline, public-data contract, and production test seam | Issue #75 and accepted branch `prototype/home-node-detail` |
| Unified dark Admin shell and Overview shared-theme foundation | Issue #110 |
| Unified Owner Settings page for History Window and Site Access Mode | Issue #111 |
| Admin visual convergence across retained pages | Issue #112 |
| Unified Admin experience integration contract, canonical Settings route, and fixed-viewport verification | Issue #113, parent Issue #109 |
| Komari-inspired Admin Overview triage hierarchy, typed Attention Items, shared Server Health policy, responsive limits, and page boundaries | Confirmed `grill-with-docs` design review; see `docs/design/platpulse.md` §8.5 and this document §8.4 |
| Prototype cleanup and production-only route boundary | Issue #89 |

Changes to a settled contract require a new decision record and must update the affected `PAGE-*`, `PATTERN-*`, and `SCN-*` references together. OpenAPI or Server policy changes do not silently change WebUI semantics; they require an explicit design review when the user-visible contract changes.
