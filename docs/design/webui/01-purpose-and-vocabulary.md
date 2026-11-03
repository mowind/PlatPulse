Part of the [PlatPulse WebUI design](../webui.md): §1–§2. Sections keep their numbers; cite them as `webui.md §X.Y`.

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

These are current-surface exclusions. §15.1–§15.3 already route Agent enrollment guidance, editable Agent metadata, Agent Removal, and Agent Attention Acknowledgment; the items below that remain out of the current surface are remote control and the other deferred management pages.

- Agent, Server, SQLite, or evaluation implementation;
- a duplicated full Node Detail inside Admin;
- RPC Endpoint editing, RPC Endpoint failover, remote commands, restart, upgrade, Docker control, or terminal access;
- TUI, arbitrary scripts, SQL/DSL alert rules, or remote-control UI;
- Validator/Restore/Node Transfer/People/Recovery/Rotation management pages (their Server/API operations exist but are not currently routed); Notification test and Event/Delivery history are routed (§15.9); the routed Agent enrollment/metadata/removal/attention surfaces (§15.1–§15.3), the Incident history and detail pages (§15.6), the Operations ledger, Task detail, and Doctor pages (§15.11, issue #208), the Backup artifact surface (§15.12, issue #209), the Retention policy surface (§15.13, issue #210), the retention-execution and cancellation surface (§15.14, issue #211), the storage capacity and low-space protection panel (§15.15, issue #212), the Node metric history surface (§15.16, issues #213 and #214), the shared Host resource history carried by the Node detail and Agent detail pages (§15.17, issue #215), the `Storage by mount path` list on the Agent detail page (§15.18, issue #216), and the recorded sync and consensus state history of the Node detail page (§15.19, issue #217) are the standing exceptions, and Geo provider selection and the global Geo refresh are both routed on Settings;
- raw Peer identity/addresses, complete Peer Snapshot browsing, multi-tenant, HA, clustering, SSO/OIDC/TOTP/WebAuthn;
- runtime theme/script injection or a second frontend framework.

## 2. Authorities and vocabulary

Use the exact domain terms in `CONTEXT.md`. Current terms include: Host, Agent, PlatON Node, Node ID, RPC Endpoint, Network, Network Identity, Network Registry, Node Inventory, Active Node, Retired Node, Component Observation, Agent Report, Report Receipt, Current Projection, Block Summary, Peer Snapshot, Peer Insight, Peer History, Host Observation, Node Process Observation, Node Chain Observation, Node Observation, Node Health Summary, Validator, Validator Activity, Geo Provider, Geo Database, Geo Location Cache, Geo Insight, Attention Item, Public Projection, Site Access Mode, Invalidation Event, Home Dashboard, Admin Dashboard, Audit Event, Owner, Viewer, and Guest.

The WebUI must not invent synonyms that blur boundaries. In particular:

- Home is not “public admin”;
- Node Health Summary is not a WebUI-computed health color; the three-state Node marker on Home and Node Detail only reflects the Server-owned health word (§5.4);
- an Agent that stops reporting does not retire its Nodes;
- an Agent-level page must not merge independent Node chain observations;
- a successful Peer Snapshot/aggregate with zero peers is authoritative, not Unknown; omitted or unsupported peer capability is not an empty snapshot;
- recent Block History is bounded by the Server window and is best-effort: normal missed blocks remain absent, while explicit bounded Gap Backfill may recover eligible heights; neither path synthesizes zeroes or fabricated summaries.

### Approved Emerald composition refinements

- Home keeps the entire page at the existing **1280px cap** and preserves the Node grid’s `repeat(auto-fill, minmax(min(100%, 22.5rem), 1fr))` column rules, whose clamped 22.5rem minimum keeps every auto-filled card at least as wide as the compact summary it holds and never overflows a narrower container (issue #238); that summary switches from two to three columns at its own 25rem container width At **1024px and above**, the overview allocates its statistics/map tracks about **4:3**: six equal summary cards in three columns by two rows at left, a complete contained **2:1** map at right. The 4:3 split holds each tile at the width the original four-card 2x2 grid gave it instead of stretching it, and the tiles keep its compact height: a 44px header row carrying the title and either the metric marker or the Breakdown control, then the value over a shared one-line stat slot — no footer row. The two cumulative tiles fill that slot with their own scope and a short valued/expected reporting count (`5/8 reporting`, with a `stale` marker and a named missing Network summary where they apply), so all six tiles still share one height and one value baseline; the trailing icon or Breakdown control is one shared 44px marker box in every header, so the six titles, markers and values line up by construction instead of by per-tile offsets (§15.29, issue #240). Below 1024px, the map leads the band visually while the statistics keep their three-columns-from-640px / two-on-phones grid and their unchanged six-item DOM order. This human-approved presentation supersedes the earlier 37:61/232px band rather than widening the page or changing Node columns.
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

Node Detail uses `Successful` only as the display label for a successful Process collection; it does not assert current process liveness or freshness. The wire Collection State vocabulary is unchanged.

`Peer data current` is a scoped, low-weight healthy summary for Peer Insight; it does not replace the independent Collection, Freshness, or Value terms when those dimensions need explanation. `Online` and `N/A` are not generic replacements. Status communication always includes text and an icon or equivalent explanation; color is supplementary. An open SSE stream renders no transport notice: the positive transport state is deliberately silent because it changes nothing the visitor should do or believe. `Live updates connected` is therefore not part of this vocabulary and must not be reintroduced; only `Connecting to live updates` and `Live updates paused` are rendered and announced.
