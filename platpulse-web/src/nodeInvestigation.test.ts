import { describe, expect, it } from "vitest";
import type {
  InvestigationBoundaryResponse,
  InvestigationGrainResponse,
  InvestigationSourceResponse,
} from "./api/generated";
import {
  boundaryAt,
  canonicalInstant,
  coverageAnswersTheWholeWindow,
  coverageNotice,
  coverageTone,
  grainEvidenceLabel,
  grainLabel,
  grainMissingLabel,
  holeAt,
  instantWithin,
  investigationKey,
  investigationQuery,
  investigationSearch,
  localInputValue,
  peerGrainOptions,
  SERVER_DEFAULT_WINDOW,
  readInvestigationLink,
  receiptDelaySeconds,
  relatedSubjectKindLabel,
  sourceAtInstant,
  timeBasisNote,
} from "./nodeInvestigation";

const OPENS = "2026-08-12T10:00:00Z";
const CLOSES = "2026-08-12T11:00:00Z";

function boundary(
  overrides: Partial<InvestigationBoundaryResponse> = {},
): InvestigationBoundaryResponse {
  return {
    at: "2026-08-12T10:00:00Z",
    detail: "the Agent was not collecting this family yet",
    kind: "pre_enablement",
    kindLabel: "Pre-enablement",
    to: "2026-08-12T10:20:00Z",
    ...overrides,
  };
}

function grain(
  overrides: Partial<InvestigationGrainResponse> = {},
): InvestigationGrainResponse {
  return {
    available: true,
    grain: "raw",
    holes: [],
    pointCount: 12,
    sampleCount: 12,
    ...overrides,
  };
}

function source(
  overrides: Partial<InvestigationSourceResponse> = {},
): InvestigationSourceResponse {
  return {
    answerPaths: [],
    boundaries: [],
    components: [],
    coverage: "complete",
    coverageLabel: "Complete",
    grains: [],
    key: "node_metrics",
    label: "Node metrics",
    notes: [],
    relatedSubjects: [],
    subject: "0195f2a1-0014-4014-8014-000000000014",
    subjectKind: "node",
    timeBasis: "metric_observation",
    timeBasisLabel: "Metric observation",
    truncated: false,
    ...overrides,
  };
}

describe("canonicalInstant", () => {
  it("reads an offset instant as the same UTC instant with second precision", () => {
    expect(canonicalInstant("2026-08-12T18:00:00+08:00")).toBe(
      "2026-08-12T10:00:00Z",
    );
  });

  it("drops sub-second precision rather than sending what the Server refuses", () => {
    expect(canonicalInstant("2026-08-12T10:00:00.750Z")).toBe(
      "2026-08-12T10:00:00Z",
    );
  });

  it('refuses a value that names no instant, so nothing is read as "now"', () => {
    expect(canonicalInstant("")).toBeNull();
    expect(canonicalInstant("   ")).toBeNull();
    expect(canonicalInstant("yesterday evening")).toBeNull();
    expect(canonicalInstant(null)).toBeNull();
    expect(canonicalInstant(undefined)).toBeNull();
  });
});

describe("localInputValue", () => {
  it("shows the instant a datetime-local control can edit", () => {
    const shown = localInputValue("2026-08-12T10:05:00Z");
    expect(shown).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/);
    expect(canonicalInstant(shown + ":00Z")).not.toBeNull();
  });

  it("shows nothing for a value that is not an instant", () => {
    expect(localInputValue("not an instant")).toBe("");
    expect(localInputValue(null)).toBe("");
  });
});

describe("readInvestigationLink", () => {
  it("asks for the Server default when the link names no window", () => {
    const read = readInvestigationLink(new URLSearchParams());
    expect(read.request).toEqual({});
    expect(read.asked).toBe("");
    expect(read.occurrence).toBeNull();
    expect(read.incident).toBeNull();
  });

  it("reads one preset as the window it names", () => {
    const read = readInvestigationLink(new URLSearchParams("window=7d"));
    expect(read.request).toEqual({ window: "7d" });
  });

  it("reads an explicit range as one window, canonicalized to UTC seconds", () => {
    const read = readInvestigationLink(
      new URLSearchParams({
        from: "2026-08-12T18:00:00+08:00",
        to: "2026-08-12T19:30:00+08:00",
      }),
    );
    expect(read.request).toEqual({
      from: "2026-08-12T10:00:00Z",
      to: "2026-08-12T11:30:00Z",
    });
  });

  it("keeps the occurrence and the Incident the link came from", () => {
    const read = readInvestigationLink(
      new URLSearchParams(
        "from=2026-08-12T09:00:00Z&to=2026-08-12T11:00:00Z&occurrence=2026-08-12T10:00:00.500Z&incident=inc-0001",
      ),
    );
    expect(read.occurrence).toBe("2026-08-12T10:00:00Z");
    expect(read.incident).toBe("inc-0001");
  });

  it("sends a link that names two windows at once rather than choosing one for the Server", () => {
    const read = readInvestigationLink(
      new URLSearchParams(
        "window=24h&from=2026-08-12T09:00:00Z&to=2026-08-12T11:00:00Z",
      ),
    );
    expect(read.request).toEqual({
      window: "24h",
      from: "2026-08-12T09:00:00Z",
      to: "2026-08-12T11:00:00Z",
    });
    expect(read.asked).toContain("window=24h");
  });

  it("sends half a range as the half the link states instead of completing it from the clock", () => {
    expect(
      readInvestigationLink(new URLSearchParams("from=2026-08-12T09:00:00Z"))
        .request,
    ).toEqual({ from: "2026-08-12T09:00:00Z" });
    expect(
      readInvestigationLink(new URLSearchParams("to=2026-08-12T11:00:00Z"))
        .request,
    ).toEqual({ to: "2026-08-12T11:00:00Z" });
  });

  it("passes an instant nobody can read through exactly as the link wrote it", () => {
    const read = readInvestigationLink(
      new URLSearchParams("from=soon&to=not-a-time"),
    );
    expect(read.request).toEqual({ from: "soon", to: "not-a-time" });
  });

  it("sends a range the Server refuses for the Server to name in its own words", () => {
    expect(
      readInvestigationLink(
        new URLSearchParams(
          "from=2026-08-12T10:00:00Z&to=2026-08-12T10:30:00Z",
        ),
      ).request,
    ).toEqual({
      from: "2026-08-12T10:00:00Z",
      to: "2026-08-12T10:30:00Z",
    });
    expect(
      readInvestigationLink(
        new URLSearchParams(
          "from=2026-08-12T10:00:00Z&to=2026-08-12T09:00:00Z",
        ),
      ).request,
    ).toEqual({
      from: "2026-08-12T10:00:00Z",
      to: "2026-08-12T09:00:00Z",
    });
  });
});

describe("investigationQuery", () => {
  it("sends one preset, one range, or nothing at all", () => {
    expect(investigationQuery({ window: "1h" })).toEqual({ window: "1h" });
    expect(investigationQuery({ from: OPENS, to: CLOSES })).toEqual({
      from: OPENS,
      to: CLOSES,
    });
    expect(investigationQuery(SERVER_DEFAULT_WINDOW)).toEqual({});
  });

  it("sends every window a link names, including the ones the Server will refuse", () => {
    expect(investigationQuery({ window: "24h", from: OPENS })).toEqual({
      window: "24h",
      from: OPENS,
    });
    expect(investigationQuery({ to: CLOSES })).toEqual({ to: CLOSES });
  });
});

describe("investigationKey", () => {
  it("separates two windows so no answer is read as another window", () => {
    expect(investigationKey({ window: "1h" })).not.toBe(
      investigationKey({ window: "24h" }),
    );
    expect(investigationKey({ from: OPENS, to: CLOSES })).not.toBe(
      investigationKey({ from: OPENS, to: "2026-08-12T12:00:00Z" }),
    );
    expect(investigationKey(SERVER_DEFAULT_WINDOW)).not.toBe(
      investigationKey({ window: "24h" }),
    );
    expect(investigationKey({ window: "24h", from: OPENS })).not.toBe(
      investigationKey({ window: "24h" }),
    );
  });

  it("names the same window the same way every time", () => {
    expect(investigationKey({ window: "7d" })).toBe(
      investigationKey({ window: "7d" }),
    );
    expect(investigationKey({ from: OPENS, to: CLOSES })).toBe(
      investigationKey({ from: OPENS, to: CLOSES }),
    );
  });
});

describe("investigationSearch", () => {
  it("writes one window and keeps the located occurrence attached to it", () => {
    expect(
      investigationSearch(
        { window: "6h" },
        { occurrence: "2026-08-12T10:00:00.500Z", incident: "inc-0001" },
      ),
    ).toBe("?window=6h&occurrence=2026-08-12T10%3A00%3A00Z&incident=inc-0001");
  });

  it("writes the range form when the range is the window", () => {
    const search = investigationSearch({ from: OPENS, to: CLOSES });
    expect(new URLSearchParams(search).get("from")).toBe(OPENS);
    expect(new URLSearchParams(search).get("to")).toBe(CLOSES);
    expect(new URLSearchParams(search).get("window")).toBeNull();
  });

  it("round-trips what readInvestigationLink reads", () => {
    const search = investigationSearch(
      { from: OPENS, to: CLOSES },
      { occurrence: OPENS },
    );
    const read = readInvestigationLink(new URLSearchParams(search.slice(1)));
    expect(read.request).toEqual({ from: OPENS, to: CLOSES });
    expect(read.occurrence).toBe(OPENS);
  });

  it("carries only the located occurrence when the Server default is asked for", () => {
    expect(
      investigationSearch(SERVER_DEFAULT_WINDOW, { occurrence: OPENS }),
    ).toBe("?occurrence=2026-08-12T10%3A00%3A00Z");
    expect(investigationSearch(SERVER_DEFAULT_WINDOW)).toBe("");
  });
});

describe("coverage verdicts", () => {
  it("paints a complete window as answered and a partial one as worth reading further", () => {
    expect(coverageTone("complete")).toBe("ok");
    expect(coverageTone("partial")).toBe("warning");
    expect(coverageTone("empty")).toBe("neutral");
    expect(coverageTone("unavailable")).toBe("neutral");
    expect(coverageTone("unsupported")).toBe("neutral");
  });

  it("treats only a complete verdict as answering the whole window", () => {
    expect(coverageAnswersTheWholeWindow("complete")).toBe(true);
    for (const covered of ["partial", "empty", "unavailable", "unsupported"]) {
      expect(coverageAnswersTheWholeWindow(covered)).toBe(false);
    }
  });

  it("never reads an absent window as zero", () => {
    expect(coverageNotice("empty")).toContain("never zero");
    expect(coverageNotice("unavailable")).not.toContain("zero");
    expect(coverageNotice("partial")).toContain("boundaries");
  });

  it("passes a verdict the Server adds later through unchanged", () => {
    expect(coverageNotice("throttled")).toBe("throttled");
  });
});

describe("time bases and Peer grains", () => {
  it("explains each basis the Server names", () => {
    expect(timeBasisNote("peer_receipt_bucket")).toContain("received");
    expect(timeBasisNote("metric_observation")).toContain("observed");
    expect(timeBasisNote("incident_evaluation")).toContain("rule evaluation");
    expect(timeBasisNote("validator_snapshot_source")).toContain("Validator");
    expect(timeBasisNote("state_observation")).toContain("state");
  });

  it("leaves a basis it does not know without a sentence", () => {
    expect(timeBasisNote("agent_wall_clock")).toBeNull();
  });

  it("names each related subject kind the Server records", () => {
    expect(relatedSubjectKindLabel("agent")).toBe("Agent");
    expect(relatedSubjectKindLabel("host")).toBe("Host");
    expect(relatedSubjectKindLabel("network")).toBe("Network");
    expect(relatedSubjectKindLabel("validator")).toBe("Validator");
  });

  it("shows a kind it does not know as the Server wrote it", () => {
    expect(relatedSubjectKindLabel("peer")).toBe("peer");
  });

  it("reads the Peer grains out of the source itself", () => {
    const peers = source({
      key: "peers",
      grains: [
        grain({ grain: "receipt_bucket_5m", grainSeconds: 300 }),
        grain({ grain: "receipt_bucket_1h", grainSeconds: 3600 }),
      ],
    });
    expect(peerGrainOptions(peers)).toEqual([
      { value: "5m", label: "5m (300s)" },
      { value: "1h", label: "1h (3600s)" },
    ]);
  });

  it("offers no grain for a source that holds no receipt bucket", () => {
    expect(
      peerGrainOptions(source({ grains: [grain({ grain: "raw" })] })),
    ).toEqual([]);
    expect(peerGrainOptions(undefined)).toEqual([]);
  });
});

describe("instantWithin", () => {
  it("covers its start and not its end", () => {
    expect(instantWithin(OPENS, CLOSES, OPENS)).toBe(true);
    expect(instantWithin(OPENS, CLOSES, "2026-08-12T10:59:59Z")).toBe(true);
    expect(instantWithin(OPENS, CLOSES, CLOSES)).toBe(false);
    expect(instantWithin(OPENS, CLOSES, "2026-08-12T09:59:59Z")).toBe(false);
  });

  it("reads an open-ended stretch as still running", () => {
    expect(instantWithin(OPENS, null, "2030-01-01T00:00:00Z")).toBe(true);
    expect(instantWithin(OPENS, undefined, "2030-01-01T00:00:00Z")).toBe(true);
  });

  it("answers nothing for an instant nobody can read", () => {
    expect(instantWithin(OPENS, CLOSES, "sometime")).toBe(false);
  });
});

describe("sourceAtInstant", () => {
  it("locates an instant inside the boundary that refuses it", () => {
    const metrics = source({ boundaries: [boundary()], coverage: "partial" });
    const at = sourceAtInstant(metrics, "2026-08-12T10:10:00Z");
    expect(at.boundary?.kind).toBe("pre_enablement");
    expect(at.hole).toBeNull();
    expect(at.answered).toBe(false);
  });

  it("answers an instant inside a stretch the source does cover", () => {
    const metrics = source({ boundaries: [boundary()] });
    expect(sourceAtInstant(metrics, "2026-08-12T10:30:00Z").answered).toBe(
      true,
    );
  });

  it("reports a grain hole for an instant no boundary already refuses", () => {
    const peers = source({
      grains: [
        grain({
          grain: "receipt_bucket_5m",
          holes: [{ from: OPENS, to: CLOSES, seconds: 3600 }],
        }),
      ],
      key: "peers",
      coverage: "partial",
    });
    const covered = sourceAtInstant(peers, "2026-08-12T10:30:00Z");
    expect(covered.hole?.seconds).toBe(3600);
    expect(covered.answered).toBe(false);
    // The boundary is the wider statement, so a source that refuses the stretch
    // does not also report a hole inside it.
    const refused = sourceAtInstant(
      source({ boundaries: [boundary()], grains: peers.grains }),
      "2026-08-12T10:10:00Z",
    );
    expect(refused.boundary).not.toBeNull();
    expect(refused.hole).toBeNull();
  });

  it("reports no boundary and no hole for a source that holds neither", () => {
    expect(boundaryAt(undefined, OPENS)).toBeNull();
    expect(holeAt(undefined, OPENS)).toBeNull();
  });

  it("names a family that does not apply instead of reading it as evidence", () => {
    const validator = source({ key: "validator", coverage: "unsupported" });
    expect(sourceAtInstant(validator, "2026-08-12T10:30:00Z").applicable).toBe(
      false,
    );
    const metrics = source({ coverage: "partial" });
    expect(sourceAtInstant(metrics, "2026-08-12T10:30:00Z").applicable).toBe(
      true,
    );
  });
});

describe("evidence wording", () => {
  it("counts what a grain holds, and what it aggregates", () => {
    expect(grainEvidenceLabel(grain({ pointCount: 1, sampleCount: 1 }))).toBe(
      "1 stored point",
    );
    expect(
      grainEvidenceLabel(grain({ pointCount: 12, sampleCount: 144 })),
    ).toBe("12 stored points covering 144 observations");
    expect(
      grainEvidenceLabel(
        grain({
          available: false,
          note: "the 1m tier answers only older stretches",
        }),
      ),
    ).toBe("the 1m tier answers only older stretches");
  });

  it("states the missing points a grain proves, and stays silent when it cannot", () => {
    expect(
      grainMissingLabel(grain({ expectedPoints: 12, missingPoints: 0 })),
    ).toBe("No point of this grain is missing inside its stretch.");
    expect(
      grainMissingLabel(grain({ expectedPoints: 12, missingPoints: 3 })),
    ).toBe("3 of 12 expected points are missing inside its stretch.");
    expect(grainMissingLabel(grain())).toBeNull();
    expect(grainMissingLabel(grain({ expectedPoints: 12 }))).toBeNull();
  });

  it("names a grain with its width when it has one", () => {
    expect(grainLabel(grain())).toBe("raw");
    expect(grainLabel(grain({ grain: "5m", grainSeconds: 300 }))).toBe(
      "5m (300s)",
    );
  });

  it("measures the delay between an observation and its receipt, signed", () => {
    expect(
      receiptDelaySeconds("2026-08-12T10:00:00Z", "2026-08-12T10:00:03Z"),
    ).toBe(3);
    expect(
      receiptDelaySeconds("2026-08-12T10:00:03Z", "2026-08-12T10:00:00Z"),
    ).toBe(-3);
    expect(receiptDelaySeconds(null, "2026-08-12T10:00:00Z")).toBeNull();
    expect(receiptDelaySeconds("2026-08-12T10:00:00Z", null)).toBeNull();
  });
});
