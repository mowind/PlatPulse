/**
 * PAGE-ADMIN-NODE-INVESTIGATION (design §11.4, §11.5, §11.6, §11.7; webui.md §15.22): the pure half
 * of the one-window Node investigation (issue #220).
 *
 * One window answers every evidence family of one Node, and the window belongs to the URL: a shared
 * or reloaded link reads the same stretch instead of re-deriving it from the browser's clock. Three
 * rules this module keeps, because a coverage answer is only worth reading when it is honest.
 *
 * The window a link names is sent to the Server as the link states it, and the Server is the only
 * authority on which windows it reads: whether a range is readable, wide enough, or the right way
 * round is its rule, stated in its own words beside the window it refused, so this client repeats
 * neither its presets nor its width limits and cannot drift from them. A verdict is the
 * Server's word about a stretch, never health: `partial`, `empty`, `unavailable` and
 * `unsupported` are four different statements, and none of them is zero. And the instant an
 * Incident opened is located against the evidence — the boundary or the hole that covers it — rather
 * than assumed to have been observed.
 */

import type {
  InvestigationBoundaryResponse,
  InvestigationGrainResponse,
  InvestigationHoleResponse,
  InvestigationSourceResponse,
} from "./api/generated";

/**
 * The window a link asks for, exactly as the link states it: a preset, one end of a range, both, or
 * nothing at all.
 *
 * Every one of those shapes is handed to the Server unchanged, including the ones the Server will
 * refuse: a window this client decided not to send is a refusal the reader never gets to see, and
 * the reader of a link is the one who has to learn what its address asked for. Naming no window is a
 * request of its own — the Server answers its own default window (design §11.4, story 54) — so the
 * default, the preset list and the widest window are Server-owned facts arriving in the answer
 * rather than constants this client would repeat and could drift from.
 */
export type InvestigationRequest = {
  window?: string;
  from?: string;
  to?: string;
};

/** The request that names no window, and therefore asks for the Server's own default. */
export const SERVER_DEFAULT_WINDOW: InvestigationRequest = {};

export const INVESTIGATION_WINDOW_PARAM = "window";
export const INVESTIGATION_FROM_PARAM = "from";
export const INVESTIGATION_TO_PARAM = "to";
export const INVESTIGATION_OCCURRENCE_PARAM = "occurrence";
export const INVESTIGATION_INCIDENT_PARAM = "incident";

/** The error code the Server answers with when it will not read the window it was asked for. */
export const INVALID_WINDOW_CODE = "invalid_investigation_window";

/**
 * A UTC instant with second precision, or null when the value is not one.
 *
 * The Server accepts canonical RFC 3339 UTC only, and an offset or a `datetime-local` value means
 * the same instant once it is stated in UTC, so a value this client can read is rewritten in the
 * form the Server documents. A value nobody can read is not guessed at and not replaced with the
 * current time: it is left as the link wrote it, for the Server to name in its own refusal.
 */
export function canonicalInstant(
  value: string | null | undefined,
): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  if (trimmed === "") return null;
  const date = new Date(trimmed);
  if (Number.isNaN(date.getTime())) return null;
  return date.toISOString().slice(0, 19) + "Z";
}

/** The local-time value a `datetime-local` control shows for an instant, or '' when there is none. */
export function localInputValue(instant: string | null | undefined): string {
  const canonical = canonicalInstant(instant);
  if (canonical === null) return "";
  const date = new Date(canonical);
  const pad = (value: number) => String(value).padStart(2, "0");
  return (
    date.getFullYear() +
    "-" +
    pad(date.getMonth() + 1) +
    "-" +
    pad(date.getDate()) +
    "T" +
    pad(date.getHours()) +
    ":" +
    pad(date.getMinutes())
  );
}

export type InvestigationLinkRead = {
  /** The window the link names, handed to the Server as the link states it. */
  request: InvestigationRequest;
  /** The Incident occurrence the link located, when it named one. */
  occurrence: string | null;
  /** The Incident the link came from, when it named one. */
  incident: string | null;
  /** The window query the link carried, kept verbatim so the page can show what was asked for. */
  asked: string;
};

/**
 * Read the window, the occurrence and the Incident a link carries.
 *
 * Nothing about the window is judged here. Which windows the Server reads — and whether both ends of
 * a range, one end, an instant nobody can parse or a preset it has never heard of is acceptable — is
 * the Server's own rule, stated by the Server beside the window it refused, so a link it will not
 * answer is sent rather than quietly replaced with something else the reader never asked for. Only
 * the two ends this client can canonicalize are rewritten, because `datetime-local` values and
 * offsets both mean one instant once stated in UTC; an end nobody can read is passed through exactly
 * as the link wrote it, for the Server to name.
 */
export function readInvestigationLink(
  search: URLSearchParams,
): InvestigationLinkRead {
  const occurrence = canonicalInstant(
    search.get(INVESTIGATION_OCCURRENCE_PARAM),
  );
  const incident =
    (search.get(INVESTIGATION_INCIDENT_PARAM) ?? "").trim() || null;
  const window = (search.get(INVESTIGATION_WINDOW_PARAM) ?? "").trim();
  const from = readRangeEnd(search.get(INVESTIGATION_FROM_PARAM));
  const to = readRangeEnd(search.get(INVESTIGATION_TO_PARAM));
  const request: InvestigationRequest = {};
  if (window !== "") request.window = window;
  if (from !== undefined) request.from = from;
  if (to !== undefined) request.to = to;
  return { request, occurrence, incident, asked: search.toString() };
}

/** The end of a range as the link wrote it, canonicalized when it is an instant this client reads. */
function readRangeEnd(value: string | null | undefined): string | undefined {
  const trimmed = (value ?? "").trim();
  if (trimmed === "") return undefined;
  return canonicalInstant(trimmed) ?? trimmed;
}

/** The query the investigation read is asked with. An unnamed window sends no parameter at all. */
export function investigationQuery(request: InvestigationRequest): {
  window?: string;
  from?: string;
  to?: string;
} {
  const query: { window?: string; from?: string; to?: string } = {};
  if (request.window !== undefined) query.window = request.window;
  if (request.from !== undefined) query.from = request.from;
  if (request.to !== undefined) query.to = request.to;
  return query;
}

/**
 * The cache identity of one requested window, so two windows never share an answer.
 *
 * The requested window is keyed rather than the resolved one: the resolved window only exists once
 * an answer has arrived, and a cache entry has to be identified before the read is made.
 */
export function investigationKey(request: InvestigationRequest): string {
  const window = request.window ?? "";
  const from = request.from ?? "";
  const to = request.to ?? "";
  if (window === "" && from === "" && to === "") return "server-default";
  return "asked:" + window + "|" + from + "|" + to;
}

/** The most Peer receipt buckets one range answer may carry. This is the Server's own bound
 * (crates/platpulse-server/src/http/admin.rs MAX_PEER_RANGE_LIMIT), so a dense window is narrowed by
 * policy rather than by this page asking for less than the Server would answer. */
export const PEER_RANGE_LIMIT = 1000;

/**
 * The search a link for one window carries, with the located occurrence kept attached to it.
 *
 * A page that walks into another window of the same investigation must not lose the Incident it came
 * from: the occurrence is what makes the new window readable as evidence about that Incident.
 */
export function investigationSearch(
  request: InvestigationRequest,
  extras: { occurrence?: string | null; incident?: string | null } = {},
): string {
  const search = new URLSearchParams();
  if (request.window !== undefined && request.window !== "")
    search.set(INVESTIGATION_WINDOW_PARAM, request.window);
  if (request.from !== undefined && request.from !== "")
    search.set(INVESTIGATION_FROM_PARAM, request.from);
  if (request.to !== undefined && request.to !== "")
    search.set(INVESTIGATION_TO_PARAM, request.to);
  const occurrence = canonicalInstant(extras.occurrence ?? null);
  if (occurrence !== null)
    search.set(INVESTIGATION_OCCURRENCE_PARAM, occurrence);
  const incident = (extras.incident ?? "").trim();
  if (incident !== "") search.set(INVESTIGATION_INCIDENT_PARAM, incident);
  const text = search.toString();
  return text === "" ? "" : "?" + text;
}

/**
 * What each time basis means for the instants read beside it.
 *
 * The Server names the basis of every source; this turns that name into the sentence an Operator needs
 * to compare two sources' timestamps without reading one source's instant as another's. A basis this
 * build does not know keeps no sentence rather than getting a wrong one.
 */
const TIME_BASIS_NOTES: Record<string, string> = {
  metric_observation:
    "Every instant here is when the Agent observed the value, never when the Server received it: the receipt instant and the delay between the two are shown beside it.",
  state_observation:
    "Every instant here is when the Agent observed the state; an unchanged state is re-recorded as its own anchor rather than inferred between two rows.",
  peer_receipt_bucket:
    "Every instant here is when the Server received the Peer snapshot: a receipt bucket is aligned to that instant, and the newest observation inside the bucket is shown beside it.",
  incident_evaluation:
    "Every instant here is when a rule evaluation opened the Incident, not when the condition it names began to hold.",
  validator_snapshot_source:
    "Every instant here is when the Validator snapshot was taken, on the Validator source clock, and it is shown as taken rather than corrected toward the Server clock.",
};

export function timeBasisNote(basis: string): string | null {
  return TIME_BASIS_NOTES[basis] ?? null;
}

/** The evidence grain names a Peer receipt range can be read at. */
const PEER_GRAIN_BY_EVIDENCE: Record<string, string> = {
  receipt_bucket_5m: "5m",
  receipt_bucket_1h: "1h",
};

/**
 * The receipt grains a Peer source proves evidence at, in the order the Server lists them.
 *
 * The grain is named by the evidence the Server reported, never by a list this page keeps: a Peer
 * source that holds no receipt bucket offers no grain to read, and an evidence name this build does
 * not know is left out rather than mapped to a tier that would answer a different grid.
 */
export function peerGrainOptions(
  source: InvestigationSourceResponse | undefined,
): { value: string; label: string }[] {
  const options: { value: string; label: string }[] = [];
  for (const grain of source?.grains ?? []) {
    const value = PEER_GRAIN_BY_EVIDENCE[grain.grain];
    if (value === undefined) continue;
    // The choice is named by the receipt grain the range is read at, with the
    // width the Server reported beside it, so no page-level name is invented for
    // a tier the Server named itself.
    options.push({
      value,
      label: grain.grainSeconds
        ? value + " (" + grain.grainSeconds + "s)"
        : value,
    });
  }
  return options;
}

export type CoverageTone = "ok" | "warning" | "error" | "neutral";

/**
 * The tone a verdict is drawn with. Colour supplements the verdict's own word, never replaces it,
 * and a verdict that is not the Server's fault is not painted as an error: an empty window, a
 * family without a linked Validator and evidence older than retention are all honest answers.
 */
export function coverageTone(coverage: string): CoverageTone {
  if (coverage === "complete") return "ok";
  if (coverage === "partial") return "warning";
  return "neutral";
}

/** True when a source answers every instant of the window. */
export function coverageAnswersTheWholeWindow(coverage: string): boolean {
  return coverage === "complete";
}

/** What a verdict means for the counts read beside it. */
export function coverageNotice(coverage: string): string {
  switch (coverage) {
    case "complete":
      return "Every part of this window is answered from stored evidence.";
    case "partial":
      return "Part of this window cannot be answered; the boundaries below name the stretch and the reason.";
    case "empty":
      return "This window can be answered and holds no evidence: an empty window is not a failed collection, and it is never zero.";
    case "unavailable":
      return "No stored evidence answers this window: what the Server holds does not reach back this far, or this family was never collected for this subject.";
    case "unsupported":
      return "This family does not apply to this Node, so there is nothing to answer with or without a window.";
    default:
      return coverage;
  }
}

/** True when an instant falls inside `[from, to)`: the coordinate a boundary or a hole covers. */
export function instantWithin(
  from: string,
  to: string | null | undefined,
  instant: string,
): boolean {
  const at = Date.parse(instant);
  const start = Date.parse(from);
  if (Number.isNaN(at) || Number.isNaN(start)) return false;
  if (at < start) return false;
  if (to === null || to === undefined) return true;
  const end = Date.parse(to);
  if (Number.isNaN(end)) return true;
  return at < end;
}

/** The boundary covering an instant, when one does. */
export function boundaryAt(
  boundaries: InvestigationBoundaryResponse[] | undefined,
  instant: string,
): InvestigationBoundaryResponse | null {
  for (const boundary of boundaries ?? []) {
    if (instantWithin(boundary.at, boundary.to ?? null, instant))
      return boundary;
  }
  return null;
}

/** The hole covering an instant, when one does. Holes are the grains' own silences. */
export function holeAt(
  grains: InvestigationGrainResponse[] | undefined,
  instant: string,
): InvestigationHoleResponse | null {
  for (const grain of grains ?? []) {
    for (const hole of grain.holes) {
      if (instantWithin(hole.from, hole.to, instant)) return hole;
    }
  }
  return null;
}

export type SourceAtInstant = {
  boundary: InvestigationBoundaryResponse | null;
  hole: InvestigationHoleResponse | null;
  /** True when neither a boundary nor a hole covers the instant. */
  answered: boolean;
  /** False when the family does not apply to this Node at all. */
  applicable: boolean;
};

/**
 * What one source says about one instant.
 *
 * A boundary is a statement about the source's coverage, so it wins over a grain's own hole: the hole
 * explains a silence inside a stretch this source does answer, while the boundary explains a stretch
 * it does not. Reporting a hole for an instant a boundary already refuses would read as evidence
 * where there is none.
 *
 * A family the Server marked as not applying to this Node holds no boundaries and no holes, so it is
 * named as inapplicable instead of being read as a family that answers the instant with evidence.
 */
export function sourceAtInstant(
  source: InvestigationSourceResponse,
  instant: string,
): SourceAtInstant {
  const boundary = boundaryAt(source.boundaries, instant);
  const hole = boundary === null ? holeAt(source.grains, instant) : null;
  return {
    boundary,
    hole,
    answered: boundary === null && hole === null,
    applicable: source.coverage !== "unsupported",
  };
}

/** How much evidence one grain holds, in the Operator's words. */
export function grainEvidenceLabel(grain: InvestigationGrainResponse): string {
  if (!grain.available)
    return grain.note ?? "No stored point of this grain falls in the window";
  const points =
    grain.pointCount === 1
      ? "1 stored point"
      : grain.pointCount + " stored points";
  if (grain.sampleCount === 0 || grain.sampleCount === grain.pointCount)
    return points;
  const samples =
    grain.sampleCount === 1
      ? "1 observation"
      : grain.sampleCount + " observations";
  return points + " covering " + samples;
}

/** The points a grain proves missing, or null when that count is not knowable. */
export function grainMissingLabel(
  grain: InvestigationGrainResponse,
): string | null {
  if (grain.expectedPoints === null || grain.expectedPoints === undefined)
    return null;
  if (grain.missingPoints === null || grain.missingPoints === undefined)
    return null;
  if (grain.missingPoints === 0)
    return "No point of this grain is missing inside its stretch.";
  return (
    grain.missingPoints +
    " of " +
    grain.expectedPoints +
    " expected points are missing inside its stretch."
  );
}

/** The grain's own name, with its width when it has one: `5m` is a name, not a width. */
export function grainLabel(grain: InvestigationGrainResponse): string {
  if (grain.grainSeconds === null || grain.grainSeconds === undefined)
    return grain.grain;
  return grain.grain + " (" + grain.grainSeconds + "s)";
}

/** The delay between an observation and its receipt, or null when either instant is missing.
 * The signed value is returned as measured: a negative one is an observation stamped after the
 * receipt that carried it, which is a suspicious clock rather than a fast delivery. */
export function receiptDelaySeconds(
  observedAt: string | null | undefined,
  receivedAt: string | null | undefined,
): number | null {
  const observed = canonicalInstant(observedAt ?? null);
  const received = canonicalInstant(receivedAt ?? null);
  if (observed === null || received === null) return null;
  return (Date.parse(received) - Date.parse(observed)) / 1000;
}
