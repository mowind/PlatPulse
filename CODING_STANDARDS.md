# PlatPulse coding standards

These are the rules a reviewer checks by reading the diff. Authors check them before they open a pull request; the `code-review` skill's Standards review checks them for every change.

- The engineering conventions in [AGENTS.md](AGENTS.md#engineering-conventions) also apply. This file does not repeat them; it adds what only shows up in review.
- A rule a gate already enforces is not listed here and is not raised in review: `cargo fmt`, clippy (`unwrap_used`, the disallowed `Pool::begin`), ESLint (Playwright specs import `test` from `e2e/fixtures.ts`), and the `scripts/check-*.py` checks AGENTS.md lists.
- **Blocking** findings stop the merge. **Review checks** are where PlatPulse defects have come from; a finding there is blocking when the diff shows the defect, and a question otherwise.

## Blocking

1. **A behaviour change carries a test that fails without it.** A fix adds a regression test that is red on the old code. The Testing section says how that was shown (reverted fix, counterexample input, mutation).
2. **Tests go through the real path.** Real temporary SQLite and real PostgreSQL through `platpulse_server::test_support`, the Axum router for HTTP behaviour, a real Server for e2e; no mocked database or HTTP layer. A test that only holds on SQLite is annotated `#[cfg_attr(storage_postgres, ignore = "<reason>")]` with the reason it cannot hold on PostgreSQL.
3. **Documents change with the behaviour.** When the diff changes a contract, the section that states it changes in the same pull request: `docs/design/` (design §, webui.md §), `CONTEXT.md` for a term, the ADR for a decision (its Status becomes `Accepted` in the pull request that delivers it; see `docs/agents/domain.md`), `docs/deployment.md` and `docs/deployment/` for operator-facing behaviour, the OpenAPI document for the HTTP API.
4. **Breaking change is answered honestly.** Configuration keys, CLI, data format or schema, Agent–Server version compatibility and the HTTP API are externally visible; changing one is `Yes` and adds the operator action to `docs/upgrade-guide/v<next>/README.md`.
5. **Testing lists what was run.** Commands and their results, and which lanes were left to CI. "Tests pass" with no command is not evidence.

## Review checks

### Server

- **Trust boundary.** A new or changed AgentReport field is validated by the Server with explicit bounds (the contract limits live in `platpulse-core`, e.g. the chain-time upper bound), and a rejection names the field. An out-of-contract value is rejected or quarantined, never stored as a value that poisons later reads.
- **Read-decide-write on PostgreSQL.** The PostgreSQL Storage Backend runs several work connections, so two writers can pass the same check on one snapshot. A write that decides from a read takes the fence for its subject (`crates/platpulse-server/src/http/write_fence.rs`) in the order Agent → Nodes (ascending) → retention sweep gate, and never awaits a fence while it holds a connection or transaction. A test for it runs concurrent writers on PostgreSQL.
- **Unique-key conflicts.** A duplicate is detected with `ON CONFLICT … DO NOTHING` (or `RETURNING`) and answered with the existing row's semantics (replay, 409 with a stable code). Catching the unique violation and querying again does not work on PostgreSQL: the failed statement aborts the transaction.
- **The receipt transaction does receipt work only.** Geo, retention and other maintenance run after the commit or in their worker; work added inside the Report receipt transaction is justified in the pull request.
- **Bounded work per Report and per request.** Ingestion, settlement and list endpoints touch rows proportional to what the Report or request names, through an index, never a table scan per Report. A new query on a hot path names its index; a test that proves boundedness counts reads or rows (a probe), not wall-clock time.
- **Observation time, not receipt time.** Freshness, history timestamps and sample identity use the observation time (ADR 0014). A comparison against a horizon or window has an explicit boundary rule with a test on both sides of it.
- **Failure is not a negative answer.** A failed database read is Unknown or `503 unavailable`, never `401`, Ended, absent or empty; a stream that cannot confirm authorization keeps the connection but sends nothing (design §10.2).
- **Process lifecycle.** Signal handlers are installed before startup work begins; a background task added to `serve` stops on shutdown and its failure is logged and retried, not swallowed.

### Agent

- **Durable Spool.** A Report leaves the spool only by being sent or by being discarded, and both paths validate it first; a corrupt Report fails closed (`StoreFatal`), it is never dropped silently. A read that a write decision depends on holds the store permit until commit or rollback (ADR 0001).
- **Reuse clients.** HTTP and RPC clients are built once and shared (`http_client.rs`); no client per request.
- **Units at the boundary.** A value whose unit depends on the Node (seconds vs milliseconds) is normalized where it is read, with a test for each unit.

### WebUI

- **Subscribe, then read.** A component that reads a value and subscribes to its changes reads it again after the subscription exists; a change that lands between the first read and the subscription is otherwise lost.
- **An invalidation is answered by a read issued after it.** When an SSE invalidation arrives while a read is in flight, the page reads again after that read settles (`flushPublicInvalidations`, `invalidateAdminQuery`); reusing the in-flight request answers the event with stale data.
- **Unknown stays unknown.** A never-observed, stale or unavailable value renders as such, never as `0`, `false`, empty or Healthy; an unreachable Server is an outage, not a sign-out.
- **Every surface works at 360 px.** A layout change is checked on the phone projects as well as desktop; controls keep an accessible name (webui.md §10).

### Tests

- **Wait on state, not time.** A test waits for the condition it needs (a response, a rendered state, a log line), not a sleep. A series of timestamps is computed from one fixed now (`canonicalClock()` in `e2e/helpers.ts`), not `Date.now()` per call.
- **Own the data.** A test that writes creates its own Agents and Nodes or uses a disposable Server (`e2e/server-harness.ts`); it never depends on what another test left behind.
- **A slow test says why.** `test.slow()` carries a comment naming the heavy step; timeouts are not raised globally.
- **The assertion can fail.** The test asserts the outcome the change is about, on the value the user or caller sees; an assertion that also passes on the old code is not a regression test.

## General

- **Every new concept earns its place.** A new type, module, configuration key or abstraction serves the issue being closed; nothing is added for a later phase (AGENTS.md: no shallow crates, no empty schema).
- **Names come from `CONTEXT.md`.** A domain term uses the glossary word, not a synonym it bans; a new term is added there in the same pull request.
- **Comments state the current contract.** Why the code is the way it is now, not which issue changed it; history goes in the pull request.
