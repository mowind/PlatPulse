import { expect, test, type Page, type Response } from "@playwright/test";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import type {
  InvestigationResponse,
  InvestigationSourceResponse,
} from "../src/api/generated";
import { sourceAtInstant } from "../src/nodeInvestigation";
import {
  VIEWPORTS,
  expectLocalTableScroll,
  expectResolvedTheme,
  focusByKeyboard,
  gotoAuthenticated,
  loginToDisposableServer,
} from "./admin-flow";
import {
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
} from "./helpers";
import { startDisposableServer, type DisposableServer } from "./server-harness";

/**
 * One adjustable UTC window over a Node's own evidence (issue #220, design
 * §15.22).
 *
 * The acceptance of this surface is that one window answers every family of the
 * Node's evidence, that each family states the clock it answers in and the
 * stretch it cannot answer rather than a zero, and that the window itself is the
 * Server's answer instead of a stretch this browser invented. The spec therefore
 * seeds real Reports into a disposable Server, reads the Server's own answer
 * over real HTTP first so every panel is judged against the record rather than
 * against a number the spec picked, and only then drives the production WebUI
 * across all five viewports the Admin matrix fixes, in both resolved themes.
 *
 * A window is never guessed and never filled from the browser clock: the window a
 * link names is sent to the Server exactly as it is written, and a window the
 * Server will not read is reported in the Server's own words rather than being
 * replaced by one nobody asked for. That vocabulary is driven here too, together
 * with the entrance an Incident gives an Operator (occurrence as the start of the
 * window) and the difference between a protection pause and a failed collection.
 */

/** A floor no real filesystem clears, and a floor every filesystem clears. */
const MAX_PERSISTED_BYTES = "9223372036854775807";
const CLEARED_FLOOR = 1;
const MINUTE_SECONDS = 60;
const HOUR_SECONDS = 3600;
const DAY_SECONDS = 24 * HOUR_SECONDS;

/** The canonical fixture supplies the healthy Node process probe the minimal one
 * disables; without it a Report carries no Node metric evidence at all. */
const CANONICAL_FIXTURE =
  "../crates/platpulse-core/tests/fixtures/report_v1_canonical.json";
const REPORT_FIXTURE =
  "../crates/platpulse-core/tests/fixtures/report_v1_minimal.json";

/** The six families, in the order every answer names them. */
const SOURCE_ORDER = [
  "node_metrics",
  "node_state",
  "host_metrics",
  "peers",
  "incidents",
  "validator",
];

/** The clock each family states its own evidence in (design §11.4). */
const SOURCE_TIME_BASES = [
  "metric_observation",
  "state_observation",
  "metric_observation",
  "peer_receipt_bucket",
  "incident_evaluation",
  "validator_snapshot_source",
];

/** The window presets the Server offers, in the order the page must render them. */
const PRESET_KEYS = ["1h", "6h", "24h", "7d", "30d"];
const PRESET_LABELS = [
  "Last hour",
  "Last 6 hours",
  "Last 24 hours",
  "Last 7 days",
  "Last 30 days",
];

/** A clock of whole-second instants a chosen age before one fixed now: the spans
 * the Server measures are then exactly the spans this spec states. */
function clock(): (secondsAgo: number) => string {
  const now = Math.floor(Date.now() / 1000);
  return (secondsAgo: number): string =>
    new Date((now - secondsAgo) * 1000).toISOString().replace(/\.\d{3}Z$/, "Z");
}

/** The same clock, ahead of now, for a range that ends in the future. */
function futureInstant(seconds: number): string {
  return new Date((Math.floor(Date.now() / 1000) + seconds) * 1000)
    .toISOString()
    .replace(/\.\d{3}Z$/, "Z");
}

/** The instant as the Server's own rendering prints it, so a panel is judged
 * against the answer rather than against a format this spec chose. */
function printedInstant(instant: string): string {
  return instant.slice(0, 19).replace("T", " ") + " UTC";
}

function printedRange(from: string, to: string): string {
  return printedInstant(from) + " to " + printedInstant(to);
}

function fixtureNodeId(): string {
  const report = JSON.parse(readFileSync(REPORT_FIXTURE, "utf8")) as {
    nodes: { node_id: string }[];
  };
  return report.nodes[0].node_id;
}

function enableNodeProcess(report: Record<string, unknown>) {
  const canonicalFixture = JSON.parse(
    readFileSync(CANONICAL_FIXTURE, "utf8"),
  ) as {
    nodes: { process: unknown }[];
  };
  const nodes = report.nodes as Record<string, unknown>[];
  nodes[0].process = canonicalFixture.nodes[0].process;
}

/** Stamp a genuinely new Report: a fresh id and sequence so the Server stores a
 * new observation instead of answering an exact replay from its receipt. */
function stamp(
  report: Record<string, unknown>,
  observed: string,
  sequence: number,
) {
  report.report_sequence = sequence;
  report.report_id =
    "0195f2a1-0091-4091-8091-0000000000" + String(sequence).padStart(2, "0");
  report.generated_at = observed;
}

/** One new Report whose Node process observation is stamped at a real instant and
 * carries a chosen CPU reading. */
function readingsReport(
  observed: string,
  sequence: number,
  cpuPercent: number,
) {
  return (report: Record<string, unknown>) => {
    enableNodeProcess(report);
    stamp(report, observed, sequence);
    const nodes = report.nodes as Record<string, unknown>[];
    const component = nodes[0].process as Record<string, unknown>;
    component.attempted_at = observed;
    component.latest_observed_at = observed;
    (component.latest as Record<string, unknown>).cpu_percent = cpuPercent;
  };
}

/** One new Report carrying the Node's own sync probe, so the recorded state log
 * holds one delivery inside the window. */
function syncReport(observed: string, sequence: number) {
  return (report: Record<string, unknown>) => {
    stamp(report, observed, sequence);
    const nodes = report.nodes as Record<string, unknown>[];
    const chain = nodes[0].chain as Record<string, unknown>;
    const sync = chain.sync as Record<string, unknown>;
    sync.status = "ok";
    sync.attempted_at = observed;
    sync.latest_observed_at = observed;
    (sync.latest as Record<string, unknown>).syncing = false;
  };
}

/** One new Report in which the Node reports its Peers as healthy. A receipt
 * bucket exists only when Peers were healthy, so this is the only shape that can
 * put a bucket in the window. */
function peersReport(observed: string, sequence: number, peers: number) {
  return (report: Record<string, unknown>) => {
    stamp(report, observed, sequence);
    const nodes = report.nodes as Record<string, unknown>[];
    const chain = nodes[0].chain as Record<string, unknown>;
    chain.peers = {
      status: "ok",
      attempted_at: observed,
      latest_observed_at: observed,
      state_revision: 1,
      value_revision: 1,
      latest: {
        peers: Array.from({ length: peers }, (_unused, index) => ({
          peer_id: "peer-" + String(index + 1).padStart(2, "0"),
          direction: index % 2 === 0 ? "outbound" : "inbound",
          trusted: true,
          static_peer: false,
          consensus_peer: true,
          caps: ["platon/1"],
          cbft_protocol_version: 4,
          cbft_commit_block: 1200,
        })),
      },
    };
  };
}

/** One Alert catalog row and one Incident occurrence, seeded while the database
 * is still closed so the Server boots onto it. */
function seedIncident(
  subjectKey: string,
  incidentId: string,
  openedAt: string,
): string {
  const evidence =
    '{"observedAt":"' + openedAt + '","message":"seeded acceptance evidence"}';
  return [
    "INSERT OR IGNORE INTO alert_rules (rule_key, enabled, severity, version, condition_json, created_at, updated_at) VALUES ('node.rpc_unreachable', 1, 'warning', 1, '{}', '" +
      openedAt +
      "', '" +
      openedAt +
      "');",
    "INSERT INTO alert_incidents (incident_id, rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, resolved_at, opened_evidence_json, resolved_evidence_json) VALUES",
    "  ('" +
      incidentId +
      "', 'node.rpc_unreachable', 1, 'node', '" +
      subjectKey +
      "', 'warning', 'open', 1, '" +
      openedAt +
      "', NULL, '" +
      evidence +
      "', NULL);",
  ].join("\n");
}

/** Declare a new low-space floor in the Server's configuration file. The policy
 * is read at startup, so the Server is restarted on the same state directory. */
function declareFloor(stateDir: string, floorBytes: number | string) {
  const path = join(stateDir, "server.toml");
  const lines = readFileSync(path, "utf8").split("\n");
  const start = lines.indexOf("[capacity]");
  expect(start, "the harness declares a [capacity] section").toBeGreaterThan(
    -1,
  );
  let end = start + 1;
  while (end < lines.length && !lines[end].startsWith("[")) end += 1;
  lines.splice(
    start,
    end - start,
    "[capacity]",
    "enabled = true",
    "pause_below_bytes = " + floorBytes,
    "resume_above_bytes = " + floorBytes,
    "sample_interval_seconds = 5",
  );
  writeFileSync(path, lines.join("\n"));
}

function investigationPath(nodeId: string, query = ""): string {
  return "/admin/nodes/" + nodeId + "/investigation" + query;
}

/** The Server route the same window is answered at. The page and the API share the
 * last path segment, so the two are never interchangeable. */
function investigationApiPath(nodeId: string, query = ""): string {
  return "/api/admin/v1/nodes/" + nodeId + "/investigation" + query;
}

/** Read the Owner-only investigation answer over real HTTP. */
async function readInvestigation(
  server: DisposableServer,
  nodeId: string,
  query = "",
): Promise<InvestigationResponse> {
  const response = await server.adminGet(investigationApiPath(nodeId, query));
  expect(response.status, "the Server answers " + query).toBe(200);
  return response.body as InvestigationResponse;
}

function family(
  answer: InvestigationResponse,
  key: string,
): InvestigationSourceResponse {
  const found = answer.sources.find((source) => source.key === key);
  expect(found, "the answer carries the " + key + " family").toBeTruthy();
  return found as InvestigationSourceResponse;
}

/** The card of one family, so no assertion can read another family's figures. */
function familyCard(page: Page, key: string) {
  return page.locator(
    '[data-slot="investigation-source"][data-source="' + key + '"]',
  );
}

/** Tell the Server's own investigation route apart from the WebUI page of the same
 * name. A browser navigates to the page, so a waiting response matched on the path
 * suffix alone would read the page's HTML document instead of the evidence: only the
 * API answers under /api/admin/v1. */
function isInvestigationAnswer(response: Response): boolean {
  const { pathname } = new URL(response.url());
  return (
    response.request().method() === "GET" &&
    pathname.startsWith("/api/admin/v1/") &&
    pathname.endsWith("/investigation")
  );
}

/** One seeded Incident, so the located-occurrence entrance and the Incident
 * family have something real to find. */
const INCIDENT_ID = "0195f2a1-0200-4200-8200-000000000200";

test.describe("Node investigation over one UTC window (issue #220)", () => {
  test("answers one window over every family of evidence and reads one clock on every viewport", async ({
    browser,
  }, testInfo) => {
    // The disposable Server and the full viewport matrix are exercised once: the
    // other projects in the matrix re-run the same Server-side flow, which this
    // spec proves over real HTTP instead.
    test.skip(
      testInfo.project.name !== "desktop-1280",
      "the acceptance flow runs once",
    );
    test.setTimeout(600_000);

    const nodeId = fixtureNodeId();
    const ago = clock();
    const server = await startDisposableServer({
      seedSql: seedIncident(nodeId, INCIDENT_ID, ago(3 * HOUR_SECONDS)),
    });
    const context = await browser.newContext({
      hasTouch: true,
      colorScheme: "light",
    });

    try {
      const agent = await server.enrollAgent();

      // Evidence with two genuine silences: a fourteen-hour gap between the CPU
      // readings, and a newest reading six hours before the window ends. Neither
      // may be reported as zero.
      await server.submitReport(
        agent.agentId,
        agent.credential,
        readingsReport(ago(20 * HOUR_SECONDS), 1, 11),
      );
      await server.submitReport(
        agent.agentId,
        agent.credential,
        readingsReport(ago(20 * HOUR_SECONDS - 2 * MINUTE_SECONDS), 2, 12),
      );
      await server.submitReport(
        agent.agentId,
        agent.credential,
        readingsReport(ago(6 * HOUR_SECONDS), 3, 21),
      );
      await server.submitReport(
        agent.agentId,
        agent.credential,
        readingsReport(ago(6 * HOUR_SECONDS - 2 * MINUTE_SECONDS), 4, 22),
      );
      await server.submitReport(
        agent.agentId,
        agent.credential,
        syncReport(ago(23 * HOUR_SECONDS), 5),
      );

      // Two Peer reports received back to back collapse into one receipt bucket,
      // because a bucket is the Server's receipt window and not an Agent point.
      const peersAt = ago(5 * HOUR_SECONDS);
      await server.submitReport(
        agent.agentId,
        agent.credential,
        peersReport(peersAt, 6, 6),
      );
      await server.submitReport(
        agent.agentId,
        agent.credential,
        peersReport(peersAt, 7, 8),
      );
      expect(agent.agentId, "the Agent is enrolled on this Node").toBeTruthy();

      const answer = await readInvestigation(server, nodeId, "?window=24h");

      // The default window is the Server's own answer, adjustable, and every
      // preset it offers is named by the Server rather than fixed in the WebUI.
      expect(answer.window.preset).toBe("24h");
      expect(answer.window.presetLabel).toBe("Last 24 hours");
      expect(answer.window.custom).toBe(false);
      expect(answer.window.durationSeconds).toBe(24 * HOUR_SECONDS);
      expect(answer.window.clampedToNow).toBe(false);
      expect(answer.window.answeredAt).toBe(answer.window.to);
      expect(answer.window.requestedFrom).toBe(answer.window.from);
      expect(answer.window.requestedTo).toBe(answer.window.to);
      expect(
        answer.window.supportedPresets.map((preset) => preset.key),
      ).toEqual(PRESET_KEYS);
      expect(
        answer.window.supportedPresets.map((preset) => preset.label),
      ).toEqual(PRESET_LABELS);
      expect(
        answer.window.supportedPresets.map((preset) => preset.hours),
      ).toEqual([1, 6, 24, 168, 720]);
      expect(answer.window.horizonDays).toBe(30);
      expect(answer.window.rawRetentionDays).toBe(1);
      expect(
        answer.notes.length,
        "the answer states the rule every family answers by",
      ).toBeGreaterThan(0);

      // Six families, in the one order, each on its own clock.
      expect(answer.sources.map((source) => source.key)).toEqual(SOURCE_ORDER);
      expect(answer.sources.map((source) => source.timeBasis)).toEqual(
        SOURCE_TIME_BASES,
      );
      for (const source of answer.sources) {
        expect(source.label.length).toBeGreaterThan(0);
        expect(source.coverageLabel.length).toBeGreaterThan(0);
        expect(source.timeBasisLabel.length).toBeGreaterThan(0);
        expect(source.subjectKind.length).toBeGreaterThan(0);
        expect(source.subject.length).toBeGreaterThan(0);
        for (const path of source.answerPaths) {
          expect(path.path, "every answer path is an Admin route").toContain(
            "/api/admin/v1/",
          );
          expect(path.label.length).toBeGreaterThan(0);
        }
        if (source.coverage === "unsupported") {
          expect(
            source.answerPaths,
            "a family that does not apply offers no answer path",
          ).toHaveLength(0);
          expect(
            source.notes.length,
            "a family that does not apply says why",
          ).toBeGreaterThan(0);
        }
      }

      // Node metrics: the two silences are proved as silences, with the boundary
      // that names the failed collection, and the path to the fuller history.
      const metrics = family(answer, "node_metrics");
      expect(metrics.coverage).toBe("partial");
      expect(metrics.timeBasis).toBe("metric_observation");
      expect(metrics.subjectKind).toBe("node");
      expect(metrics.subject).toBe(nodeId);
      const raw = metrics.grains.find((grain) => grain.grain === "raw");
      expect(
        raw?.available,
        "the raw grain holds the readings this window covers",
      ).toBe(true);
      expect(raw?.pointCount ?? 0).toBeGreaterThanOrEqual(4);
      // The raw grain is read from stored points, so it never claims a grid.
      expect(raw?.expectedPoints ?? null).toBeNull();
      expect(
        (raw?.holes ?? []).some(
          (hole) =>
            hole.series === "process_cpu_percent" &&
            (hole.seconds ?? 0) > 12 * HOUR_SECONDS,
        ),
        "the longest interruption is named on the series it belongs to",
      ).toBe(true);
      expect(
        metrics.boundaries.some(
          (boundary) => boundary.kind === "collection_failure",
        ),
        "a silence outside the grace is a collection gap",
      ).toBe(true);
      expect(
        metrics.boundaries.find(
          (boundary) => boundary.kind === "collection_failure",
        )?.detail,
      ).toContain("process_cpu_percent");
      expect(
        metrics.boundaries.some((boundary) => boundary.kind === "stale_tail"),
        "a newest reading six hours old leaves a stale tail",
      ).toBe(true);
      expect(
        metrics.answerPaths.some((path) =>
          path.path.includes("/metric-history"),
        ),
      ).toBe(true);

      // Recorded Node state answers on its own clock, and it is not the metric
      // clock: one delivery a minute apart is not a 15-minute silence.
      const state = family(answer, "node_state");
      expect(state.timeBasis).toBe("state_observation");
      expect(state.coverage === "unsupported").toBe(false);
      expect(state.grains.length).toBeGreaterThan(0);

      // Host evidence belongs to the Agent that collected it, not to the Node.
      const host = family(answer, "host_metrics");
      expect(host.subjectKind).toBe("agent");
      expect(host.subject).toBe(agent.agentId);
      expect(host.coverage === "unsupported").toBe(false);

      // Peer evidence is a receipt bucket: two reports in one bucket are two
      // samples of one point, not two points.
      const peers = family(answer, "peers");
      expect(peers.timeBasis).toBe("peer_receipt_bucket");
      expect(peers.coverage === "unsupported").toBe(false);
      const bucket = peers.grains.find(
        (grain) => grain.grain === "receipt_bucket_5m",
      );
      expect(bucket?.available).toBe(true);
      expect(bucket?.grainSeconds).toBe(300);
      expect(bucket?.pointCount ?? 0).toBe(1);
      expect(bucket?.sampleCount ?? 0).toBeGreaterThanOrEqual(2);
      expect(
        peers.answerPaths.some((path) => path.path.includes("/peer-history")),
      ).toBe(true);

      // Incidents are evaluated by the Server from its own history.
      const incidents = family(answer, "incidents");
      expect(incidents.timeBasis).toBe("incident_evaluation");
      expect(incidents.grains.map((grain) => grain.grain)).toContain(
        "occurrence",
      );
      expect(
        incidents.grains.find((grain) => grain.grain === "occurrence")
          ?.pointCount,
      ).toBe(1);

      // No Validator is linked, so the family says so instead of reporting zero.
      const validator = family(answer, "validator");
      expect(validator.coverage).toBe("unsupported");
      expect(validator.coverageLabel).toBe("Not applicable");
      expect(validator.notes.join(" ")).toContain(
        "no Validator is linked to this Node",
      );
      expect(validator.answerPaths).toHaveLength(0);

      // A window the Server cannot read is refused with vocabulary of its own, and
      // no part of a refused window is guessed: the page shows this reason instead
      // of substituting a window the reader never asked for.
      const refusal = async (query: string): Promise<string> => {
        const body = (await server.expectAdminGet(
          investigationApiPath(nodeId, query),
          400,
        )) as {
          error?: { code?: string; message?: string };
        };
        expect(body.error?.code, "the refusal code for " + query).toBe(
          "invalid_investigation_window",
        );
        return body.error?.message ?? "";
      };
      const from = encodeURIComponent(answer.window.from);
      const to = encodeURIComponent(answer.window.to);
      expect(await refusal("?window=24h&from=" + from + "&to=" + to)).toContain(
        "not both",
      );
      expect(await refusal("?from=" + from)).toContain(
        "needs both from and to",
      );
      expect(await refusal("?from=" + to + "&to=" + from)).toContain(
        "ends at or before it starts",
      );
      expect(
        await refusal(
          "?from=" +
            encodeURIComponent(ago(10 * MINUTE_SECONDS)) +
            "&to=" +
            encodeURIComponent(ago(5 * MINUTE_SECONDS)),
        ),
      ).toContain("supported minimum is 1 hour");
      expect(
        await refusal(
          "?from=" +
            encodeURIComponent(ago(31 * DAY_SECONDS)) +
            "&to=" +
            encodeURIComponent(ago(0)),
        ),
      ).toContain("supported maximum is 720 hours");
      expect(
        await refusal(
          "?from=" +
            encodeURIComponent(futureInstant(HOUR_SECONDS)) +
            "&to=" +
            encodeURIComponent(futureInstant(2 * HOUR_SECONDS)),
        ),
      ).toContain("the window lies in the future");
      expect(await refusal("?window=31d")).toContain(
        "is not a supported window",
      );

      // A range that reaches past this instant is answered with the part the
      // Server could have observed, and the clamping is stated rather than hidden.
      const clampedFrom = ago(6 * HOUR_SECONDS);
      const clampedTo = futureInstant(2 * HOUR_SECONDS);
      const clamped = await readInvestigation(
        server,
        nodeId,
        "?from=" +
          encodeURIComponent(clampedFrom) +
          "&to=" +
          encodeURIComponent(clampedTo),
      );
      expect(clamped.window.preset).toBe("custom");
      expect(clamped.window.custom).toBe(true);
      expect(clamped.window.clampedToNow).toBe(true);
      expect(clamped.window.requestedFrom).toBe(clampedFrom);
      expect(clamped.window.requestedTo).toBe(clampedTo);
      expect(Date.parse(clamped.window.to)).toBeLessThanOrEqual(
        Date.now() + 1_000,
      );
      // The asked-for start is kept exactly and the future end becomes this
      // instant, so the width is measured against the answered end rather than
      // assumed to still be the six hours the request named: a request takes time
      // to travel and is answered at the instant the Server answers it.
      expect(clamped.window.from).toBe(clampedFrom);
      expect(clamped.window.durationSeconds).toBe(
        (Date.parse(clamped.window.to) - Date.parse(clampedFrom)) / 1000,
      );
      expect(
        Date.parse(clamped.window.to) - Date.parse(clampedFrom),
      ).toBeGreaterThanOrEqual(6 * HOUR_SECONDS * 1000);

      // The production WebUI: one logged-in context (hasTouch keeps the
      // small-viewport tap path), every viewport the Admin matrix fixes, in both
      // resolved themes. Each cell reads the answer over real HTTP first, so
      // every figure on screen is judged against the Server's own record.
      const page = await context.newPage();
      await loginToDisposableServer(page, server.baseUrl);

      for (const viewport of VIEWPORTS) {
        for (const colorScheme of ["light", "dark"] as const) {
          const where =
            viewport.width + "x" + viewport.height + " " + colorScheme;
          await page.setViewportSize({
            width: viewport.width,
            height: viewport.height,
          });
          await page.emulateMedia({ colorScheme });
          const [read] = await Promise.all([
            page.waitForResponse((response) => isInvestigationAnswer(response)),
            gotoAuthenticated(page, server.baseUrl, investigationPath(nodeId)),
          ]);
          expect(read.status(), where).toBe(200);
          const rendered = (await read.json()) as InvestigationResponse;
          await expectResolvedTheme(page, colorScheme);

          // The window the page reads is the window the Server answered, named by
          // the Server's own preset list rather than by this browser's clock.
          await expect(
            page.getByRole("heading", { level: 1 }),
            where,
          ).toContainText("Node investigation");
          await expect(
            page.getByText("Last 24 hours (the Server resolved it)"),
          ).toBeVisible();
          await expect(
            page
              .getByText(printedRange(rendered.window.from, rendered.window.to))
              .first(),
            where + ": the answered range",
          ).toBeVisible();
          await expect(page.getByText("24 hours").first()).toBeVisible();
          await expect(page.getByText("30 days").first()).toBeVisible();

          // Every family answers on the page with its own coverage and its own clock.
          await expect(
            page.getByRole("heading", { name: "Coverage of this window" }),
          ).toBeVisible();
          await expect(
            page.getByRole("heading", { name: "Peer receipt evidence" }),
          ).toBeVisible();
          await expect(
            page.getByRole("heading", { name: "Node metrics in this window" }),
          ).toBeVisible();
          await expect(
            page.getByRole("heading", {
              name: "Recorded Node state in this window",
            }),
          ).toBeVisible();
          await expect(
            page.locator('[data-slot="investigation-source"]'),
          ).toHaveCount(SOURCE_ORDER.length);
          for (const source of rendered.sources) {
            const card = familyCard(page, source.key);
            await expect(
              card.locator("summary"),
              where + ": " + source.key,
            ).toContainText(source.label);
            await expect(card.locator("summary")).toContainText(
              source.coverageLabel + " · " + source.timeBasisLabel,
            );
          }

          // The Node metrics family says which part of the window it cannot
          // answer, on the series that went silent, instead of reporting zero.
          const metricsCard = familyCard(page, "node_metrics");
          await metricsCard.locator("summary").click();
          await expect(
            metricsCard.locator('[data-slot="investigation-source-verdict"]'),
          ).toContainText("Part of this window cannot be answered");
          await expect(
            metricsCard.locator('[data-slot="investigation-holes"]'),
          ).toContainText("process_cpu_percent");
          await expect(
            metricsCard.locator('[data-slot="investigation-holes"]'),
          ).toContainText("is silent from");
          await expect(
            metricsCard.locator('[data-slot="investigation-boundaries"]'),
          ).toContainText("Collection gap");
          await expect(
            metricsCard.locator('[data-slot="investigation-boundaries"]'),
          ).toContainText("Newest evidence is older than the window end");

          // A family that does not apply says so, in words rather than a figure.
          const validatorCard = familyCard(page, "validator");
          await validatorCard.locator("summary").click();
          await expect(
            validatorCard.locator('[data-slot="investigation-source-verdict"]'),
          ).toContainText("This family does not apply to this Node");
          await expect(
            validatorCard.locator('[data-slot="investigation-source-notes"]'),
          ).toContainText("no Validator is linked to this Node");
          await expect(
            validatorCard.locator('[data-slot="investigation-no-boundary"]'),
          ).toBeVisible();

          // The Peer panel reads receipt buckets: the two reports that arrived in
          // one bucket are samples of one bucket, and no bucket is claimed missing.
          await expect(
            page.locator('[data-slot="investigation-peer-grain"]'),
          ).toBeVisible();
          await expect(
            page.locator('[data-slot="investigation-peer-range"]'),
          ).toContainText(
            /\d+ of \d+ buckets this window covers are in this answer/,
          );
          await expect(
            page.locator('[data-slot="investigation-peer-buckets"]'),
          ).toContainText("Bucket start (receipt)");
          await expect(
            page.locator('[data-slot="investigation-peer-buckets"]'),
          ).toContainText("CFBT lag (blocks)");

          // A wide table scrolls inside its own frame rather than moving the page.
          await expectLocalTableScroll(page, "investigation-sources");
          await expectLocalTableScroll(page, "investigation-peer-buckets");
          await expectLocalTableScroll(
            page,
            "investigation-grains",
            '[data-slot="investigation-source"][data-source="node_metrics"]',
          );
          await expectLocalTableScroll(
            page,
            "investigation-boundaries",
            '[data-slot="investigation-source"][data-source="node_metrics"]',
          );

          // The windowed Incident reader is entered with the window this page read
          // (the answer captured above, not the earlier probe), so the list applies
          // the occurrence predicate the Server applied here.
          expect(
            await page
              .getByRole("link", {
                name: "Incidents for this Node in this window",
              })
              .getAttribute("href"),
            where + ": the windowed Incident entry",
          ).toBe(
            "/admin/alerts/incidents?subject=node&subject_key=" +
              nodeId +
              "&from=" +
              encodeURIComponent(rendered.window.from) +
              "&to=" +
              encodeURIComponent(rendered.window.to),
          );

          if (colorScheme === "light") {
            // Reloading re-reads the window the link names: the width is answered
            // by the Server again instead of being recomputed here.
            const [reloaded] = await Promise.all([
              page.waitForResponse((response) =>
                isInvestigationAnswer(response),
              ),
              page.locator('[data-slot="investigation-reload"]').click(),
            ]);
            expect(reloaded.status()).toBe(200);
            expect(new URL(page.url()).searchParams.get("window")).toBeNull();
            await expect(
              page.getByText("Last 24 hours (the Server resolved it)"),
            ).toBeVisible();

            // A range typed here is sent as the window to read, and the Server names
            // the width it will not read instead of this page guessing at the ends.
            const control = page.locator('[data-slot="investigation-window"]');
            await control.getByLabel("From").fill("2026-01-02T00:00");
            await control.getByLabel("To").fill("2026-01-02T00:00");
            const [typedRange] = await Promise.all([
              page.waitForResponse((response) =>
                isInvestigationAnswer(response),
              ),
              page.getByRole("button", { name: "Read this range" }).click(),
            ]);
            expect(typedRange.status()).toBe(400);
            const typedRefusal = page.locator(
              '[data-slot="investigation-window-refused"]',
            );
            await expect(typedRefusal).toContainText(
              "ends at or before it starts",
            );
            await expect(typedRefusal).toContainText("Nothing was read for it");
          }

          if (viewport.width <= 768) {
            // Keyboard, focus and touch on a freshly loaded page, so the tab order
            // is the one an Operator meets.
            await gotoAuthenticated(
              page,
              server.baseUrl,
              investigationPath(nodeId),
            );
            await expectVisibleInteractiveTargets(page);
            const preset = page.getByRole("button", {
              name: "Last hour",
              exact: true,
            });
            const focused = await focusByKeyboard(page, preset);
            await expect(focused).toBeFocused();
            await expectFocusedElementHasVisibleFocus(page);
            const [chosen] = await Promise.all([
              page.waitForResponse((response) =>
                response.url().includes("window=1h"),
              ),
              preset.tap(),
            ]);
            const presetAnswer = (await chosen.json()) as InvestigationResponse;
            expect(new URL(page.url()).searchParams.get("window"), where).toBe(
              "1h",
            );
            expect(presetAnswer.window.preset).toBe("1h");
            expect(presetAnswer.window.durationSeconds).toBe(HOUR_SECONDS);
            await expect(
              page.getByText("Last hour (the Server resolved it)"),
            ).toBeVisible();
            await expect(
              page
                .getByText(
                  printedRange(
                    presetAnswer.window.from,
                    presetAnswer.window.to,
                  ),
                )
                .first(),
            ).toBeVisible();
            await expectNoHorizontalOverflow(page);
          }
        }
      }
    } finally {
      await context.close();
      await server.dispose();
    }
  });

  test("reads the window a link names, and reads the Server default instead of inventing one", async ({
    browser,
  }, testInfo) => {
    test.skip(
      testInfo.project.name !== "desktop-1280",
      "the acceptance flow runs once",
    );
    test.setTimeout(600_000);

    const nodeId = fixtureNodeId();
    const ago = clock();
    const server = await startDisposableServer();
    const context = await browser.newContext({
      hasTouch: true,
      colorScheme: "light",
    });

    try {
      const agent = await server.enrollAgent();
      await server.submitReport(
        agent.agentId,
        agent.credential,
        readingsReport(ago(2 * HOUR_SECONDS), 1, 21),
      );
      await server.submitReport(
        agent.agentId,
        agent.credential,
        syncReport(ago(2 * HOUR_SECONDS), 2),
      );
      await server.submitReport(
        agent.agentId,
        agent.credential,
        peersReport(ago(2 * HOUR_SECONDS), 3, 3),
      );

      const page = await context.newPage();
      await loginToDisposableServer(page, server.baseUrl);

      /** Read one linked window, handing back the Server's own answer and the exact
       * address the page asked it at — the second is what proves nothing was invented. */
      async function load(query: string) {
        const [sent] = await Promise.all([
          page.waitForResponse((response) => isInvestigationAnswer(response)),
          gotoAuthenticated(
            page,
            server.baseUrl,
            investigationPath(nodeId, query),
          ),
        ]);
        return {
          answer: (await sent.json()) as InvestigationResponse,
          url: new URL(sent.url()),
        };
      }

      // 1. A preset the link names is the window that is read, and the page names it
      // as the Server resolved it rather than as a width this browser derived.
      const named = await load("?window=7d");
      expect(named.url.searchParams.get("window")).toBe("7d");
      expect(named.answer.window.preset).toBe("7d");
      expect(named.answer.window.presetLabel).toBe("Last 7 days");
      expect(named.answer.window.durationSeconds).toBe(7 * DAY_SECONDS);
      expect(named.answer.window.custom).toBe(false);
      expect(new URL(page.url()).searchParams.get("window")).toBe("7d");
      await expect(
        page.getByText("Last 7 days (the Server resolved it)"),
      ).toBeVisible();
      await expect(
        page
          .getByText(
            printedRange(named.answer.window.from, named.answer.window.to),
          )
          .first(),
      ).toBeVisible();
      await expect(
        page.locator('[data-slot="investigation-window-refused"]'),
      ).toHaveCount(0);

      // 2. A link that names a window the Server will not read is sent as the link
      // writes it: the refusal belongs to the address that was followed, and the page
      // shows the Server's own reason instead of reading some other window in its
      // place. The controls stay on screen, so another window can be asked for.
      const refusals = [
        {
          query: "?window=7d&from=" + ago(DAY_SECONDS) + "&to=" + ago(0),
          reason: "not both",
          params: [
            ["window", "7d"],
            ["from", ago(DAY_SECONDS)],
            ["to", ago(0)],
          ],
        },
        {
          query: "?from=" + ago(DAY_SECONDS),
          reason: "needs both from and to",
          params: [["from", ago(DAY_SECONDS)]],
        },
        {
          query: "?from=soon&to=later",
          reason: "is not a canonical RFC 3339 instant",
          params: [
            ["from", "soon"],
            ["to", "later"],
          ],
        },
        {
          query: "?from=" + ago(0) + "&to=" + ago(DAY_SECONDS),
          reason: "ends at or before it starts",
          params: [
            ["from", ago(0)],
            ["to", ago(DAY_SECONDS)],
          ],
        },
        {
          query: "?from=" + ago(10 * MINUTE_SECONDS) + "&to=" + ago(0),
          reason: "supported minimum is 1 hour",
          params: [
            ["from", ago(10 * MINUTE_SECONDS)],
            ["to", ago(0)],
          ],
        },
        {
          query: "?from=" + ago(31 * DAY_SECONDS) + "&to=" + ago(0),
          reason: "supported maximum is 720 hours",
          params: [
            ["from", ago(31 * DAY_SECONDS)],
            ["to", ago(0)],
          ],
        },
      ];

      for (const refusal of refusals) {
        const asked = await load(refusal.query);
        expect(asked.url.pathname, refusal.query).toBe(
          "/api/admin/v1/nodes/" + nodeId + "/investigation",
        );
        expect(
          (asked.answer as { error?: { code?: string } }).error?.code,
          refusal.query,
        ).toBe("invalid_investigation_window");
        expect(asked.url.searchParams.size, refusal.query).toBe(
          refusal.params.length,
        );
        for (const [key, value] of refusal.params) {
          expect(
            asked.url.searchParams.get(key),
            refusal.query + " " + key,
          ).toBe(value);
        }
        const notice = page.locator(
          '[data-slot="investigation-window-refused"]',
        );
        await expect(notice, refusal.query).toContainText(refusal.reason);
        await expect(notice, refusal.query).toContainText(
          "Nothing was read for it",
        );
        await expect(
          page.locator('[data-slot="investigation-source"]'),
          refusal.query,
        ).toHaveCount(0);
        await expect(
          page.locator('[data-slot="investigation-window"]'),
          refusal.query,
        ).toBeVisible();
        await expect(
          page.getByText("Last 24 hours (the Server resolved it)"),
          refusal.query,
        ).toHaveCount(0);
      }

      // 3. A range that ends in the future is read up to now, and the page says the end
      // was clipped rather than filling it with anything.
      const aheadTo = futureInstant(6 * HOUR_SECONDS);
      const ahead = await load("?from=" + ago(HOUR_SECONDS) + "&to=" + aheadTo);
      expect(ahead.answer.window.clampedToNow).toBe(true);
      expect(ahead.answer.window.requestedTo).toBe(aheadTo);
      expect(Date.parse(ahead.answer.window.to)).toBeLessThanOrEqual(
        Date.parse(ahead.answer.window.answeredAt),
      );
      const clamped = page.locator(
        '[data-slot="investigation-window-clamped"]',
      );
      await expect(clamped).toBeVisible();
      await expect(clamped).toContainText(
        printedInstant(ahead.answer.window.requestedTo),
      );
      await expect(clamped).toContainText(
        printedInstant(ahead.answer.window.to),
      );

      // 4. A preset key the Server does not support is refused by the Server, and the
      // page reports that refusal instead of rendering a window nobody answered.
      const [unsupported] = await Promise.all([
        page.waitForResponse((response) => isInvestigationAnswer(response)),
        gotoAuthenticated(
          page,
          server.baseUrl,
          investigationPath(nodeId, "?window=90d"),
        ),
      ]);
      expect(unsupported.status()).toBe(400);
      const failure = (await unsupported.json()) as { error: { code: string } };
      expect(failure.error.code).toBe("invalid_investigation_window");
      await expect(
        page.locator('[data-slot="investigation-window-refused"]'),
      ).toContainText("is not a supported window");
      // No window was answered, so no evidence is rendered; the controls stay on
      // screen so the reader can ask for a window the Server does read.
      await expect(
        page.locator('[data-slot="investigation-source"]'),
      ).toHaveCount(0);
      await expect(
        page.locator('[data-slot="investigation-window"]'),
      ).toBeVisible();

      // 5. A link that names no window at all asks the Server for its own default,
      // sending no parameter rather than a width this client chose.
      const bare = await load("");
      expect(bare.url.searchParams.size).toBe(0);
      expect(bare.answer.window.preset).toBe("24h");
      expect(bare.answer.window.durationSeconds).toBe(DAY_SECONDS);
      await expect(
        page.getByText("Last 24 hours (the Server resolved it)"),
      ).toBeVisible();
      await expect(
        page.locator('[data-slot="investigation-window-refused"]'),
      ).toHaveCount(0);
    } finally {
      await context.close();
      await server.dispose();
    }
  });

  test("opens on the Incident occurrence rather than on a clock, and locates it in every family", async ({
    browser,
  }, testInfo) => {
    test.skip(
      testInfo.project.name !== "desktop-1280",
      "the acceptance flow runs once",
    );
    test.setTimeout(600_000);

    const nodeId = fixtureNodeId();
    const ago = clock();
    const openedAt = ago(3 * HOUR_SECONDS);
    // The first evidence the Node ever reports is deliberately later than the
    // instant the Incident opened at: a disposable Server first sees the Node
    // while this test runs, so every honest answer about the located instant is
    // "before this evidence started" rather than a claim about it.
    const firstObserved = ago(3 * HOUR_SECONDS - 2 * MINUTE_SECONDS);
    const server = await startDisposableServer({
      seedSql: seedIncident(nodeId, INCIDENT_ID, openedAt),
    });
    const context = await browser.newContext({
      hasTouch: true,
      colorScheme: "light",
    });

    try {
      const agent = await server.enrollAgent();
      await server.submitReport(
        agent.agentId,
        agent.credential,
        readingsReport(firstObserved, 1, 11),
      );
      await server.submitReport(
        agent.agentId,
        agent.credential,
        readingsReport(ago(2 * HOUR_SECONDS), 2, 21),
      );

      const page = await context.newPage();
      await loginToDisposableServer(page, server.baseUrl);
      await gotoAuthenticated(
        page,
        server.baseUrl,
        "/admin/alerts/incidents/" + INCIDENT_ID,
      );

      // The entry states the window it will read: from the occurrence itself, at
      // least the narrowest width the Server answers, and carrying the Incident.
      const link = page.getByRole("link", {
        name: "Investigate this Node in this window",
      });
      await expect(link).toBeVisible();
      const href = new URL(
        (await link.getAttribute("href")) ?? "",
        server.baseUrl,
      );
      expect(href.pathname).toBe("/admin/nodes/" + nodeId + "/investigation");
      expect(href.searchParams.get("occurrence")).toBe(openedAt);
      expect(href.searchParams.get("from")).toBe(openedAt);
      expect(href.searchParams.get("incident")).toBe(INCIDENT_ID);
      const linkedTo = href.searchParams.get("to") ?? "";
      expect(
        Date.parse(linkedTo) - Date.parse(openedAt),
      ).toBeGreaterThanOrEqual(HOUR_SECONDS * 1000);

      // Following it reads that window from the Server, at the width the link asked
      // for rather than at a width this browser chose.
      const [sent] = await Promise.all([
        page.waitForResponse((response) => isInvestigationAnswer(response)),
        link.click(),
      ]);
      const read = (await sent.json()) as InvestigationResponse;
      expect(read.window.custom).toBe(true);
      expect(read.window.preset).toBe("custom");
      expect(read.window.requestedFrom).toBe(openedAt);
      expect(read.window.requestedTo).toBe(linkedTo);
      expect(read.window.from).toBe(openedAt);
      expect(read.window.clampedToNow).toBe(false);
      expect(read.window.durationSeconds).toBe(
        linkedTo === ""
          ? -1
          : (Date.parse(linkedTo) - Date.parse(openedAt)) / 1000,
      );
      expect(new URL(page.url()).pathname).toBe(
        "/admin/nodes/" + nodeId + "/investigation",
      );
      expect(new URL(page.url()).searchParams.get("occurrence")).toBe(openedAt);

      // The Incident that brought the Operator here stays reachable, and the page
      // says the window came from the occurrence rather than from this clock.
      const located = page.locator('[data-slot="investigation-occurrence"]');
      await expect(
        page.getByRole("heading", { name: "The located occurrence" }),
      ).toBeVisible();
      await expect(located).toContainText(
        "The Incident opened at " + printedInstant(openedAt),
      );
      await expect(
        page.locator('[data-slot="investigation-occurrence-entry"]'),
      ).toContainText("the width is the width the link asked for");
      await expect(
        page.getByRole("link", { name: "Back to the Incident" }),
      ).toHaveAttribute("href", "/admin/alerts/incidents/" + INCIDENT_ID);

      // Every family is located against that instant: the family the Incident itself
      // is evaluated by answers it, and the family that does not apply to this Node
      // says so instead of reading as evidence.
      const table = page.locator(
        '[data-slot="investigation-occurrence-table"]',
      );
      const rows = table.locator("tbody tr");
      await expect(rows).toHaveCount(SOURCE_ORDER.length);
      for (const key of SOURCE_ORDER) {
        await expect(table, key).toContainText(family(read, key).label);
      }
      const incidentFamily = family(read, "incidents");
      expect(
        incidentFamily.grains.some(
          (grain) => grain.grain === "occurrence" && grain.pointCount >= 1,
        ),
      ).toBe(true);
      await expect(rows.first()).toContainText(
        family(read, "node_metrics").label,
      );
      // The Incident family is evaluated by the Server itself, so its first
      // evaluation is later than the instant the Incident carries: the window
      // opens before the Server evaluated anything, and that is said rather than
      // smoothed over. The family that does not apply says so.
      await expect(rows.nth(4)).toContainText("Before this evidence started");
      await expect(rows.nth(4)).toContainText(
        "so the window opens before the Agent reported any of it",
      );
      await expect(rows.nth(5)).toContainText(
        "This family does not apply to this Node.",
      );
      expect(family(read, "validator").coverage).toBe("unsupported");

      // Every row repeats the Server's own answer for that instant: the boundary
      // that covers it, the silence the grain proves, nothing at all, or the plain
      // statement that the family does not apply. The located instant is read
      // through the same locator the page uses, so a row that claims evidence the
      // Server does not hold for that instant fails here.
      const locatedRows = await rows.allTextContents();
      read.sources.forEach((source, index) => {
        const located = sourceAtInstant(source, openedAt);
        const expected = !located.applicable
          ? "This family does not apply to this Node."
          : located.boundary
            ? located.boundary.kindLabel
            : located.hole
              ? "A silence this grain proves"
              : "No boundary and no silence of this family covers the instant.";
        expect(
          locatedRows[index],
          "the " +
            source.key +
            " row says what the Server says about " +
            openedAt,
        ).toContain(expected);
      });
      // The located instant sits two minutes before this family's first evidence
      // inside the window, so it is inside neither a boundary nor a silence: the
      // page must not turn that into a claim of stored evidence.
      await expect(rows.first()).toContainText(
        family(read, "node_metrics").label,
      );
      const metricLocated = sourceAtInstant(
        family(read, "node_metrics"),
        openedAt,
      );
      expect(metricLocated.answered).toBe(true);
      await expect(rows.first()).toContainText(
        "No boundary and no silence of this family covers the instant.",
      );
    } finally {
      await context.close();
      await server.dispose();
    }
  });

  test("names a low-space pause as a pause instead of a collection gap", async ({
    browser,
  }, testInfo) => {
    test.skip(
      testInfo.project.name !== "desktop-1280",
      "the acceptance flow runs once",
    );
    test.setTimeout(600_000);

    const nodeId = fixtureNodeId();
    const ago = clock();
    // A floor every real filesystem clears, so the observation taken before the
    // disk fills is genuinely stored rather than skipped.
    const server = await startDisposableServer({
      capacity: {
        pauseBelowBytes: CLEARED_FLOOR,
        resumeAboveBytes: CLEARED_FLOOR,
        sampleIntervalSeconds: 5,
      },
    });
    const context = await browser.newContext({
      hasTouch: true,
      colorScheme: "light",
    });

    /** Whether the Server is protecting the disk right now. */
    const readProtected = async (): Promise<boolean> => {
      const response = await server.adminGet("/api/admin/v1/capacity");
      expect(response.status, "the capacity policy is readable").toBe(200);
      return (response.body as { protected: boolean }).protected;
    };

    try {
      const agent = await server.enrollAgent();
      const beforePause = ago(40 * MINUTE_SECONDS);
      const duringPause = ago(35 * MINUTE_SECONDS);
      const afterPause = ago(2 * MINUTE_SECONDS);

      await server.submitReport(
        agent.agentId,
        agent.credential,
        readingsReport(beforePause, 1, 11),
      );

      // The operator declares a floor no real filesystem clears and restarts on the
      // same state directory: optional history pauses from here on.
      declareFloor(server.stateDir, MAX_PERSISTED_BYTES);
      await server.restart();
      await expect
        .poll(readProtected, {
          timeout: 30_000,
          message: "the declared floor pauses optional history",
        })
        .toBe(true);

      // A Report under pressure is still accepted in full: protection refuses
      // optional history, not the Report.
      const underPressure = await server.submitReport(
        agent.agentId,
        agent.credential,
        readingsReport(duringPause, 2, 12),
      );
      expect(underPressure.receipt.disposition).toBe("accepted");

      // The operator clears the floor and restarts: history resumes and the
      // protection interval closes.
      declareFloor(server.stateDir, CLEARED_FLOOR);
      await server.restart();
      await expect
        .poll(readProtected, {
          timeout: 30_000,
          message: "the cleared floor resumes optional history",
        })
        .toBe(false);

      await server.submitReport(
        agent.agentId,
        agent.credential,
        readingsReport(afterPause, 3, 21),
      );

      // The Server names the stretch it refused as a pause, and it does not report
      // the same stretch as a failed collection.
      const answer = await readInvestigation(server, nodeId, "?window=6h");
      expect(
        answer.notes.some((note) =>
          note.includes(
            "reported as pauses rather than as collection failures",
          ),
        ),
        "the answer says a refused sample is not a collection failure",
      ).toBe(true);
      const metrics = family(answer, "node_metrics");
      const paused = metrics.boundaries.filter(
        (boundary) =>
          boundary.kind === "low_space_pause" &&
          boundary.detail.includes("process_cpu_percent"),
      );
      expect(
        paused.length,
        "the refused series is named as a paused series",
      ).toBe(1);
      expect(paused[0].kindLabel).toBe("Optional history paused for low space");
      expect(paused[0].detail).toContain(
        "were not stored while the Server protected a nearly full disk",
      );
      expect(
        paused[0].detail.startsWith("1 optional process_cpu_percent sample(s)"),
      ).toBe(true);
      expect(paused[0].detail).toContain("protection closed");
      // The pause names the refused observation itself, so the reader can see which
      // instant inside the silence is accounted for, and the stretch it covers is the
      // silence between the stored observation and the one taken after it resumed.
      expect(Date.parse(paused[0].at)).toBeGreaterThan(Date.parse(beforePause));
      expect(Date.parse(paused[0].at)).toBeLessThan(Date.parse(afterPause));
      expect(
        metrics.boundaries.some(
          (boundary) =>
            boundary.kind === "collection_failure" &&
            boundary.detail.includes("process_cpu_percent"),
        ),
        "a sample refused by protection is not passed off as a collection failure",
      ).toBe(false);
      // Nothing already written was released to make room.
      const raw = metrics.grains.find((grain) => grain.grain === "raw");
      expect(raw?.available, "the raw grain still serves this window").toBe(
        true,
      );
      expect(raw?.pointCount).toBeGreaterThanOrEqual(2);

      // The same distinction reaches the Operator in the production WebUI: the
      // family reads "paused for low space", never "Collection gap".
      const page = await context.newPage();
      await loginToDisposableServer(page, server.baseUrl);
      await gotoAuthenticated(
        page,
        server.baseUrl,
        investigationPath(nodeId, "?window=6h"),
      );
      const card = familyCard(page, "node_metrics");
      await card.locator("summary").click();
      const boundaries = card.locator('[data-slot="investigation-boundaries"]');
      await expect(boundaries).toContainText(
        "Optional history paused for low space",
      );
      await expect(boundaries).toContainText("process_cpu_percent");
      await expect(boundaries).not.toContainText("Collection gap");
    } finally {
      await context.close();
      await server.dispose();
    }
  });
});
