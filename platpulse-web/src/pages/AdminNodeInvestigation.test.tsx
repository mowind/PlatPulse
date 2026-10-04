import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { adminQueryClient } from "../api/admin";
import { client } from "../api/generated/client.gen";

const TEST_ORIGIN = "http://platpulse.test";
const NODE_ID = "0195f2a1-0014-4014-8014-000000000014";
const AGENT_ID = "0195f2a1-0011-4011-8011-000000000011";
const WINDOW_FROM = "2026-08-11T12:00:00Z";
const WINDOW_TO = "2026-08-12T12:00:00Z";
const ANSWERED_AT = WINDOW_TO;
const OCCURRENCE = "2026-08-11T18:00:00Z";

const OWNER_SESSION = {
  session: {
    userId: "u1",
    username: "admin",
    role: "owner",
    createdAt: "2026-08-12T00:00:00Z",
    lastSeenAt: "2026-08-12T00:00:00Z",
    expiresAt: "2026-08-19T00:00:00Z",
  },
  csrfToken: "csrf-token",
};

/** The Server's own preset list, as an answer reports it. */
const SUPPORTED_PRESETS = [
  { key: "1h", label: "1 hour", hours: 1 },
  { key: "6h", label: "6 hours", hours: 6 },
  { key: "24h", label: "24 hours", hours: 24 },
  { key: "7d", label: "7 days", hours: 168 },
  { key: "30d", label: "30 days", hours: 720 },
];

/** The window the Server resolved an investigation over. */
function windowFixture(overrides: Record<string, unknown> = {}) {
  return {
    answeredAt: ANSWERED_AT,
    clampedToNow: false,
    custom: false,
    durationSeconds: 86400,
    from: WINDOW_FROM,
    horizonDays: 30,
    preset: "24h",
    presetLabel: "24 hours",
    rawRetentionDays: 1,
    requestedFrom: WINDOW_FROM,
    requestedTo: WINDOW_TO,
    supportedPresets: SUPPORTED_PRESETS,
    to: WINDOW_TO,
    ...overrides,
  };
}

/** One grain of one family. */
function grainFixture(overrides: Record<string, unknown> = {}) {
  return {
    available: true,
    cadenceSeconds: 300,
    expectedPoints: 288,
    firstObservedAt: WINDOW_FROM,
    from: WINDOW_FROM,
    grain: "5m",
    grainSeconds: 300,
    holes: [],
    lastObservedAt: WINDOW_TO,
    longestGapSeconds: 300,
    missingPoints: 0,
    note: null,
    pointCount: 240,
    sampleCount: 1200,
    to: WINDOW_TO,
    ...overrides,
  };
}

/** One family of evidence, in the shape the investigation answers it. */
function sourceFixture(overrides: Record<string, unknown> = {}) {
  return {
    answerPaths: [
      {
        label: "Metric history",
        note: null,
        path:
          "/api/admin/v1/nodes/" +
          NODE_ID +
          "/metric-history?from=" +
          WINDOW_FROM +
          "&to=" +
          WINDOW_TO,
      },
    ],
    boundaries: [],
    components: [],
    coverage: "complete",
    coverageLabel: "Complete",
    errorCode: null,
    firstObservedAt: WINDOW_FROM,
    grains: [grainFixture()],
    key: "node_metrics",
    label: "Node metrics",
    lastObservedAt: WINDOW_TO,
    lastReceivedAt: "2026-08-12T12:00:05Z",
    notes: [],
    relatedSubjects: [],
    releasedBefore: null,
    retainedFrom: null,
    retentionDays: 1,
    sourceState: "ok",
    subject: NODE_ID,
    subjectKind: "node",
    timeBasis: "metric_observation",
    timeBasisLabel: "Metric observation",
    truncated: false,
    ...overrides,
  };
}

function peersSourceFixture(overrides: Record<string, unknown> = {}) {
  return sourceFixture({
    key: "peers",
    label: "Peer receipts",
    timeBasis: "peer_receipt_bucket",
    timeBasisLabel: "Peer receipt bucket",
    grains: [
      grainFixture({
        expectedPoints: null,
        grain: "receipt_bucket_5m",
        missingPoints: 1,
        pointCount: 287,
      }),
    ],
    ...overrides,
  });
}

/** A whole investigation answer over one window. */
function answerFixture(overrides: Record<string, unknown> = {}) {
  return {
    agentId: AGENT_ID,
    components: [],
    displayName: "Node A",
    lifecycle: "active",
    nodeId: NODE_ID,
    notes: [],
    sources: [
      sourceFixture(),
      sourceFixture({
        key: "node_state",
        label: "Node state",
        timeBasis: "state_observation",
        timeBasisLabel: "State observation",
      }),
      peersSourceFixture(),
    ],
    visibility: "public",
    window: windowFixture(),
    ...overrides,
  };
}

/** One stored Peer receipt bucket. A bucket start IS the receipt instant. */
function peerBucket(bucketStart: string) {
  return {
    arrivals: 1,
    average_peers: 8.5,
    bucket_start: bucketStart,
    cbft_lag: { average: 4.5, maximum: 9, minimum: 1, sample_count: 12 },
    consensus_count: 6,
    countries: [{ count: 4, country_code: "DE" }],
    departures: 0,
    inbound_count: 5,
    known_country_count: 4,
    last_observed_at: bucketStart,
    outbound_count: 3,
    sample_count: 12,
    static_count: 2,
    total_peers: 9,
    trusted_count: 7,
    unknown_country_count: 5,
  };
}

/** One bounded read of a Node's Peer receipt buckets, newest first. */
function peerRangeFixture(overrides: Record<string, unknown> = {}) {
  return {
    buckets: [
      peerBucket("2026-08-12T11:55:00Z"),
      peerBucket("2026-08-12T11:50:00Z"),
    ],
    continuation: "2026-08-12T11:50:00Z",
    coverage: "partial",
    first_bucket: "2026-08-12T11:50:00Z",
    from: WINDOW_FROM,
    grain: "5m",
    grain_seconds: 300,
    last_bucket: "2026-08-12T11:55:00Z",
    matching: 300,
    missing_buckets: 1,
    returned: 2,
    to: WINDOW_TO,
    truncated: true,
    ...overrides,
  };
}

function peerHistoryFixture(overrides: Record<string, unknown> = {}) {
  return {
    five_minute: [],
    hourly: [],
    range: peerRangeFixture(),
    freshness: "current",
    state: "ok",
    ...overrides,
  };
}

/** The Node's own Process series over the answered window. */
function metricHistoryFixture() {
  return {
    scopeKind: "node",
    scopeKey: NODE_ID,
    nodeId: NODE_ID,
    metric: "process_cpu_percent",
    dimension: "",
    from: WINDOW_FROM,
    to: WINDOW_TO,
    requestedFrom: WINDOW_FROM,
    availability: null,
    rawRetentionDays: 1,
    grain: "raw",
    aggregateSupported: true,
    historyHorizonDays: 30,
    windowSeconds: 86400,
    truncated: false,
    continuation: null,
    segments: [
      {
        from: WINDOW_FROM,
        to: WINDOW_TO,
        grain: "raw",
        source: "raw",
        pointCount: 2,
        truncated: false,
      },
    ],
    series: {
      observed: true,
      firstObservedAt: "2026-08-12T10:00:00Z",
      lastObservedAt: "2026-08-12T11:00:00Z",
      lastReceivedAt: "2026-08-12T11:00:01Z",
      observationCount: 2,
      replayedCount: 0,
      correctedCount: 0,
      sampledCount: 2,
      coverageSeconds: 60,
      windowSeconds: 86400,
      latestDelaySeconds: 1,
      latestClockSuspect: false,
    },
    items: [
      {
        observedAt: "2026-08-12T10:00:00Z",
        receivedAt: "2026-08-12T10:00:01Z",
        value: 11,
        grain: "raw",
        source: "raw",
        minValue: 11,
        maxValue: 11,
        sampleCount: 1,
        lastObservedAt: "2026-08-12T10:00:00Z",
        delaySeconds: 1,
        clockSuspect: false,
      },
      {
        observedAt: "2026-08-12T11:00:00Z",
        receivedAt: "2026-08-12T11:00:01Z",
        value: 2.5,
        grain: "raw",
        source: "raw",
        minValue: 2.5,
        maxValue: 2.5,
        sampleCount: 1,
        lastObservedAt: "2026-08-12T11:00:00Z",
        delaySeconds: 1,
        clockSuspect: false,
      },
    ],
    gaps: [],
  };
}

/** Every request this page made, so a call can be read rather than assumed. */
const REQUESTS: string[] = [];

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function mockFetch(
  routes: Record<string, (request: Request) => Response | Promise<Response>>,
) {
  const fetchMock = vi.fn(
    async (input: RequestInfo | URL, init?: RequestInit) => {
      const request =
        input instanceof Request ? input : new Request(String(input), init);
      REQUESTS.push(request.url);
      const url = request.url.replace(TEST_ORIGIN, "");
      for (const [pattern, handler] of Object.entries(routes)) {
        if (pattern.endsWith("*")) {
          if (url.startsWith(pattern.slice(0, -1))) return handler(request);
        } else if (url === pattern) {
          return handler(request);
        }
      }
      return jsonResponse({ error: { code: "not_found" } }, 404);
    },
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

/** The routes every case needs: a session, and the families this page reads. */
function investigationRoutes(
  answer: () => Response | Promise<Response>,
  overrides: Record<
    string,
    (request: Request) => Response | Promise<Response>
  > = {},
) {
  return {
    "/api/public/v1/session": () => jsonResponse(OWNER_SESSION, 200),
    ["/api/admin/v1/nodes/" + NODE_ID + "/investigation*"]: answer,
    ["/api/admin/v1/nodes/" + NODE_ID + "/metric-history*"]: () =>
      jsonResponse(metricHistoryFixture(), 200),
    ["/api/admin/v1/nodes/" + NODE_ID + "/state-history*"]: () =>
      jsonResponse(
        { error: { code: "unavailable", message: "no state history" } },
        503,
      ),
    ["/api/admin/v1/nodes/" + NODE_ID + "/peer-history*"]: () =>
      jsonResponse(peerHistoryFixture(), 200),
    ...overrides,
  };
}

function calledUrls(): string[] {
  return REQUESTS.map((url) => url.replace(TEST_ORIGIN, ""));
}

function callsTo(fragment: string): string[] {
  return calledUrls().filter((url) => url.indexOf(fragment) >= 0);
}

function queryOf(url: string | undefined): URLSearchParams {
  return new URL(url ?? "", TEST_ORIGIN).searchParams;
}

function presetButton(label: string): HTMLElement {
  return within(
    screen.getByRole("group", { name: "Investigation window" }),
  ).getByRole("button", {
    name: label,
  });
}

function windowControl(): HTMLElement {
  const slot = document.querySelector('[data-slot="investigation-window"]');
  if (!slot) throw new Error("the window control is not rendered");
  return slot as HTMLElement;
}

/** The panel under one heading, so an assertion never reads a neighbouring card. */
function panel(title: string): HTMLElement {
  const heading = screen.getByRole("heading", { level: 2, name: title });
  const section = heading.closest("section");
  if (!section) throw new Error("the panel " + title + " is not rendered");
  return section as HTMLElement;
}

async function renderAt(path: string) {
  render(<App />);
  await act(async () => {
    window.history.pushState({}, "", path);
    window.dispatchEvent(new PopStateEvent("popstate"));
    await Promise.resolve();
  });
}

async function waitForInvestigation() {
  await screen.findByRole("heading", { level: 2, name: "Window" });
}

beforeEach(() => {
  REQUESTS.length = 0;
  window.history.replaceState({}, "", "/");
  client.setConfig({ baseUrl: TEST_ORIGIN });
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  adminQueryClient.clear();
});

describe("PAGE-ADMIN-NODE-INVESTIGATION (one UTC window over every family)", () => {
  it("reads the Server default window and answers every family in one coordinate", async () => {
    mockFetch(investigationRoutes(() => jsonResponse(answerFixture(), 200)));
    await renderAt("/admin/nodes/" + NODE_ID + "/investigation");
    await waitForInvestigation();

    // The link names no window at all, so the window is the Server's own default.
    const investigation = callsTo("/investigation");
    expect(investigation.length).toBe(1);
    expect(queryOf(investigation[0]).toString()).toBe("");

    // One card per family, whether or not that family holds evidence.
    expect(
      document.querySelectorAll('[data-slot="investigation-source"]').length,
    ).toBe(3);
    // The presets are the Server's own list, and the answered preset is the one marked.
    expect(presetButton("24 hours").getAttribute("aria-pressed")).toBe("true");
    expect(presetButton("1 hour").getAttribute("aria-pressed")).toBe("false");
    // The answered window is named on the page as one range, in UTC and without a local clock.
    const answeredRange = screen.getByText("Answered range")
      .parentElement as HTMLElement;
    expect(answeredRange.textContent).toContain(
      "2026-08-11 12:00:00 UTC to 2026-08-12 12:00:00 UTC",
    );
    expect(
      screen.getByText(
        "1 day; older stretches are answered by aggregate buckets, not by raw points",
      ),
    ).toBeTruthy();

    // Every panel below reads the range the Server answered rather than a clock of its own.
    await waitFor(() =>
      expect(callsTo("/metric-history").length).toBeGreaterThan(0),
    );
    const metric = queryOf(callsTo("/metric-history")[0]);
    expect(metric.get("from")).toBe(WINDOW_FROM);
    expect(metric.get("to")).toBe(WINDOW_TO);
    const state = queryOf(callsTo("/state-history")[0]);
    expect(state.get("from")).toBe(WINDOW_FROM);
    expect(state.get("to")).toBe(WINDOW_TO);
    const peer = queryOf(callsTo("/peer-history")[0]);
    expect(peer.get("grain")).toBe("5m");
    expect(peer.get("from")).toBe(WINDOW_FROM);
    expect(peer.get("to")).toBe(WINDOW_TO);

    // The panels render the answers they were handed rather than a window of their own.
    expect(
      await within(panel("Node metrics in this window")).findAllByText(
        "Observed",
      ),
    ).not.toHaveLength(0);
    expect(
      screen.getByRole("heading", {
        level: 2,
        name: "Recorded Node state in this window",
      }),
    ).toBeTruthy();
    expect(
      screen.getByRole("heading", { level: 2, name: "Peer receipt evidence" }),
    ).toBeTruthy();

    // The windowed Incident reader is entered with the window that was just read, so the list applies
    // the same occurrence predicate instead of opening on a clock of its own.
    expect(
      screen
        .getByRole("link", { name: "Incidents for this Node in this window" })
        .getAttribute("href"),
    ).toBe(
      "/admin/alerts/incidents?subject=node&subject_key=" +
        NODE_ID +
        "&from=" +
        encodeURIComponent(WINDOW_FROM) +
        "&to=" +
        encodeURIComponent(WINDOW_TO),
    );
  });

  it("reads the preset a link names and moves the link when another preset is chosen", async () => {
    mockFetch(
      investigationRoutes(() =>
        jsonResponse(
          answerFixture({
            window: windowFixture({
              durationSeconds: 604800,
              from: "2026-08-05T12:00:00Z",
              preset: "7d",
              presetLabel: "7 days",
              requestedFrom: "2026-08-05T12:00:00Z",
            }),
          }),
          200,
        ),
      ),
    );
    await renderAt("/admin/nodes/" + NODE_ID + "/investigation?window=7d");
    await waitForInvestigation();

    expect(queryOf(callsTo("/investigation")[0]).get("window")).toBe("7d");
    expect(presetButton("7 days").getAttribute("aria-pressed")).toBe("true");

    // A preset is a coordinate of the link, not component state: choosing one rewrites the address.
    fireEvent.click(presetButton("1 hour"));
    await waitFor(() =>
      expect(
        callsTo("/investigation").some(
          (url) => queryOf(url).get("window") === "1h",
        ),
      ).toBe(true),
    );
    expect(new URLSearchParams(window.location.search).get("window")).toBe(
      "1h",
    );
  });

  it("reads an explicit range from the link and names it as one", async () => {
    const from = "2026-08-10T00:00:00Z";
    const to = "2026-08-11T00:00:00Z";
    mockFetch(
      investigationRoutes(() =>
        jsonResponse(
          answerFixture({
            window: windowFixture({
              custom: true,
              from: from,
              preset: "custom",
              presetLabel: "Custom range",
              requestedFrom: from,
              requestedTo: to,
              to: to,
            }),
          }),
          200,
        ),
      ),
    );
    await renderAt(
      "/admin/nodes/" + NODE_ID + "/investigation?from=" + from + "&to=" + to,
    );
    await waitForInvestigation();

    const call = queryOf(callsTo("/investigation")[0]);
    expect(call.get("window")).toBeNull();
    expect(call.get("from")).toBe(from);
    expect(call.get("to")).toBe(to);
    expect(screen.getByText("An explicit range in this link")).toBeTruthy();
  });

  it("sends the window a link names and shows the Server's own refusal in place of evidence", async () => {
    const reason =
      "choose either a preset window or an explicit from/to range, not both";
    mockFetch(
      investigationRoutes(() =>
        jsonResponse(
          { error: { code: "invalid_investigation_window", message: reason } },
          400,
        ),
      ),
    );
    const asked =
      "?window=7d&from=2026-08-10T00:00:00Z&to=2026-08-11T00:00:00Z";
    await renderAt("/admin/nodes/" + NODE_ID + "/investigation" + asked);

    // The link's window is sent exactly as the link states it: which windows the Server reads is the
    // Server's rule, so the refusal the reader sees belongs to the address they followed.
    const call = await waitFor(() => {
      expect(callsTo("/investigation").length).toBeGreaterThan(0);
      return queryOf(callsTo("/investigation")[0]);
    });
    expect(call.get("window")).toBe("7d");
    expect(call.get("from")).toBe("2026-08-10T00:00:00Z");
    expect(call.get("to")).toBe("2026-08-11T00:00:00Z");

    const refused = await waitFor(() => {
      const slot = document.querySelector(
        '[data-slot="investigation-window-refused"]',
      );
      expect(slot).not.toBeNull();
      return slot as HTMLElement;
    });
    expect(refused.textContent).toContain(reason);
    // Nothing was read for the refused window, and another window can still be asked for here.
    expect(
      document.querySelector('[data-slot="investigation-source"]'),
    ).toBeNull();
    expect(windowControl()).toBeTruthy();
    expect(window.location.search).toBe(asked);
  });

  it("sends a range narrower than the Server reads, for the Server to name its width", async () => {
    const reason =
      "the window is 1800 seconds wide; the supported minimum is 1 hour";
    mockFetch(
      investigationRoutes(() =>
        jsonResponse(
          { error: { code: "invalid_investigation_window", message: reason } },
          400,
        ),
      ),
    );
    const from = "2026-08-12T11:00:00Z";
    const to = "2026-08-12T11:30:00Z";
    await renderAt(
      "/admin/nodes/" + NODE_ID + "/investigation?from=" + from + "&to=" + to,
    );

    const call = await waitFor(() => {
      expect(callsTo("/investigation").length).toBeGreaterThan(0);
      return queryOf(callsTo("/investigation")[0]);
    });
    expect(call.get("from")).toBe(from);
    expect(call.get("to")).toBe(to);
    const refused = await waitFor(() => {
      const slot = document.querySelector(
        '[data-slot="investigation-window-refused"]',
      );
      expect(slot).not.toBeNull();
      return slot as HTMLElement;
    });
    expect(refused.textContent).toContain("1800 seconds wide");
  });

  it("sends the ends that were typed as the range to read", async () => {
    mockFetch(investigationRoutes(() => jsonResponse(answerFixture(), 200)));
    await renderAt("/admin/nodes/" + NODE_ID + "/investigation");
    await waitForInvestigation();

    // A `datetime-local` value names an instant in this browser's zone, so the ends are stated in
    // UTC here the same way the page states them: one window, read the same for every reader.
    const typedFrom = new Date("2026-08-12T11:00")
      .toISOString()
      .replace(".000Z", "Z");
    const typedTo = new Date("2026-08-12T11:30")
      .toISOString()
      .replace(".000Z", "Z");
    const before = callsTo("/investigation").length;
    fireEvent.change(within(windowControl()).getByLabelText("From"), {
      target: { value: "2026-08-12T11:00" },
    });
    fireEvent.change(within(windowControl()).getByLabelText("To"), {
      target: { value: "2026-08-12T11:30" },
    });
    fireEvent.click(
      within(windowControl()).getByRole("button", { name: "Read this range" }),
    );
    // A range this page cannot read is not a range this page refuses on the Server's behalf: the two
    // ends are sent and the Server answers with its own window or its own refusal.
    await waitFor(() =>
      expect(callsTo("/investigation").length).toBe(before + 1),
    );
    expect(queryOf(callsTo("/investigation")[before]).get("from")).toBe(
      typedFrom,
    );

    const calls = callsTo("/investigation");
    const read = queryOf(calls[calls.length - 1]);
    expect(read.get("from")).toBe(typedFrom);
    expect(read.get("to")).toBe(typedTo);
  });

  it("locates the occurrence of the Incident that brought the Operator here", async () => {
    // A family that started inside the window: it answers the stretch after its own
    // start and names the stretch before it.
    const bridging = sourceFixture({
      boundaries: [
        {
          at: "2026-08-11T00:00:00Z",
          detail: "This Node held no state delivery before this instant.",
          kind: "pre_enablement",
          kindLabel: "Pre-enablement",
          to: null,
        },
      ],
      coverage: "partial",
      coverageLabel: "Partial",
      key: "node_state",
      label: "Sync and consensus entries",
      timeBasis: "state_observation",
      timeBasisLabel: "State observation time",
    });
    // A family that does not apply to this Node at all: it holds no boundary and no
    // hole, so it must not be read as a family that answers the instant.
    const inapplicable = sourceFixture({
      answerPaths: [],
      coverage: "unsupported",
      coverageLabel: "Not applicable",
      grains: [],
      key: "validator",
      label: "Validator source",
      notes: [
        "No Validator link covers this window, so no Validator snapshot answers it.",
      ],
      timeBasis: "validator_snapshot_source",
      timeBasisLabel: "Validator source",
    });
    mockFetch(
      investigationRoutes(() =>
        jsonResponse(
          answerFixture({ sources: [sourceFixture(), bridging, inapplicable] }),
          200,
        ),
      ),
    );
    await renderAt(
      "/admin/nodes/" +
        NODE_ID +
        "/investigation?occurrence=" +
        OCCURRENCE +
        "&incident=inc-1",
    );
    await waitForInvestigation();

    const located = screen.getByText(
      /The Incident opened at 2026-08-11 18:00:00 UTC/,
    );
    expect(located.getAttribute("data-slot")).toBe("investigation-occurrence");
    const table = document.querySelector(
      '[data-slot="investigation-occurrence-table"]',
    ) as HTMLElement;
    expect(within(table).getAllByRole("row").length).toBe(4);
    // A family with no boundary and no silence covering the instant says so: the
    // page never claims the instant is answered from evidence it does not cite.
    expect(
      within(table).getByText(
        "No boundary and no silence of this family covers the instant.",
      ),
    ).toBeTruthy();
    expect(within(table).getByText("Pre-enablement")).toBeTruthy();
    // A family that does not apply to this Node answers nothing about the instant.
    expect(
      within(table).getByText("This family does not apply to this Node."),
    ).toBeTruthy();
    // The instant alone is not the answer: the Incident it belongs to stays reachable.
    const card = located.closest('[data-slot="card-x"]') as HTMLElement;
    expect(
      within(card)
        .getByRole("link", { name: "Back to the Incident" })
        .getAttribute("href"),
    ).toBe("/admin/alerts/incidents/inc-1");
  });

  it("pages Peer receipts from the answer own continuation rather than this browser clock", async () => {
    mockFetch(
      investigationRoutes(() => jsonResponse(answerFixture(), 200), {
        ["/api/admin/v1/nodes/" + NODE_ID + "/peer-history*"]: (request) => {
          const before = new URL(request.url).searchParams.get("before");
          if (before === null) return jsonResponse(peerHistoryFixture(), 200);
          return jsonResponse(
            peerHistoryFixture({
              range: peerRangeFixture({
                buckets: [peerBucket("2026-08-12T11:45:00Z")],
                continuation: null,
                first_bucket: "2026-08-12T11:45:00Z",
                last_bucket: "2026-08-12T11:45:00Z",
                returned: 1,
                truncated: false,
              }),
            }),
            200,
          );
        },
      }),
    );
    await renderAt("/admin/nodes/" + NODE_ID + "/investigation");
    await waitForInvestigation();

    const range = await screen.findByText(
      /2 of 300 buckets this window covers are in this answer/,
    );
    expect(range.getAttribute("data-slot")).toBe("investigation-peer-range");
    expect(
      document.querySelector('[data-slot="investigation-peer-truncated"]'),
    ).not.toBeNull();
    const table = document.querySelector(
      '[data-slot="investigation-peer-buckets"]',
    ) as HTMLElement;
    expect(within(table).getAllByRole("row").length).toBe(3);
    // A bucket start IS the receipt instant, and the CFBT lag column is measured in blocks.
    expect(within(table).getByText("CFBT lag (blocks)")).toBeTruthy();
    expect(within(table).getAllByText("2026-08-12 11:55:00 UTC").length).toBe(
      1,
    );

    fireEvent.click(screen.getByRole("button", { name: "Load older buckets" }));
    await waitFor(() =>
      expect(
        callsTo("/peer-history").some(
          (url) => queryOf(url).get("before") === "2026-08-12T11:50:00Z",
        ),
      ).toBe(true),
    );
    // The older page is named by the answer, and the newest page stays one click away.
    fireEvent.click(
      screen.getByRole("button", { name: "Back to the newest buckets" }),
    );
    // Returning to the newest page is not a new window: the same receipts are read again.
    await screen.findByText(
      /2 of 300 buckets this window covers are in this answer/,
    );
    const newest = document.querySelector(
      '[data-slot="investigation-peer-buckets"]',
    ) as HTMLElement;
    await waitFor(() =>
      expect(within(newest).getAllByRole("row").length).toBe(3),
    );
    expect(within(newest).getAllByText("2026-08-12 11:55:00 UTC").length).toBe(
      1,
    );
  });

  it("keeps the last successful Peer range and says so when the refresh fails", async () => {
    let failing = false;
    mockFetch(
      investigationRoutes(() => jsonResponse(answerFixture(), 200), {
        ["/api/admin/v1/nodes/" + NODE_ID + "/peer-history*"]: () =>
          failing
            ? jsonResponse(
                { error: { code: "unavailable", message: "the Server is busy" } },
                503,
              )
            : jsonResponse(peerHistoryFixture(), 200),
      }),
    );
    await renderAt("/admin/nodes/" + NODE_ID + "/investigation");
    await waitForInvestigation();
    await screen.findByText(
      /2 of 300 buckets this window covers are in this answer/,
    );

    // A refresh of the same window fails. The receipts already read are the last
    // successful read of this window, and the panel says exactly that instead of
    // showing a failure over a range that still reads as this read's answer.
    failing = true;
    await act(async () => {
      await adminQueryClient.refetchQueries({
        predicate: (query) => query.queryKey.includes("peer-range"),
      });
    });

    expect(
      await screen.findByText(
        /Failed to refresh; the range below is the last successful read of this window./,
      ),
    ).toBeTruthy();
    expect(screen.queryByText(/Unable to load the Peer receipt range/)).toBeNull();
    expect(
      document.querySelector('[data-slot="investigation-peer-range"]')
        ?.textContent,
    ).toContain("2 of 300 buckets this window covers are in this answer");
  });

  it("names a Node the Server no longer holds as missing rather than as an empty window", async () => {
    mockFetch(
      investigationRoutes(() =>
        jsonResponse(
          { error: { code: "not_found", message: "no such node" } },
          404,
        ),
      ),
    );
    await renderAt("/admin/nodes/" + NODE_ID + "/investigation");

    await screen.findByText("This Node is no longer available.");
    // A Node without evidence is never presented as a window of zeros, and there is nothing to retry.
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
    expect(
      document.querySelector('[data-slot="investigation-source"]'),
    ).toBeNull();
  });

  it("tells a purged Node apart from one that is merely missing", async () => {
    mockFetch(
      investigationRoutes(() =>
        jsonResponse(
          {
            error: {
              code: "node_purged",
              message: "purged at 2026-08-01T00:00:00Z",
            },
          },
          404,
        ),
      ),
    );
    await renderAt("/admin/nodes/" + NODE_ID + "/investigation");

    await screen.findByText(
      "This Node was purged, so its evidence was deleted by design and cannot be reconstructed.",
    );
  });

  it("moves the page window when an evidence panel asks for another width", async () => {
    mockFetch(
      investigationRoutes(() =>
        jsonResponse(
          answerFixture({
            window: windowFixture({
              supportedPresets: [
                { key: "1h", label: "1 hour", hours: 1 },
                { key: "24h", label: "24 hours", hours: 24 },
              ],
            }),
          }),
          200,
        ),
      ),
    );
    await renderAt("/admin/nodes/" + NODE_ID + "/investigation");
    await waitForInvestigation();
    await waitFor(() =>
      expect(callsTo("/metric-history").length).toBeGreaterThan(0),
    );

    // The panel's own presets are this page's window: a width the Server named no preset for is read
    // as a range sharing the answered end, instead of jumping to this browser's now.
    const metricPanel = panel("Node metrics in this window");
    fireEvent.click(
      within(metricPanel).getByRole("button", { name: "6 hours" }),
    );
    await waitFor(() =>
      expect(
        callsTo("/investigation").some(
          (url) =>
            queryOf(url).get("from") === "2026-08-12T06:00:00Z" &&
            queryOf(url).get("to") === WINDOW_TO,
        ),
      ).toBe(true),
    );
    const search = new URLSearchParams(window.location.search);
    expect(search.get("from")).toBe("2026-08-12T06:00:00Z");
    expect(search.get("to")).toBe(WINDOW_TO);

    // Reloading the window re-reads the same stretch: it does not derive a new one from the clock.
    const calls = callsTo("/investigation").length;
    fireEvent.click(
      within(panel("Node metrics in this window")).getByRole("button", {
        name: "Reload window",
      }),
    );
    await waitFor(() =>
      expect(callsTo("/investigation").length).toBeGreaterThan(calls),
    );
    expect(new URLSearchParams(window.location.search).get("from")).toBe(
      "2026-08-12T06:00:00Z",
    );
  });

  it("re-reads the window the link asked for instead of sliding it to this browser clock", async () => {
    mockFetch(investigationRoutes(() => jsonResponse(answerFixture(), 200)));
    await renderAt("/admin/nodes/" + NODE_ID + "/investigation?window=1h");
    await waitForInvestigation();

    const calls = callsTo("/investigation").length;
    // The page owns the reload, so it sits with the window control rather than inside a panel.
    fireEvent.click(
      within(windowControl()).getByRole("button", { name: "Reload window" }),
    );
    await waitFor(() =>
      expect(callsTo("/investigation").length).toBeGreaterThan(calls),
    );
    // The link still names the window: a reload re-reads that stretch, it does not derive a new one.
    expect(queryOf(callsTo("/investigation").at(-1)).get("window")).toBe("1h");
    expect(new URLSearchParams(window.location.search).get("window")).toBe(
      "1h",
    );
  });

  it("says a window opened from an Incident starts at that occurrence rather than being chosen here", async () => {
    mockFetch(
      investigationRoutes(() =>
        jsonResponse(
          answerFixture({
            window: windowFixture({
              custom: true,
              preset: "custom",
              requestedFrom: OCCURRENCE,
              from: OCCURRENCE,
              requestedTo: WINDOW_TO,
              to: WINDOW_TO,
            }),
          }),
          200,
        ),
      ),
    );
    await renderAt(
      "/admin/nodes/" +
        NODE_ID +
        "/investigation?incident=inc-1&occurrence=" +
        OCCURRENCE +
        "&from=" +
        OCCURRENCE +
        "&to=" +
        WINDOW_TO,
    );
    await waitForInvestigation();

    const entry = await screen.findByText(/This window starts at that instant/);
    expect(entry.getAttribute("data-slot")).toBe(
      "investigation-occurrence-entry",
    );
    // The width is the one the link asked for, so the card never claims a width nobody chose.
    expect(entry.textContent).toContain(
      "the width is the width the link asked for",
    );
  });

  it("attributes a family to the subjects the Server recorded and to no other key", async () => {
    const FORMER_AGENT = "0195f2a1-0021-4021-8021-000000000021";
    const related = sourceFixture({
      key: "host_metrics",
      label: "Host metrics",
      relatedSubjects: [
        {
          basis: "recorded_relationship",
          basisLabel: "Recorded relationship",
          detail: "the Server recorded this Agent for this stretch",
          from: WINDOW_FROM,
          role: "source",
          subject: AGENT_ID,
          subjectKind: "agent",
          to: WINDOW_TO,
        },
        {
          basis: "recorded_relationship",
          basisLabel: "Recorded relationship",
          detail: "the Server recorded another Agent for this stretch",
          from: "2026-08-12T10:00:00Z",
          role: "related",
          subject: FORMER_AGENT,
          subjectKind: "agent",
          to: "2026-08-12T11:00:00Z",
        },
      ],
    });
    mockFetch(
      investigationRoutes(() =>
        jsonResponse(
          answerFixture({ sources: [related, peersSourceFixture()] }),
          200,
        ),
      ),
    );
    await renderAt("/admin/nodes/" + NODE_ID + "/investigation");
    await waitForInvestigation();

    const table = document.querySelector(
      '[data-slot="investigation-related-subjects"]',
    );
    if (!table) throw new Error("the related subjects table is not rendered");
    const rows = table.querySelectorAll("tbody tr");
    expect(rows).toHaveLength(2);
    // The family's own subject is named as such, and the earlier key is named as related rather than
    // being folded into the family's own points.
    expect(rows[0].textContent).toContain("Agent");
    expect(rows[0].textContent).toContain(AGENT_ID);
    expect(rows[0].textContent).toContain("This family's own subject");
    expect(rows[1].textContent).toContain(FORMER_AGENT);
    expect(rows[1].textContent).toContain("Related subject");
    expect(rows[1].textContent).toContain("Recorded relationship");
    expect(rows[1].textContent).toContain("recorded_relationship");
    expect(rows[1].textContent).toContain(
      "the Server recorded another Agent for this stretch",
    );

    // A family the Server credited to nobody says so, rather than reading its own points as the
    // subject's own.
    expect(
      document.querySelector(
        '[data-slot="investigation-no-related-subject"]',
      )?.textContent,
    ).toContain("records no subject related to this family");
  });

  it("reads a family the Server points no further than as a statement rather than a broken link", async () => {
    // The record family is answered inside this window: the Server names no endpoint for it, so the
    // card states that instead of heading a list of links that does not exist.
    const record = sourceFixture({
      answerPaths: [],
      key: "relationships",
      label: "Recorded relationships",
      timeBasis: "server_record",
      timeBasisLabel: "Server record time",
    });
    mockFetch(
      investigationRoutes(() =>
        jsonResponse(
          answerFixture({ sources: [record, peersSourceFixture()] }),
          200,
        ),
      ),
    );
    await renderAt("/admin/nodes/" + NODE_ID + "/investigation");
    await waitForInvestigation();

    const card = document.querySelector(
      '[data-slot="investigation-source"][data-source="relationships"]',
    ) as HTMLElement;
    expect(
      card.querySelector('[data-slot="investigation-answer-paths"]'),
    ).toBeNull();
    expect(
      card.querySelector('[data-slot="investigation-no-answer-path"]')
        ?.textContent,
    ).toContain("The Server sends no endpoint for this family");

    // A family that does send them still lists them, and states nothing about endpoints.
    const peersCard = document.querySelector(
      '[data-slot="investigation-source"][data-source="peers"]',
    ) as HTMLElement;
    expect(
      peersCard.querySelector('[data-slot="investigation-answer-paths"]'),
    ).not.toBeNull();
    expect(
      peersCard.querySelector('[data-slot="investigation-no-answer-path"]'),
    ).toBeNull();
  });
});
