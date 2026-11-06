Part of the [PlatPulse WebUI design](../webui.md): §5–§7. Sections keep their numbers; cite them as `webui.md §X.Y`.

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

Where Home or Node Detail mark a Node by name, a three-state marker sits before that name: Healthy is the Emerald success colour, Unhealthy the error colour, and every other state - Unknown and never-observed - the theme's warning colour, so insufficient evidence is never presented as a definite failure. The marker carries the Server-owned health word as its accessible name, so the compact presentation never drops Healthy, Unhealthy, or Unknown from the accessibility tree; it does not replace the Summary, and an exceptional Node states one short diagnostic line whose full Server reason stays reachable in the card's explanation (§15.32). Admin keeps the full Server-owned severity presentation (§8.4.3).

Node Currency is Server-owned in the same way. At Node level the Public and Admin Node DTOs carry `currency` (`current`, `stale` or `never_observed`) and its instant (`currencyAsOf` in Public, `currency_as_of` in Admin): the oldest of the RPC, sync and consensus observation instants, and null whenever one of them has never been observed. Both DTOs read it from the same Server derivation as Health, so one Node never reads Current on one surface and Stale on the other. The WebUI renders it as Current, Stale or Never observed and never re-derives it from `Date.now()`; the Node Head comparison on Home and Node Detail reads it as its currency gate, so a Stale Node's height is marked as a stale observation and a Never Observed Node's height as one whose observation time is unknown. Value-level freshness (Peers, consensus insight, Validator Activity and rank) stays a separate dimension of its own value.

### 5.5 Server-side state machines without current pages

Alert Rule/Incident/Silence/Maintenance evaluation and long-running Operation states are implemented in the Server and exposed through Admin APIs. The read-side Incident history and detail pages, including the Owner acknowledgment, are routed per §15.6 (issue #203), and Alert Rule read/edit/preview with Network/Node overrides is routed per §15.7 (issue #204), with Silence and Maintenance Window management routed per §15.8 (issue #205), and the Operations ledger/Task detail plus Doctor pages consuming the recorded Operation DTOs per §15.11 (issue #208); storage capacity and low-space protection is likewise a Server-owned state machine surfaced read-only on the routed Operations page per §15.15 (issue #212), the mount list a storage series is read through is likewise read from the stored coverage ledger on the Agent detail page per §15.18 (issue #216), and the recorded state history the Node detail page reads is the Server's own state log with its per-series ledger per §15.19 (issue #217); no page resolves or suppresses the underlying state machines. They remain independent of the WebUI's Node Health Summary; a future page must consume the typed DTOs rather than recreate those state machines in the browser.

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

The Public/Admin event endpoints accept cursor recovery through the `after` query parameter or the `Last-Event-ID` header (the reconnect header takes precedence when both are supplied). Responses are `text/event-stream`; each emitted event uses the `invalidation` event name, a numeric event id/cursor, and JSON `data` with `version`, `eventId`, `resource`, optional `resourceId`, `revision`, and optional `reset`. A first connection without a cursor starts after the current buffered sequence because REST has already supplied the snapshot; the browser should send the last received cursor when reconnecting and discard/reconcile events older than its current authorization generation. The sequence lives in memory and each Server process starts it at its wall-clock start (microseconds since the Unix epoch), so every ID a process issues is above any ID an earlier process issued; a reconnect cursor below the current process's start — one kept across a Server restart — is answered at once with a collection reset rather than with silence until the new sequence climbs past it.

SSE connection status is visible but does not cover valid content. Public Home and Node Detail surfaces show `Connecting to live updates` while connecting and `Live updates paused` after disconnect; an open stream renders no transport notice at all, because a connected stream changes nothing the visitor should do or believe. Browser/network loss additionally uses `You are offline` when the browser signal is authoritative. The compact Admin header retains its existing `Starting`/`Current` transport labels so public transport wording does not increase Admin information density. These transport labels never certify REST refresh success, observation freshness, Agent liveness, or Node Health. The current generated OpenAPI records the stream operation but does not fully describe every cursor/header/event field; runtime `realtime` behavior is the authority for replay.

SSE updates must preserve filters, sorting, scroll, expansion, and ordinary drafts. They do not reorder a list merely because a timestamp changed.

### 6.4 Mutation flow

- no optimistic business state;
- no automatic mutation retry or replay;
- success immediately invalidates and refetches authoritative REST;
- failure preserves drafts and shows field/page errors;
- conflicts refetch current authoritative state without overwriting drafts;
- an inline editor keeps three values apart: the read view shows only the Server's current value; an edit round starts from the latest Server value and a background refresh never overwrites its draft (a Server change during the round is stated beside the draft with the current Server value and withdraws an open confirmation, and the save control is disabled with that statement as its visible, adjacent reason — never a control that accepts a click and then withdraws it; a Server that already holds the draft, compared under the same normalization the save sends, says nothing is left to save, offers no save control and turns Cancel into Close editor, and that editor no longer counts toward its tab's `Unsaved` marker while other operations keep theirs); a confirmation holds the exact request it sends, any field change withdraws it, and Confirm re-runs the form's validity rules (the same normalization and checks Save runs: a Node display name is trimmed, and one that is empty after trimming is refused beside its field and never reaches a confirmation or the Server — there is no operation that clears a name); Cancel discards the round, so reopening never reuses an earlier draft or confirmation;
- a request in flight locks only its own editor (inputs, Keep editing and Cancel; the Confirm control reads `Saving…`, keeps focus and is `aria-disabled`, with `aria-busy` on the form), survives a section switch, is never sent twice, and success is shown only once the authoritative detail has been read back; a write that succeeded and a read-back that failed after it are two results — the write is stated in the success tone, the failed read-back is its own warning-toned notice beside it (`Not read back`, with a read-only "Read … again"; the page's refresh alert carries the error itself, so the notice is not a second alert), the write is never sent again, and the notice is lifted only by a later successful read of the same entity's detail (not by the detail held from before the write, a connected stream, or another entity's read); an edit round belongs to one entity and never carries into another;
- a write that replaces a whole value (the Network identity tuple) is held per access generation and entity outside its editor, from the moment it is sent until it is answered and the entity is read again by a read begun after the answer; leaving and reopening the editor does not lift the hold, which keeps new drafts editable but unsavable, with the reason adjacent to the save control. Only a refusal the Server is known to send for that operation before anything is committed — one of the error codes of its handler or of the guards in front of it (session, Owner role, proxy headers, Inventory cutover), at the HTTP status it is sent with, in the complete documented error envelope; the generated OpenAPI lists only some of these statuses, so the list is kept beside the write and checked against the Server code — proves nothing was applied and returns the draft and confirmation for another attempt. A readable error object is not such a proof on its own: any other failure (no response, an unreadable one, an intermediary's page or JSON error such as a gateway's 502/504 carrying its own `error.code`, an unknown code, a code at another status, or an incomplete envelope) leaves the outcome unknown: it is stated as such (`Outcome unknown`, not as a failed save), never re-sent, and only a read showing the Server holding exactly that value ends it — a read of the earlier value proves nothing. The hold is memory only and ends with the access generation;
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
