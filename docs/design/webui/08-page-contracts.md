Part of the [PlatPulse WebUI design](../webui.md): §8–§8.4. Sections keep their numbers; cite them as `webui.md §X.Y`.

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

### 8.1 Home Node Detail (PAGE-HOME-NODE)

- One continuous shared page follows the [unified Node Detail contract](11.1-node-detail-and-visual-acceptance.md#node-detail-composition-page-home-node): Header → six KPIs + Node Peer Country View → Chain state / Process / Host resources → conditional Validator performance → Latest 60 seconds → conditional Validator diagnostics. No Details/Network tabs or role-specific page copies remain.
- The uncarded Header shows Node Health, name, sync, evidence-backed role, applicable Activity and inline report age, with full Node ID/copy below. Role/performance eligibility uses Server Current Validator Status and independent currency, never consensus membership or Link existence alone.
- Both roles share six equal KPIs and the Node-scoped Peer country map, approximately 5:6 desktop tracks and KPI-before-map mobile order; the map keeps Home's band (proportional 2:1 below xl, the fixed 22rem band from xl) and the six KPIs keep their natural height bottom-aligned to that band, exactly as Home renders its statistics beside the same band, rather than stretching to fill it. Map and Peers share the retained current_node_peers basis, without IP deduplication or exposing Peer addresses/identities; Disabled, never-observed, stale and failed map states retain honest local degradation.
- Shared panels show available chain heights/current high-confidence lag, process collection/start/storage and shared Host resources. Collection success is not present-tense Running; process CPU/memory appear in common charts. Missing optional observations are hidden, not converted to zero or filler Unknown rows.
- Six charts preserve real response bounds, CPU/memory dynamic axes, current values, units, bottom legends and tooltips. Series failures degrade locally; last-good values survive. History derives only the consecutive-block interval, with no Bounded Block History or public-history export.
- Retired/deleted/unknown Nodes retain non-leaking unavailable semantics. Public API, Home-card and Admin boundaries are unchanged.

### 8.2 Admin Node Detail (`PAGE-ADMIN-NODE-DETAIL`)

The accepted future permanent-delete interaction is in §15.2; it is not implemented by the current metadata/lifecycle controls described here.

- consumes the full administrative `AdminNodeDetail` DTO, which includes display name, redacted RPC Endpoint diagnostics, Node Inventory/lifecycle (Active/Retired), health/freshness, process/data/RPC/Sync/Consensus/Peer diagnostics, identity/high-watermark/resync, and transfer context;
- the current page renders the approved administrative/diagnostic subset and must not reproduce Home's full observation cards/chart deck; it links to the shared `/admin/access/audit` page separately rather than treating Audit links as DTO fields;
- every mutation is audited; transfer fields are visible as data where supplied but no transfer management page is currently routed;
- the page is layered (§8.7): an always-visible header (back link, name, Network and Agent links, the Node ID behind `AdminIdentifier`, the independent Health / Freshness / Identity / Lifecycle / Current head dimensions and the Agent-owned Spool Backlog Age — labelled `Agent Spool backlog` with the instant its reading is anchored on, so the group reads `Current Node and Agent dimensions` because Spool state is observed once per Agent and the Agent link above is its only owner — and the current findings with an `Open Diagnostics` entry), then four sections on the shared Tabs — `Overview` (Health and freshness; Node Inventory & lifecycle with Agent/Network context), `History` (Process metric history, Sync and consensus history, Host metric history), `Diagnostics` (Network identity and Block-history refusal, Validator identity, RPC diagnostics, and Component diagnostics — RPC/Sync/Consensus/Peers/Process/Node Data state, safe error, last-good value and time, attempt time) and `Manage` (Server-owned display name, then the separate `Danger zone` with Permanent deletion). The open section is the `?section=` URL parameter; it is presentation state, not a route.

### 8.3 Settings (`PAGE-ADMIN-SETTINGS`)

- renders one Settings heading with an ordered History Window, Site Access Mode, Geo provider, then read-only Storage Backend module inside one left-aligned constrained surface (see §8.6);
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
- the read-only Storage Backend card reports where the Server keeps its data from the same Owner-only read-only Admin endpoint Doctor reads (issue #256, ADR 0013): on SQLite it names the database file and the offline backup, restore, integrity and capacity consequences that then apply, while on PostgreSQL it names the server version, host, port, database and user and states that the database disk, its backups and its integrity belong to the database operator. A PostgreSQL server older than the supported minimum is called out as an alert, an unreadable version stays `Unknown` rather than being guessed, and a failed refresh says the facts are the last read instead of passing them off as current. The card offers no control at all: the backend is chosen in the Server configuration file, and no Admin surface switches it at runtime;
- the Storage Backend card comes last because it decides which of the cards above it even apply — capacity protection and Backup Artifacts exist on SQLite only;
- the Settings cards stack on narrow viewports, preserve 44×44 CSS pixel targets, and never cause primary horizontal page overflow.

The retired `/admin/history-window` and `/admin/site-access` routes are not redirected; they resolve through the Admin Section not found fallback. This is a page-level SPA outcome and does not guarantee an HTTP 404 response from the Server.

#### 8.3.1 Settings integration acceptance

The Settings route is the single canonical configuration surface. The accepted scenarios map to `/admin/settings` as follows:

| Scenario | Route and outcome |
|---|---|
| `SCN-SETTINGS-ROUTE` | `/admin/settings` renders one logical `Settings` h1, ordered `History Window`, `Site Access Mode`, `Geo provider`, then read-only `Storage Backend` sections, and no obsolete navigation entries; the retired URLs remain on the Admin Section not found fallback without redirecting. |
| `SCN-SETTINGS-STORAGE-BACKEND` | The Settings page reports the Storage Backend the Server is running on without offering to change it: on SQLite the database file plus the offline backup/restore/integrity/capacity consequences, on PostgreSQL the server identity plus the consequence that the database operator owns the disk and its backups. The card is read-only, and no Storage Backend identity appears in a Public Projection. |
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
3. `Node Health Summary` previews at most the five highest-priority Active Nodes. The complete inventory remains at `/admin/nodes`.
4. `Agent inventory` previews at most the five highest-priority Agents. The complete inventory remains at `/admin/agents`.

The two previews sit side by side when each has at least about 28rem, otherwise they stack (one `auto-fit` grid); they are lists, not the wide diagnostic tables the earlier Overview used.

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

Each preview item keeps only:

```text
Node display name (shortened Node ID when unnamed), linked to Admin Node Detail
Network display name
Server-owned Health and Freshness as two separate badges
Health reason only when Health is not healthy
Identity mismatch (with the mismatched fields) and a non-normal resync state, when present
```

A healthy, current item repeats no reason and no per-row teaching copy such as "Server-owned freshness"; Freshness is never folded into Health and a stale or unknown value is never shown as current.

The default priority is unhealthy, unknown health, stale, then healthy/current, with stable Network and Node-name tie-breakers. Timestamp-only invalidations do not reorder the list. At most five Active Nodes are shown, followed by `Showing N of M Active Nodes`, a `Not shown: …` line that counts the left-out unhealthy, unknown-health, stale, identity-mismatch and identity-not-reported Nodes (independent dimensions, so one Node can appear in more than one count) whenever any exist (so a limit never reads as "no other exceptions"), and `View all Nodes`. Retired Nodes remain in their summary card and the filtered Nodes inventory, not this live preview.

The Overview no longer expands per-component RPC/Sync/Consensus/Peers/Process/Node Data diagnostics in place; they moved to the `Component diagnostics` panel of the Node Detail `Diagnostics` section, which the Nodes row details link to (§8.7).

#### 8.4.4 Agent inventory

Each preview item represents one Agent and keeps only:

```text
Server-owned display name, otherwise the shortened Agent ID, linked to Agent Detail
Server reporting status (liveness)
Last received (relative, with the absolute UTC time on hover/title) or Never received / Unknown
Retained Node count from the Agent's own Inventory, plus the unhealthy count joined from the Node list when it is loaded
The summary headline (§8.7 "Agent evidence summary rule"): the most severe current item, or, without one, the first not-observed item labelled "Not observed", then the folded rest per group ("+N current (K critical) · +N not observed"; current = the Server's own Agent Attention Items and a credential set with no Server-active credential)
Recorded history: the highest recorded item and "+N recorded (K error) on the Agent"
```

Host CPU, memory and RX/TX, the full Agent ID, the report sequence, Spool capacity and queued bytes, and Clock skew are not shown on Overview; they remain on Agent Detail (`Overview` keeps a short runtime summary, `Diagnostics` the complete evidence). Unknown is never zero: a missing Host or Spool observation reads `Host not observed yet` / `Spool not observed yet`. Recorded history is never presented as a current fault, and a known-zero counter is not listed.

The default priority is presentation only (`agentPreviewTier`; no Server state, history or acknowledgment changes): Agents with a current critical item (a Server Attention Item of Critical severity, or a Spool store fatal in the latest Host observation), then offline, unknown reporting, other current items, not fully observed, recorded history only, and the rest, with stable Agent ID tie-breaking. Accumulated counters (`security_event_count`, gap intervals) and retained Spool evidence are recorded history and never rank an Agent as critical on their own; once the Owner acknowledges the Server's own Attention Item, that item no longer ranks the Agent while its history stays visible — except that a Spool store fatal still reported by the latest Host observation is current state and keeps the Agent in the critical tier. At most five items are shown, followed by `Showing N of M Agents`, a `Not shown: …` count of left-out Agents, each counted once under the same tier that ordered it (with critical current attention, offline, unknown reporting, with current attention, not fully observed, with recorded history), and `View all Agents`.

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

When the loaded Agents, Nodes, and Networks projections are empty, the page retains authoritative zero/Empty states and adds a compact setup guide: register the expected Network identity (linked to Networks), add an Agent (linked to the existing `/admin/agents/enroll` page, whose local instructions cover installing and enrolling the Agent on its Host), declare the Host's Nodes in the Agent's local `agent.toml` and start the Agent (plain text, no link: the Node Inventory is Agent-local configuration that neither the Server nor the WebUI edits), and wait for the first accepted Agent Report. The guide does not link Settings, which only manages Server-wide values. The guide appears only when the Overview, Nodes and Agents surfaces all loaded successfully and are empty; a pending or failed surface is never read as a first install. The page cannot configure an RPC Endpoint, start an Agent, create or edit a local Node Inventory, run an Enrollment from the Overview itself, enable fake data, or offer one-click initialization.

#### 8.4.6 Page boundaries

Overview owns triage, compact counts, priority subsets, and safe navigation. The Nodes page owns the full inventory, lifecycle/Network/health/freshness filters, stable sorting, inventory revision, identity disposition, redacted RPC Endpoint diagnostics, metadata editing, and Audit links. Admin Node Detail owns full administrative Component diagnostics and must not reproduce Home's observation-card and chart deck.

The Agents page owns the complete Agent inventory, epoch, boot/report state, Node Inventory, credential state, Spool diagnostics, clock diagnostics, sequence gaps, and security events. Agent Detail owns full identity, credential, Host, Inventory, diagnostic evidence, and Audit context. Overview never displays complete credentials, complete RPC Endpoints, Boot IDs, internal paths, stack traces, or remote controls.

#### 8.4.7 Responsive and visual acceptance

The visual balance is PlatPulse's Emerald light system first and Komari-inspired density second: Slate-50-like background, translucent-white surfaces, quiet one-pixel borders, restrained 8-10px radii, no default blur, minimal shadows, high-contrast counts, quiet labels, neutral primary controls, measured Emerald selection accents, and semantic green/amber/red/blue/neutral status treatments. No status depends on color alone. The current production UI remains English; localization is a separate whole-application capability rather than a mixed-language Overview.

At `1280x800`, the Admin sidebar is persistent, Attention is full-width, summary cards form four columns, and the Node and Agent previews sit side by side. At `768x1024`, navigation uses the accessible drawer, summary cards form a two-by-two grid, and the two previews stack. At `360x800` and `390x844`, summary cards remain a compact two-by-two grid when legible and may fall to one column when content requires it; the previews keep the same item anatomy (name and status on the first line, context and exceptions below) and Audit becomes priority cards. Secondary evidence stays on the inventory pages and detail sections, controls remain at least 44x44 CSS pixels, and no primary horizontal page scrolling is allowed. The page remains functional at 200% zoom, in portrait and landscape, and with reduced motion.

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
