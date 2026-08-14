# PlatPulse WebUI Design and Implementation Handoff

**Status:** Accepted design contract; production implementation follows this document.

**Scope:** PlatPulse Home Dashboard and Admin Dashboard before Phase 2 implementation.

**Primary sources:**

- `CONTEXT.md` for domain vocabulary;
- `docs/design/platpulse.md` for Server, Agent, API, security, and deployment boundaries;
- generated OpenAPI artifacts for DTOs, operations, error envelopes, and client behavior;
- resolved Wayfinder issues #34–#40 for user-confirmed decisions;
- prototype reference branch `prototype/phase2-operations-loop`, commit `58d6f9c`, for visual/state review only.

This document is the WebUI UX and interaction authority. It does not replace OpenAPI, does not define Server policy, and does not turn the throwaway prototype into production code.

## 1. Purpose and non-goals

PlatPulse WebUI presents operational truth from the Server and gives authorized Owners safe, auditable workflows. It is a monitoring and administration surface, not a remote-control terminal.

### 1.1 In scope

- Home Dashboard: read-only Public Projections for Networks and PlatON Nodes;
- Admin Dashboard: Owner-first operations workbench;
- responsive behavior at 360×800, 390×844, 768×1024, and 1280×800;
- independent collection, freshness, value, authorization, and operation states;
- REST-authoritative data loading and SSE invalidation;
- identity, lifecycle, access, alert, maintenance, retention, backup/restore, and Doctor workflows;
- accessible forms, tables/cards, confirmations, errors, conflicts, audit links, and session transitions;
- deterministic Playwright-oriented acceptance scenarios.

### 1.2 Out of scope

- Agent, Server, SQLite, alert evaluator, notification provider, backup, or Doctor implementation;
- endpoint editing, endpoint failover, remote commands, restart, upgrade, Docker control, or terminal access;
- TUI, arbitrary scripts, SQL/DSL alert rules, network actions, or remote-control UI;
- Peer/Geo, Validator, multi-tenant, HA, PostgreSQL, SSO/OIDC/TOTP/WebAuthn unless represented as an explicit Later/Unsupported state;
- runtime theme/script injection or a second frontend framework.

## 2. Authorities and vocabulary

Use the exact domain terms in `CONTEXT.md`: Host, Agent, PlatON Node, Network, Alert Rule, Alert Incident, Notification Event, Notification Delivery, Silence, Maintenance Window, Owner, Viewer, and Guest.

The WebUI must not invent synonyms that blur boundaries. In particular:

- Home is not “public admin”;
- Node Health Summary is not a WebUI-computed health color;
- Silence is not disabling a Rule or resolving an Incident;
- Maintenance Window is not a Health override;
- Notification Delivery is not an exactly-once message;
- Agent Offline is not Node Retired;
- an Agent-level page must not merge independent Node chain observations.

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
    ├── Block and transactions
    ├── Sync and consensus
    ├── Process summary
    ├── Sanitized Host percentages
    ├── Peer summaries
    └── Validator summary
```

Home is organized Network → PlatON Node, never Agent → Node. Agent and Host topology belongs to Admin.

For private, retired, deleted, forbidden, or unknown Nodes, Public routes use non-leaking unavailable semantics such as “This Node is no longer available.” Admin routes may distinguish forbidden from not-found.

### 3.2 Admin Dashboard

Admin is authenticated and Owner-authorized for management actions. Viewer may use authorized read-only surfaces where explicitly permitted, but cannot mutate users or security state. Guest never accesses Admin.

Admin groups:

1. Overview;
2. Agents;
3. Nodes;
4. Networks;
5. Alerts and Operations;
6. Access, Sessions, and Audit;
7. Data and Maintenance.

Every Admin render begins with `Checking access…` when authorization is unresolved. It never flashes data from a previous session.

### 3.3 Authorization generation

Authorization changes create a new access generation:

1. stop the old Admin/Public stream as applicable;
2. abort old requests;
3. clear affected sensitive cache;
4. discard responses from older generations;
5. reload authoritative REST state under the new authorization.

Session revoke, expiry, role change, Guest disable, and Public-to-private transitions all use this sequence. Tokens and business DTOs are never transferred between tabs; tabs synchronize only an access-generation signal.

## 4. Route and page inventory

Each page has a stable ID. IDs are semantic and do not prescribe React filenames.

### 4.1 Public/Home pages

| Page ID | Route | Purpose | Actors |
|---|---|---|---|
| `PAGE-HOME-NETWORKS` | `/` | Network list and public availability | Guest, Viewer, Owner |
| `PAGE-HOME-NETWORK` | `/networks/:networkKey` | Network overview and Node list/cards | Guest, Viewer, Owner |
| `PAGE-HOME-NODE` | `/nodes/:nodeId` | Public Node detail and independent observation dimensions | Guest, Viewer, Owner |
| `PAGE-HOME-UNAVAILABLE` | public fallback route/state | Non-leaking private/retired/deleted/unknown response | Guest, Viewer, Owner |

### 4.2 Authentication and access pages

| Page ID | Route | Purpose | Actors |
|---|---|---|---|
| `PAGE-AUTH-LOGIN` | `/login` | Human login with safe `returnTo` | Guest |
| `PAGE-ACCESS-SESSIONS` | `/admin/access/sessions` | Coarse session review and revoke | Owner |
| `PAGE-ACCESS-PEOPLE` | `/admin/access/people` | People and role management | Owner |
| `PAGE-ACCESS-AUDIT` | `/admin/access/audit` | Immutable redacted Audit review | Owner, permitted Viewer read |
| `PAGE-AUTH-REVOKED` | `/session-revoked` or route-preserving state | Explain access generation transition | authenticated/expired |
| `PAGE-AUTH-FORBIDDEN` | protected route state | Explain insufficient role without leaking data | authenticated |

### 4.3 Agent and Node pages

| Page ID | Route | Purpose | Actors |
|---|---|---|---|
| `PAGE-ADMIN-OVERVIEW` | `/admin` | Owner attention queue and operational overview | Owner |
| `PAGE-ADMIN-AGENTS` | `/admin/agents` | Agent inventory, liveness, boot/report/spool diagnostics | Owner |
| `PAGE-ADMIN-AGENT-DETAIL` | `/admin/agents/:agentId` | Identity, credentials, liveness, inventory, diagnostics, audit | Owner |
| `PAGE-ADMIN-ENROLL` | `/admin/agents/enroll` | One-time enrollment workflow | Owner |
| `PAGE-ADMIN-AGENT-RECOVER` | `/admin/agents/:agentId/recover` | One-time recovery and epoch advancement | Owner |
| `PAGE-ADMIN-AGENT-ROTATE` | `/admin/agents/:agentId/rotate` | Credential rotation and overlap/revocation | Owner |
| `PAGE-ADMIN-NODES` | `/admin/nodes` | Node list, visibility, health summary, freshness | Owner |
| `PAGE-ADMIN-NODE-DETAIL` | `/admin/nodes/:nodeId` | Server-owned Node detail and diagnostics | Owner |
| `PAGE-ADMIN-NODE-TRANSFER` | `/admin/nodes/:nodeId/transfer` | Two-phase transfer workflow | Owner |
| `PAGE-ADMIN-NODE-VISIBILITY` | `/admin/nodes/:nodeId/visibility` | Public/private publication workflow | Owner |
| `PAGE-ADMIN-NETWORKS` | `/admin/networks` | Network Registry metadata and Nodes | Owner |
| `PAGE-ADMIN-NETWORK-DETAIL` | `/admin/networks/:networkKey` | Expected identity, metadata, mismatch diagnostics | Owner |

### 4.4 Alert and operations pages

| Page ID | Route | Purpose | Actors |
|---|---|---|---|
| `PAGE-ADMIN-ALERT-RULES` | `/admin/alerts/rules` | Typed Rule list, evaluation state, overrides | Owner |
| `PAGE-ADMIN-ALERT-RULE` | `/admin/alerts/rules/:ruleId` | Rule version, input, subjects, incidents, audit | Owner |
| `PAGE-ADMIN-ALERT-RULE-EDIT` | `/admin/alerts/rules/:ruleId/edit` | Structured Rule mutation | Owner |
| `PAGE-ADMIN-INCIDENTS` | `/admin/alerts/incidents` | Incident list and filters | Owner |
| `PAGE-ADMIN-INCIDENT` | `/admin/alerts/incidents/:incidentId` | Evaluation/Incident/Suppression/Delivery timeline | Owner |
| `PAGE-ADMIN-SILENCES` | `/admin/alerts/silences` | Active, expired, cancelled Silence policies | Owner |
| `PAGE-ADMIN-SILENCE` | `/admin/alerts/silences/:silenceId` | Matcher, scope, impact, expiry | Owner |
| `PAGE-ADMIN-MAINTENANCE` | `/admin/alerts/maintenance` | Maintenance Window list | Owner |
| `PAGE-ADMIN-MAINTENANCE-DETAIL` | `/admin/alerts/maintenance/:windowId` | Scope, expected conditions, expiry, reevaluation | Owner |
| `PAGE-ADMIN-DELIVERIES` | `/admin/alerts/deliveries` | Outbox rows, retry/dead-letter filters | Owner |
| `PAGE-ADMIN-DELIVERY` | `/admin/alerts/deliveries/:deliveryId` | Destination redaction, attempts, retry | Owner |
| `PAGE-ADMIN-CHANNELS` | `/admin/alerts/channels` | Supported notification channels and policy | Owner |
| `PAGE-ADMIN-CHANNEL` | `/admin/alerts/channels/:channelId` | Channel policy and test action | Owner |
| `PAGE-ADMIN-OPERATIONS` | `/admin/operations` | Long-running Operation history | Owner |
| `PAGE-ADMIN-OPERATION` | `/admin/operations/:operationId` | Progress, warnings, result, Audit link | Owner |

### 4.5 Data and maintenance pages

| Page ID | Route | Purpose | Actors |
|---|---|---|---|
| `PAGE-ADMIN-DATA` | `/admin/data` | DB, worker, retention, backup, Doctor summary | Owner |
| `PAGE-ADMIN-RETENTION` | `/admin/data/retention` | Per-family policies and execution state | Owner |
| `PAGE-ADMIN-RETENTION-EDIT` | `/admin/data/retention/edit` | Safety-bounded retention mutation | Owner |
| `PAGE-ADMIN-BACKUPS` | `/admin/data/backups` | Artifact list and integrity state | Owner |
| `PAGE-ADMIN-BACKUP-CREATE` | `/admin/data/backups/create` | Backup Operation submission | Owner |
| `PAGE-ADMIN-BACKUP` | `/admin/data/backups/:backupId` | Artifact detail and verify action | Owner |
| `PAGE-ADMIN-RESTORE` | `/admin/data/restore` | Highest-risk restore flow | Owner |
| `PAGE-ADMIN-DOCTOR` | `/admin/data/doctor` | Read-only diagnostic checks and reports | Owner |

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
- history gaps are rendered as gaps, never synthetic zeroes;
- host observation is collected once per Agent and referenced by Node views.

### 5.4 Node Health Summary

Severity and primary reasons are Server-owned. The WebUI presents the Summary and dimension reasons; it does not reimplement health policy or merge Node observations at Agent level.

### 5.5 Operation state

Long-running retention, backup, restore, Doctor, and similar actions use:

```text
Queued | Running | Succeeded | SucceededWithWarnings | Failed | Cancelled
```

A browser closing does not cancel an Operation. Progress is shown only when Server can compute it reliably. `SucceededWithWarnings` is not displayed as plain Success.

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

The exact implementation key format may vary, but the namespace boundary is mandatory. Public-to-private and role transitions clear affected keys before new responses are rendered.

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
- partial failures show per-object outcomes;
- access changes abort old requests and discard old responses.

## 7. Shared patterns

Stable semantic pattern references:

| Pattern ID | Contract |
|---|---|
| `PATTERN-STATUS-DIMENSIONS` | Collection, freshness, and value are displayed independently. |
| `PATTERN-ACCESS-CHECK` | First protected render is `Checking access…`; no old-data flash. |
| `PATTERN-AUTH-GENERATION` | Close old streams, abort requests, clear cache, discard old generation. |
| `PATTERN-OPERATION-DETAIL` | Technical progress and warnings are REST-authoritative and linked to Audit. |
| `PATTERN-CONFIRMATION` | High-risk actions use explicit confirmation, typed phrases where required, and no optimistic result. |
| `PATTERN-RESPONSIVE-TABLE` | Desktop table becomes priority cards; detail remains available without primary horizontal scroll. |
| `PATTERN-LIVE-REGION` | Announce meaningful transitions only; do not announce high-frequency SSE. |
| `PATTERN-SECRET-ONCE` | One-time secrets appear only in success response, never URL/history/log/Audit body. |
| `PATTERN-CONFLICT-RELOAD` | Show current server state and preserve user draft. |
| `PATTERN-REDACTED-DETAIL` | Destinations, endpoints, credentials, raw peer IPs, tokens, and complete bodies remain redacted. |

## 8. Page contract requirements

Every `PAGE-*` entry must specify the following before production coding:

1. user task and success outcome;
2. route and safe return behavior;
3. actor/permission boundary;
4. REST operations and generated DTO references;
5. query-key namespace and URL state;
6. SSE invalidations and reset behavior;
7. loading, empty, stale, error, Unknown, Disabled, Unsupported, forbidden, expired, conflict, and partial states;
8. mutation confirmation, Operation, refetch, and Audit behavior;
9. redaction and non-leaking copy;
10. desktop/tablet/mobile transformation;
11. heading, form, table, focus, live-region, zoom, and reduced-motion requirements;
12. Playwright scenario IDs.

### 8.1 Overview

`PAGE-ADMIN-OVERVIEW` prioritizes an attention queue, Server-owned Node Health Summary, freshness, and next actions. Independent panels may fail independently. An Agent monitoring multiple Nodes shows separate Node rows/cards; Host metrics are not duplicated.

### 8.2 Identity and lifecycle

Enrollment/recovery success displays a one-time secret exactly once. Rotation has an explicit overlap window and optional old-credential revocation. Node Retired/Active follows latest valid Agent Inventory; Admin guidance does not remotely force lifecycle. Node Transfer is two-phase: source remains authoritative until valid target declaration with matching Network Identity, then Server atomically switches ownership.

### 8.3 Alert operations

Alert Rules are typed and Server-owned. The first catalog is the one in `docs/design/platpulse.md` §17.1. Evaluation states are separate from Incident state:

```text
Normal → Pending → Firing → Recovering → Normal
Open → Resolved
```

Unknown/Stale cannot silently resolve an Incident. Silence suppresses Delivery only. Maintenance suppresses expected delivery for an explicit Agent/Node/Network scope without changing facts or Node Health Summary. Notification Events and per-channel Deliveries remain separate and at-least-once. Manual retry never creates a duplicate Event.

### 8.4 Data and recovery

Retention is safety-bounded and batched. It cannot delete protected history state, coverage/gap/divergence state, cumulative counters, or Audit constraints. Backup results include artifact, checksum, schema, and integrity but never secrets. Restore requires an exclusive stopped Server and typed confirmation; failed integrity or readiness preserves the current DB. Doctor is read-only, sanitized, and never auto-fixes, migrates, deletes, or rotates secrets.

## 9. Content, privacy, and redaction

The WebUI must not display or store in browser state:

- Agent credentials, enrollment/recovery secrets after their one-time success view;
- session tokens or CSRF values in URLs;
- passwords, pepper, TLS private keys, notification tokens, MaxMind keys;
- raw peer IPs, complete endpoints, complete enodes, raw provider responses, or complete request bodies;
- sensitive destination values beyond an approved redacted summary.

Errors name the failed user task and next safe action. They do not expose stack traces, secret contents, or internal paths. Confirmation copy states what will and will not change.

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
- critical fields remain visible: status, subject/Node, head/sync, freshness, primary reason, risk, next action;
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

The accepted prototype is a visual/state primary source, not a production dependency:

```text
prototype/phase2-operations-loop @ 58d6f9c
```

A production mock adapter, if needed for component tests or development, must match the typed API adapter:

```text
mock operation → response DTO → error cases → invalidation → expected refetch → scenario ID
```

Scenario IDs:

```text
SCN-AUTH-OWNER-LOGIN
SCN-AUTH-SESSION-REVOKED
SCN-OVERVIEW-FRESH
SCN-OVERVIEW-STALE-LAST-GOOD
SCN-OVERVIEW-UNKNOWN-UNSUPPORTED
SCN-NODE-TRANSFER-PENDING
SCN-NODE-TRANSFER-IDENTITY-MISMATCH
SCN-NODE-TRANSFER-COMPLETED
SCN-ALERT-UNKNOWN-NOT-RESOLVED
SCN-ALERT-SILENCE-DELIVERY
SCN-ALERT-MAINTENANCE-EXPIRY
SCN-DELIVERY-RETRY-DEAD-LETTER
SCN-DATA-BACKUP-VERIFIED
SCN-DATA-RESTORE-SERVER-RUNNING
SCN-DATA-DOCTOR-WARNINGS
SCN-ACCESS-ROLE-CHANGE
```

Scenario state is memory-only. No credentials, secrets, production endpoints, local persistence, or prototype-only branches are allowed in production pages.

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
| `SCN-OVERVIEW-FRESH` | independent Node rows, Server Health Summary, current timestamps |
| `SCN-OVERVIEW-STALE-LAST-GOOD` | last-good remains, Error/Stale reason and age visible, no zero substitution |
| `SCN-OVERVIEW-UNKNOWN-UNSUPPORTED` | Unknown/Unsupported/Disabled/Empty remain distinct |
| `SCN-NODE-TRANSFER-PENDING` | source remains authoritative, target declaration pending, expiry visible |
| `SCN-NODE-TRANSFER-IDENTITY-MISMATCH` | blocking diagnostic, no ownership switch, conflict/audit result |
| `SCN-NODE-TRANSFER-COMPLETED` | authoritative refetch after atomic switch, history not merged incorrectly |
| `SCN-ALERT-UNKNOWN-NOT-RESOLVED` | Open Incident remains open, Evaluation unavailable is explicit |
| `SCN-ALERT-SILENCE-DELIVERY` | Delivery suppressed, evaluation and Incident unchanged |
| `SCN-ALERT-MAINTENANCE-EXPIRY` | scope/reason/expiry, current firing reevaluated once, no historical backfill |
| `SCN-DELIVERY-RETRY-DEAD-LETTER` | per-channel state, bounded retry, manual retry without duplicate Event |
| `SCN-DATA-BACKUP-VERIFIED` | Operation state, checksum/integrity, latest-success preservation |
| `SCN-DATA-RESTORE-SERVER-RUNNING` | refused before mutation, current DB remains authoritative |
| `SCN-DATA-DOCTOR-WARNINGS` | mixed check states, sanitized detail, no auto-fix |
| `SCN-ACCESS-ROLE-CHANGE` | generation change, old response discarded, correct route/permission state |

For each core scenario, test semantic content rather than screenshot alone, and verify no horizontal overflow at all four viewports. Test keyboard navigation, focus return, Escape, mobile drawer behavior, 200% zoom, reduced motion, accessible names, and preservation of URL/filter/scroll/expanded/draft state after refetch.

## 13. Implementation handoff checklist

A page is ready for production implementation only when:

- [ ] `PAGE-*` contract exists in this document;
- [ ] route and authorization boundary are explicit;
- [ ] OpenAPI operation and DTO references are verified;
- [ ] Public/Admin query namespace is assigned;
- [ ] SSE invalidations and reset behavior are specified;
- [ ] all required loading/empty/stale/error/Unknown/Disabled/Unsupported/access states are specified;
- [ ] mutation, confirmation, conflict, partial success, Operation, refetch, and Audit behavior is specified;
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
| Identity, lifecycle, access, and workflow contracts | Issue #37 |
| Alert, maintenance, retention, backup/restore, Doctor | Issue #38 |
| Representative operations-loop prototype | Issue #39, branch `prototype/phase2-operations-loop` @ `58d6f9c` |
| Implementation handoff and acceptance contract | Issue #40 |

Changes to a settled contract require a new decision record and must update the affected `PAGE-*`, `PATTERN-*`, and `SCN-*` references together. OpenAPI or Server policy changes do not silently change WebUI semantics; they require an explicit design review when the user-visible contract changes.
