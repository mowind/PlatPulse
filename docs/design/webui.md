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

- Home Dashboard: read-only Public Projections, Network → Node → Node Detail, including recent Server-side Block History;
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

Use the exact domain terms in `CONTEXT.md`. MVP-relevant terms include: Host, Agent, PlatON Node, Node ID, RPC Endpoint, Network, Network Identity, Network Registry, Node Inventory, Active Node, Retired Node, Component Observation, Agent Report, Report Receipt, Current Projection, Block Summary, Peer Count Observation, Host Observation, Node Process Observation, Node Chain Observation, Node Observation, Node Health Summary, Public Projection, Site Access Mode, Invalidation Event, Home Dashboard, Admin Dashboard, Audit Event, and Owner.

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
    ├── Health and freshness
    ├── Current head, sync, consensus
    ├── Recent Block History (Server window)
    ├── Process summary
    ├── Sanitized Host percentages
    └── Peer Count
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
5. History Window;
6. Site Access, Sessions, and Audit.

Admin covers configuration and diagnostics; it must not duplicate Home's full Node Detail. The Admin Node page shows Server-owned administrative fields (display name, redacted RPC Endpoint diagnostics, Node Inventory/lifecycle, freshness summary) instead of the full Home observation cards.

Every Admin render begins with `Checking access…` when authorization is unresolved. It never flashes data from a previous session.

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
| `PAGE-ADMIN-HISTORY-WINDOW` | `/admin/history-window` | Global Block History window with safe bounds | Owner |
| `PAGE-ADMIN-SITE-ACCESS` | `/admin/site-access` | Site Access Mode (Public/Private) with confirmation and Audit | Owner |

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

- shows independent collection/freshness/value states per component, plus the Server-owned Node Health Summary and its dimension reasons;
- Block History is bounded by the Server window: missing blocks are absence, not zero; the window boundary is visible when relevant;
- the Peer section shows the Peer Count Observation only (count, freshness, and last-good age on error); no peer list and no Peer Presence;
- Host percentages are sanitized and shared, never duplicated per Node;
- retired/deleted/unknown Nodes use the non-leaking unavailable semantics.

### 8.2 Admin Node Detail (`PAGE-ADMIN-NODE-DETAIL`)

- administrative fields only: display name, redacted RPC Endpoint diagnostics, Node Inventory/lifecycle (Active/Retired), freshness summary, and Audit links;
- must not reproduce Home's full observation cards;
- every mutation is audited.

### 8.3 History Window (`PAGE-ADMIN-HISTORY-WINDOW`)

- shows the current window, its default, and its min/max bounds;
- mutation requires confirmation, and the Server records old/new values plus actor in Audit;
- copy states the consequences: changes apply immediately; shortening asynchronously deletes expired history; lengthening cannot recover already deleted or missed data;
- out-of-bounds values are rejected by the Server and shown as field errors, never clamped silently.

### 8.4 Overview (`PAGE-ADMIN-OVERVIEW`)

- prioritizes an attention queue, Server-owned Node Health Summary, freshness, and next actions;
- independent panels may fail independently;
- an Agent monitoring multiple Nodes shows separate Node rows/cards; Host metrics are not duplicated.

### 8.5 Site Access Mode (`PAGE-ADMIN-SITE-ACCESS`)

- shows the current mode and its consequences: Public lets everyone read Home without login; Private requires Owner login;
- switching requires confirmation, and the Server records old/new values plus actor in Audit;
- a mode change is an access-generation transition: close old streams, abort requests, clear affected caches, reload under the new mode;
- Admin is never anonymous; the mode gates Home only.

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
SCN-SITE-ACCESS-PRIVATE
SCN-HOME-NETWORK-LIST
SCN-HOME-NODE-DETAIL
SCN-HOME-UNAVAILABLE-NODE
SCN-OVERVIEW-FRESH
SCN-OVERVIEW-STALE-LAST-GOOD
SCN-OVERVIEW-UNKNOWN-UNSUPPORTED
SCN-SITE-ACCESS-PUBLIC
SCN-HISTORY-WINDOW-SHORTEN
SCN-HISTORY-WINDOW-BOUNDS
```

Scenario state is memory-only. No credentials, secrets, production API origins, local persistence, or prototype-only branches are allowed in production pages.

## 11.1 Accepted Home and Node Detail visual contract (Issue #75)

The accepted direction from Issue #75 and the prototype/home-node-detail branch is the production visual baseline for the public Home surface. It borrows the supplied Nezha references' operational hierarchy and dark glass treatment without importing their server, resource, pricing, or remote-control data model.

### Visual language

- Home and public Node Detail use a dark, immersive shell with near-black translucent panels, soft borders, rounded corners, restrained blur, and indigo/violet accents.
- The visual treatment is subordinate to operational truth. Decorative artwork or gradients may sit behind the shell, but the interface remains readable when the artwork is absent, blocked, or reduced.
- Primary numbers and page titles use high contrast and strong weight. Secondary labels, timestamps, identifiers, and explanatory copy are visibly quieter.
- Green, amber, red, violet/indigo, and neutral tones communicate good, attention, error, contextual accent, and unavailable/unknown states respectively. Every status also has text or an equivalent accessible explanation; color is never the sole signal.
- Cards, pills, separators, progress bars, and focus states share one spacing and radius system. Hover elevation is optional decoration and must not be required to discover an action.
- The visual contract does not authorize fields that are absent from the Public Projection. In particular, it does not add memory, disk, bandwidth, traffic, uptime, pricing, geography, raw Peer identity, or RPC Endpoint text.

### Home composition (PAGE-HOME-NETWORKS and PAGE-HOME-NETWORK)

The Home route is a read-only operational overview composed in this order:

1. A compact header with the PlatPulse brand link at left and one circular Admin icon link at right. The brand returns to Home; the Admin icon enters the Admin route and does not expose Admin data inside Home.
2. A page kicker, the Home heading, a server-authoritative live/realtime indicator, and a current-clock presentation that is decorative context rather than domain freshness.
3. Four summary cards for published Node count, Server-owned healthy Node count, Nodes needing attention, and published Network count. These are projections of already-loaded Public data; they are not new health policy.
4. A toolbar containing the supported card view, Network filter pills, and a clearly labelled sort control. Unsupported future views are not rendered as usable production actions.
5. A responsive collection of Active Node cards. Each card links to Node Detail and may link back to its Network. Cards show only the Public Node fields needed for Network, identity, Node Health Summary, RPC/Sync/Consensus/Process/Resync state, Current Head, history boundary, Peer Count Observation, freshness/value cues, and sanitized Host CPU percentage.
6. An explicit empty state when the selected Network has no published Nodes, without implying that missing data is zero or healthy.

Network hierarchy remains Network -> PlatON Node -> Node Detail. Home never reorganizes the view around Agent or Host topology.

### Node Detail composition (PAGE-HOME-NODE)

Node Detail freezes the accepted reference-inspired hierarchy:

1. The page heading identifies the PlatON Node, shows the Server-owned health state, and shows freshness/live context without replacing the health contract.
2. A summary panel provides a Network back link, Network identity, display name/Node ID, public-history export, six independent facts (Health, RPC, Sync, Consensus, Process, Resync), six independent observations (Current Head, History Boundary, Network Reference, Reference Confidence, sanitized Host CPU, Peer Count), and the Server-owned health reason.
3. A centred two-tab control defaults to Details and switches to Network without replacing the heading or summary panel. The selected tab is exposed semantically and visually.
4. Details presents current-observation signal cards for Host CPU, Current Head, History Boundary, Peers, RPC, and Consensus, followed by bounded Server-side Block History and any available Public Validator insight/analytics.
5. Network presents the Public Peer Insight and Public Peer History modules. It never exposes peer addresses or a peer identity list.
6. Block History shows the Server window's published rows, its best-effort nature, an explicit empty state, and public export. Missing blocks remain absent; the WebUI never synthesizes zero rows or gap evidence.

The dashboard presents independent observation dimensions. One failed collection must not hide or rewrite another dimension, and one Agent's Nodes must never be merged into an Agent-level chain view.

### Responsive acceptance baseline

The fixed acceptance viewports are 360x800, 390x844, 768x1024, and 1280x800.

- At 1280x800, Home uses four summary columns and a two-column Node grid. Node Detail uses six-column facts/observations and a three-column signal grid.
- At 768x1024, Home uses two summary columns and a single-column Node grid when the content width requires it. Node Detail uses three-column facts/observations and a two-column signal grid.
- At 360x800 and 390x844, Home keeps a compact two-column summary where it remains legible, uses a single-column Node grid, and allows filter pills to scroll within their own control rather than causing page overflow. Node Detail stacks the heading, summary actions, facts, observations, signal cards, and secondary panels; the two tabs remain full-width touch controls.
- At every viewport, long Node names, Node IDs, Network keys, status reasons, and values wrap or truncate with an accessible full value. No critical state requires primary horizontal page scrolling.
- The Block History table becomes priority rows/cards on phone widths. If a table representation is retained at a larger width, it must not force the phone page wider than the viewport.
- Touch targets are at least 44x44 CSS pixels. Portrait, landscape, 200% zoom, and reduced-motion settings remain usable.

### State and realtime acceptance

The UI keeps collection state, freshness state, value state, and authorization state independent. It renders the fixed user-facing vocabulary from this document: Starting, Current, Stale, Error, Unknown, Disabled, Unsupported, Empty, Live updates paused, and You are offline.

- Initial route loads show a meaningful Starting/loading state and do not fabricate values.
- A successful observation may show Current or an authoritative empty value. A successful Peer Count Observation of zero is displayed as zero, not Unknown.
- An Error or Stale observation may retain LastGood data, but the UI must show the error/stale reason and age/freshness supplied by the Server. It must never convert Unknown, stale, never-observed, Disabled, or Unsupported into 0, false, or Healthy.
- Node, history, peer-history, and validator requests fail independently. A failed optional module does not erase the Node summary or unrelated successful modules.
- A normal SSE invalidation preserves the currently displayed Node and view context while the exact Public resource is refetched. A reset, authorization transition, Node ID change, or access recheck clears affected sensitive projection state before the next render and may show a revalidation state.
- SSE carries invalidation/reset signals only. REST remains authoritative for all displayed business values. A disconnected stream announces Live updates paused; browser-offline state may additionally announce You are offline.
- Retired, deleted, forbidden, or unknown public Nodes use non-leaking unavailable copy and never reveal whether a protected record exists.

### Navigation and accessibility acceptance

- The PlatPulse brand is a keyboard-focusable link to `/`. Its accessible name identifies PlatPulse and its destination is stable from Home and Node Detail.
- The circular Admin icon is a keyboard-focusable link to /admin with an explicit accessible name such as Open Admin login. Home does not show text navigation or a Home logout action in this header.
- Node cards, Network links, the Network back link, history export, and Details/Network tabs are reachable by keyboard in a predictable order. Browser back/forward preserves route context.
- Tabs use tab/list semantics with a single selected tab, a labelled panel, visible focus, and keyboard activation. Switching tabs preserves Node identity and summary state.
- Pages expose one logical h1, ordered headings, semantic lists/tables where appropriate, meaningful empty/error regions, and polite live regions only for meaningful transitions.
- Status uses text plus icon, shape, or an equivalent explanation. Focus rings remain visible against the dark shell. Reduced motion removes non-essential transitions and does not remove state information.
- Export reports success through the browser download flow and reports a safe task-level error without leaking internal paths or response bodies.

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

The highest-value external seam is the routed public Home shell and its child page modules, exercised through the typed Public API adapter and a controllable realtime invalidation source. Tests cross this seam with real Public DTO-shaped responses and explicit transport/error transitions; they do not reach into CSS selectors, private helpers, or implementation-only state. The same seam covers Home filtering/sorting, Node Detail tabs, bounded history and export, Logo/Admin navigation, independent module failures, reset behavior, and last-good refresh preservation.

SCN-HOME-NODE-DETAIL is expanded with the visual and state assertions above. Add focused scenarios for SCN-HOME-FILTER-SORT, SCN-HOME-NAVIGATION, SCN-NODE-TABS, SCN-NODE-HISTORY-EXPORT, SCN-NODE-INDEPENDENT-STATES, SCN-NODE-LAST-GOOD-REFRESH, and SCN-HOME-RESPONSIVE-ACCESSIBILITY. Each scenario must assert semantic content at all four fixed viewports; screenshots may supplement but cannot replace those assertions.

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
| `SCN-SITE-ACCESS-PRIVATE` | switch to Private closes public streams, Home requires login, old public cache cleared, audit row |
| `SCN-HOME-NETWORK-LIST` | network list from Public Projection, all Active Nodes visible, anonymous access follows Site Access Mode |
| `SCN-HOME-NODE-DETAIL` | independent dimensions, bounded Block History, Peer Count only, sanitized Host percentages |
| `SCN-HOME-UNAVAILABLE-NODE` | non-leaking unavailable copy for retired/unknown; no internal detail |
| `SCN-OVERVIEW-FRESH` | independent Node rows, Server Health Summary, current timestamps |
| `SCN-OVERVIEW-STALE-LAST-GOOD` | last-good remains, Error/Stale reason and age visible, no zero substitution |
| `SCN-OVERVIEW-UNKNOWN-UNSUPPORTED` | Unknown/Unsupported/Disabled/Empty remain distinct |
| `SCN-SITE-ACCESS-PUBLIC` | switch to Public allows anonymous Home reads, Admin still requires Owner login, no admin data leaks, audit row |
| `SCN-HISTORY-WINDOW-SHORTEN` | confirmation, old/new shown, expired history removed asynchronously, audit row |
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
| Prototype cleanup and production-only route boundary | Issue #89 |

Changes to a settled contract require a new decision record and must update the affected `PAGE-*`, `PATTERN-*`, and `SCN-*` references together. OpenAPI or Server policy changes do not silently change WebUI semantics; they require an explicit design review when the user-visible contract changes.
