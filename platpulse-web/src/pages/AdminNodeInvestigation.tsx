/**
 * PAGE-ADMIN-NODE-INVESTIGATION (design §11.4, §11.5, §11.6, §11.7; webui.md §15.22, issue #220):
 * one UTC window over every evidence family the Server holds for one Node.
 *
 * The window lives in the URL and the Server resolves it: a shared or reloaded link reads the same
 * stretch instead of re-deriving it from the browser's clock, and the widths the page offers are the
 * presets the Server named in its own answer rather than a list this client keeps. A link this client
 * cannot read is named instead of completed — the page falls back to the Server's default window and
 * says which query it ignored, because a window the Server never answered must never be presented as
 * the one that was asked for.
 *
 * Coverage here is the Server's verdict about a stretch, never health: partial, empty, unavailable and
 * unsupported are four different statements about the evidence, and none of them is zero. Every
 * conclusion carries the existing endpoint that answers it, and Peer evidence is read as its own
 * bounded range rather than a fixed top list, so buckets older than a limit stay readable and a dense
 * window is paged instead of being silently cut.
 */
import { useMemo, useState, type ReactNode } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";
import {
  AdminApiError,
  useAdminNodeInvestigation,
  useAdminNodeMetricHistory,
  useAdminNodePeerRange,
  useAdminNodeStateHistory,
  type AdminInvestigation,
  type AdminPeerRangeAnswer,
} from "../api/admin";
import { useAuth } from "../auth/AuthContext";
import { StatusBadge, formatObservedAt } from "../components/StatusBadge";
import { Button } from "../components/ui/button";
import { CardX } from "../components/ui/card-x";
import { Disclosure } from "../components/ui/disclosure";
import { Empty } from "../components/ui/empty";
import { Input, Select } from "../components/ui/input";
import { cn } from "../lib/utils";
import { SURFACE_CARD_STATIC } from "../lib/surface";
import {
  NODE_METRIC_SERIES,
  formatHistoryDuration,
  formatSampleDelay,
  nodeMetricDefinition,
  type NodeMetricKey,
} from "../metricHistory";
import {
  stateHistoryComponent,
  type StateHistoryComponent,
} from "../stateHistory";
import {
  MetricHistoryBody,
  type MetricHistorySelection,
  type MetricHistoryWindow,
} from "./metricHistoryPanel";
import { StateHistoryBody } from "./stateHistoryPanel";
import {
  canonicalInstant,
  coverageNotice,
  coverageTone,
  grainEvidenceLabel,
  grainLabel,
  grainMissingLabel,
  investigationSearch,
  INVALID_WINDOW_CODE,
  localInputValue,
  peerGrainOptions,
  SERVER_DEFAULT_WINDOW,
  readInvestigationLink,
  receiptDelaySeconds,
  sourceAtInstant,
  timeBasisNote,
  type InvestigationRequest,
} from "../nodeInvestigation";
import type {
  InvestigationGrainResponse,
  InvestigationSourceResponse,
  InvestigationWindowResponse,
} from "../api/generated";

const CARD_SURFACE = cn("rounded-md border-none", SURFACE_CARD_STATIC);
const TH = "px-3 py-2 text-left text-xs font-medium text-muted-foreground";
const TD = "px-3 py-2 align-top";
const PAGE_LINK =
  "inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline";
const SCOPE = "min-w-0 break-words";

/** Emerald detail grid: a label above its value, stacked on narrow viewports. */
function Facts({ children }: { children: ReactNode }) {
  return <dl className="grid grid-cols-1 gap-3 sm:grid-cols-2">{children}</dl>;
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs font-medium tracking-wider text-muted-foreground">
        {label}
      </dt>
      <dd className="mt-0.5 min-w-0 break-words text-sm">{children}</dd>
    </div>
  );
}

/** The instant pair one row of evidence covers, or the honest absence of one. */
function instantPair(
  from: string | null | undefined,
  to: string | null | undefined,
): string {
  if (!from) return "None held";
  if (!to || to === from) return formatObservedAt(from);
  return formatObservedAt(from) + " to " + formatObservedAt(to);
}

/** The page's window, as every card on it reads it. */
type PageWindow = {
  /** The answered width in hours; a custom range can be a fraction of one. */
  hours: number;
  /** The answered range: every card reads this range and no other. */
  range: { from: string; to: string };
  /** Answer another width, through the presets the Server named. */
  selectRange: (nextHours: number) => void;
  /** Re-answer every card of this window as of now. */
  reload: () => void;
};

/**
 * The page's window with a page cursor per card.
 *
 * The cursor is keyed by the window it belongs to, so walking into an older page of one card never
 * pages another card and a new window always starts at its newest page. The window itself is shared:
 * two cards on this page can never read two ranges.
 */
function usePanelWindow(page: PageWindow): MetricHistoryWindow {
  const identity = page.range.from + "|" + page.range.to;
  const [cursor, setCursor] = useState<{
    window: string;
    before: string | null;
  }>({
    window: identity,
    before: null,
  });
  const before = cursor.window === identity ? cursor.before : null;
  return {
    hours: page.hours,
    range: page.range,
    olderThan: before,
    setOlderThan(next) {
      setCursor({ window: identity, before: next });
    },
    selectRange(next) {
      setCursor({ window: identity, before: null });
      page.selectRange(next);
    },
    reload() {
      setCursor({ window: identity, before: null });
      page.reload();
    },
  };
}

/**
 * One window, named in the URL.
 *
 * The preset buttons are the Server's own list, arriving in its answer, and the range controls hand
 * the Server the ends that were typed: whether a window is readable, wide enough, or the right way
 * round is the Server's own rule, so a refused window is reported here in the Server's words instead
 * of being replaced by a window nobody asked for.
 */
function WindowControl({
  resolved,
  refused,
  onPreset,
  onRange,
  onDefault,
  onReload,
}: {
  resolved: InvestigationWindowResponse | undefined;
  /** The Server's own reason for refusing the window the link names, when it refused one. */
  refused: string | null;
  onPreset: (key: string) => void;
  onRange: (from: string, to: string) => void;
  onDefault: () => void;
  onReload: () => void;
}) {
  const seed =
    localInputValue(resolved?.from) + "|" + localInputValue(resolved?.to);
  const [typed, setTyped] = useState<{
    seed: string;
    from: string;
    to: string;
  }>({
    seed: "",
    from: "",
    to: "",
  });
  const from =
    typed.seed === seed ? typed.from : localInputValue(resolved?.from);
  const to = typed.seed === seed ? typed.to : localInputValue(resolved?.to);
  const presets = resolved?.supportedPresets ?? [];

  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={<h2 className="text-lg font-semibold">Window</h2>}
    >
      <p className="text-sm text-muted-foreground">
        One window answers every family below. The window is held in this link,
        so a refreshed or shared address reads the same stretch rather than
        re-deriving it from this browser's clock, and every card on the page
        reads the range the Server answered.
      </p>
      <div
        className="mt-3 flex flex-wrap items-end gap-3"
        role="group"
        aria-label="Investigation window"
        data-slot="investigation-window"
      >
        <div className="flex flex-wrap gap-2">
          {presets.map((preset) => {
            const current = resolved?.preset === preset.key;
            return (
              <Button
                key={preset.key}
                variant={current ? "default" : "outline"}
                aria-pressed={current}
                className="min-h-11"
                onClick={() => onPreset(preset.key)}
              >
                {preset.label}
              </Button>
            );
          })}
        </div>
        <label className="grid min-w-40 gap-1 text-xs font-medium text-muted-foreground">
          From
          <Input
            type="datetime-local"
            className="min-h-11"
            value={from}
            onChange={(event) =>
              setTyped({ seed, from: event.currentTarget.value, to })
            }
          />
        </label>
        <label className="grid min-w-40 gap-1 text-xs font-medium text-muted-foreground">
          To
          <Input
            type="datetime-local"
            className="min-h-11"
            value={to}
            onChange={(event) =>
              setTyped({ seed, from, to: event.currentTarget.value })
            }
          />
        </label>
        <Button
          variant="outline"
          className="min-h-11"
          onClick={() => onRange(from, to)}
        >
          Read this range
        </Button>
        <Button variant="ghost" className="min-h-11" onClick={onDefault}>
          Server default window
        </Button>
        <Button
          variant="ghost"
          className="min-h-11"
          data-slot="investigation-reload"
          onClick={onReload}
        >
          Reload window
        </Button>
      </div>
      {refused !== null && (
        <p
          className="mt-2 min-w-0 break-words text-sm text-destructive"
          role="alert"
          data-slot="investigation-window-refused"
        >
          The Server refused the window this link names: {refused} Nothing was
          read for it; read another window below, or ask for the Server's own
          default window.
        </p>
      )}
      {resolved && (
        <div className="mt-3">
          <Facts>
            <Fact label="Answered range">
              {formatObservedAt(resolved.from)} to{" "}
              {formatObservedAt(resolved.to)}
            </Fact>
            <Fact label="Width">
              {formatHistoryDuration(resolved.durationSeconds)}
            </Fact>
            <Fact label="Window named by">
              {resolved.custom
                ? "An explicit range in this link"
                : resolved.presetLabel + " (the Server resolved it)"}
            </Fact>
            <Fact label="Answered at">
              {formatObservedAt(resolved.answeredAt)}
            </Fact>
            <Fact label="Widest window">{resolved.horizonDays} days</Fact>
            <Fact label="Raw observations kept">
              {resolved.rawRetentionDays === 1
                ? "1 day; older stretches are answered by aggregate buckets, not by raw points"
                : resolved.rawRetentionDays + " days"}
            </Fact>
          </Facts>
        </div>
      )}
      {resolved?.clampedToNow && (
        <p
          className="mt-2 text-sm text-muted-foreground"
          role="status"
          data-slot="investigation-window-clamped"
        >
          This link asked for a range ending at{" "}
          {formatObservedAt(resolved.requestedTo)}, which is later than the
          Server answered it: the window was read up to
          {" " + formatObservedAt(resolved.to)} and the end was not filled with
          anything.
        </p>
      )}
    </CardX>
  );
}

/** What the Server holds for the Node itself, before any window is read. */
function NodeFactsCard({ answer }: { answer: AdminInvestigation }) {
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={
        <>
          <h2 className="text-lg font-semibold">
            {answer.displayName || "This Node"}
          </h2>
          <StatusBadge status={answer.lifecycle} tone="neutral" />
        </>
      }
    >
      <Facts>
        <Fact label="Node">
          <span className="break-all font-mono text-xs">{answer.nodeId}</span>
        </Fact>
        <Fact label="Reporting Agent">
          <Link
            className="inline-flex min-h-11 min-w-11 items-center break-all font-mono text-xs underline-offset-4 hover:underline"
            to={"/admin/agents/" + encodeURIComponent(answer.agentId)}
          >
            {answer.agentId}
          </Link>
        </Fact>
        <Fact label="Visibility">
          {answer.visibility === "public" ? "Public" : "Owner only"}
        </Fact>
        <Fact label="Evidence families">
          {answer.sources.length === 6
            ? "All six families are listed, whether or not each holds evidence"
            : answer.sources.length + " families are listed"}
        </Fact>
      </Facts>
      {answer.notes.length > 0 && (
        <ul
          className="mt-3 grid gap-1 text-sm text-muted-foreground"
          data-slot="investigation-notes"
        >
          {answer.notes.map((note) => (
            <li key={note} className={SCOPE}>
              {note}
            </li>
          ))}
        </ul>
      )}
    </CardX>
  );
}

/**
 * Where the located occurrence falls in each family.
 *
 * The instant an Incident opened is located against the evidence rather than assumed to have been
 * observed: a boundary wins over a grain's own silence, because a boundary is the source's own
 * statement that it does not answer that stretch at all.
 */
function OccurrenceCard({
  answer,
  occurrence,
  incident,
}: {
  answer: AdminInvestigation;
  occurrence: string;
  incident: string | null;
}) {
  const openedFromOccurrence =
    answer.window.custom && answer.window.requestedFrom === occurrence;
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={
        <>
          <h2 className="text-lg font-semibold">The located occurrence</h2>
          <StatusBadge status="Located" tone="neutral" />
        </>
      }
      contentClassName="min-w-0"
    >
      <p
        className="text-sm text-muted-foreground"
        data-slot="investigation-occurrence"
      >
        The Incident opened at {formatObservedAt(occurrence)}. Nothing below
        claims what happened then: each family either answers that instant,
        names the stretch it cannot answer, or names the silence its own grain
        proves.
      </p>
      {openedFromOccurrence && (
        <p
          className="mt-1 text-sm text-muted-foreground"
          data-slot="investigation-occurrence-entry"
        >
          This window starts at that instant: it was opened from the Incident
          rather than chosen here, and the width is the width the link asked
          for.
        </p>
      )}
      {incident && (
        <p className="mt-2 text-sm">
          <Link
            className={PAGE_LINK}
            to={"/admin/alerts/incidents/" + encodeURIComponent(incident)}
          >
            Back to the Incident
          </Link>
        </p>
      )}
      <div
        data-slot="investigation-occurrence-list"
        className="mt-3 overflow-x-auto"
      >
        <table
          data-stack
          data-slot="investigation-occurrence-table"
          className="w-full min-w-[44rem] text-sm"
        >
          <caption className="sr-only">
            Each evidence family and what it says about the located instant
          </caption>
          <thead>
            <tr className="border-b border-border">
              <th scope="col" className={TH}>
                Evidence
              </th>
              <th scope="col" className={TH}>
                What this instant falls in
              </th>
            </tr>
          </thead>
          <tbody>
            {answer.sources.map((source) => {
              const located = sourceAtInstant(source, occurrence);
              return (
                <tr
                  key={source.key}
                  className="border-b border-border/60 last:border-0"
                >
                  <td className={TD}>
                    <span className="font-medium">{source.label}</span>
                    <span className="block text-xs text-muted-foreground">
                      {source.coverageLabel}
                    </span>
                  </td>
                  <td className={cn(TD, SCOPE)}>
                    {!located.applicable ? (
                      <span>This family does not apply to this Node.</span>
                    ) : located.boundary ? (
                      <span>
                        <span className="font-medium">
                          {located.boundary.kindLabel}
                        </span>{" "}
                        — {located.boundary.detail}{" "}
                        <span className="text-xs text-muted-foreground">
                          (
                          {instantPair(
                            located.boundary.at,
                            located.boundary.to,
                          )}
                          )
                        </span>
                      </span>
                    ) : located.hole ? (
                      <span>
                        A silence this grain proves:{" "}
                        {instantPair(located.hole.from, located.hole.to)}
                        {located.hole.seconds != null
                          ? " (" +
                            formatHistoryDuration(located.hole.seconds) +
                            ")"
                          : ""}
                        {located.hole.series
                          ? " on " + located.hole.series
                          : ""}
                        .
                      </span>
                    ) : (
                      <span>
                        No boundary and no silence of this family covers the
                        instant.
                      </span>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </CardX>
  );
}

/** One grain of one family: what it holds, what it proves missing, and where it is silent. */
function GrainTable({ grains }: { grains: InvestigationGrainResponse[] }) {
  return (
    <div data-slot="investigation-grains-list" className="mt-2 overflow-x-auto">
      <table
        data-stack
        data-slot="investigation-grains"
        className="w-full min-w-[52rem] text-sm"
      >
        <caption className="sr-only">
          Evidence grains and the stretches they serve
        </caption>
        <thead>
          <tr className="border-b border-border">
            <th scope="col" className={TH}>
              Grain
            </th>
            <th scope="col" className={TH}>
              Evidence in this window
            </th>
            <th scope="col" className={TH}>
              Missing points
            </th>
            <th scope="col" className={TH}>
              Stretch served
            </th>
            <th scope="col" className={TH}>
              Longest gap
            </th>
          </tr>
        </thead>
        <tbody>
          {grains.map((grain) => (
            <tr
              key={grain.grain}
              className="border-b border-border/60 last:border-0"
            >
              <td className={TD}>
                <span className="font-medium">{grainLabel(grain)}</span>
                <span className="block text-xs text-muted-foreground">
                  {grain.available ? "Holds evidence" : "Holds no point here"}
                </span>
              </td>
              <td className={cn(TD, SCOPE)}>
                {grainEvidenceLabel(grain)}
                <span className="block text-xs text-muted-foreground">
                  {grain.cadenceSeconds == null
                    ? "Not counted for this grain"
                    : "Fastest interval shown: " +
                      formatHistoryDuration(grain.cadenceSeconds)}
                </span>
              </td>
              <td className={cn(TD, SCOPE)}>
                {grainMissingLabel(grain) ?? "Not counted for this grain"}
              </td>
              <td className={TD}>{instantPair(grain.from, grain.to)}</td>
              <td className={TD}>
                {grain.longestGapSeconds == null
                  ? "Not counted for this grain"
                  : formatHistoryDuration(grain.longestGapSeconds)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** The stretches of one family's window it cannot answer, with the Server's own reason. */
function BoundaryTable({ source }: { source: InvestigationSourceResponse }) {
  if (source.boundaries.length === 0) {
    return (
      <p
        className="mt-2 text-sm text-muted-foreground"
        data-slot="investigation-no-boundary"
      >
        No stretch of this window is refused by this family on its own account.
      </p>
    );
  }
  return (
    <div
      data-slot="investigation-boundaries-list"
      className="mt-2 overflow-x-auto"
    >
      <table
        data-stack
        data-slot="investigation-boundaries"
        className="w-full min-w-[44rem] text-sm"
      >
        <caption className="sr-only">
          Stretches of the window this family cannot answer
        </caption>
        <thead>
          <tr className="border-b border-border">
            <th scope="col" className={TH}>
              Kind
            </th>
            <th scope="col" className={TH}>
              Stretch
            </th>
            <th scope="col" className={TH}>
              Why
            </th>
          </tr>
        </thead>
        <tbody>
          {source.boundaries.map((boundary) => (
            <tr
              key={boundary.kind + boundary.at}
              className="border-b border-border/60 last:border-0"
            >
              <td className={TD}>
                <span className="font-medium">{boundary.kindLabel}</span>
                <span className="block font-mono text-xs text-muted-foreground">
                  {boundary.kind}
                </span>
              </td>
              <td className={TD}>{instantPair(boundary.at, boundary.to)}</td>
              <td className={cn(TD, SCOPE)}>{boundary.detail}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** The collectors behind one family, with what each last recorded. */
function CollectorTable({ source }: { source: InvestigationSourceResponse }) {
  if (source.components.length === 0) {
    return (
      <p className="mt-2 text-sm text-muted-foreground">
        This family has no collector row of its own: it is answered from the
        Server's own records.
      </p>
    );
  }
  return (
    <div
      data-slot="investigation-components-list"
      className="mt-2 overflow-x-auto"
    >
      <table
        data-stack
        data-slot="investigation-components"
        className="w-full min-w-[52rem] text-sm"
      >
        <caption className="sr-only">
          Collectors behind this family and their last recorded state
        </caption>
        <thead>
          <tr className="border-b border-border">
            <th scope="col" className={TH}>
              Collector
            </th>
            <th scope="col" className={TH}>
              Scope
            </th>
            <th scope="col" className={TH}>
              Last state
            </th>
            <th scope="col" className={TH}>
              Observed
            </th>
            <th scope="col" className={TH}>
              Received
            </th>
            <th scope="col" className={TH}>
              Delay
            </th>
          </tr>
        </thead>
        <tbody>
          {source.components.map((component) => (
            <tr
              key={
                component.scope + component.scopeKey + component.componentKey
              }
              className="border-b border-border/60 last:border-0"
            >
              <td className={cn(TD, SCOPE)}>
                <span className="font-mono text-xs">
                  {component.componentKey}
                </span>
                {component.errorCode && (
                  <span className="block text-xs text-muted-foreground">
                    Last failure: {component.errorCode}
                  </span>
                )}
              </td>
              <td className={TD}>
                <span className="font-mono text-xs">{component.scope}</span>
                <span className="block break-all font-mono text-xs text-muted-foreground">
                  {component.scopeKey}
                </span>
              </td>
              <td className={TD}>
                <StatusBadge
                  status={component.state}
                  tone={
                    component.state === "ok"
                      ? "ok"
                      : component.state === "error"
                        ? "error"
                        : "neutral"
                  }
                />
              </td>
              <td className={TD}>{formatObservedAt(component.observedAt)}</td>
              <td className={TD}>{formatObservedAt(component.receivedAt)}</td>
              <td className={TD}>
                {formatSampleDelay(
                  receiptDelaySeconds(
                    component.observedAt ?? null,
                    component.receivedAt ?? null,
                  ),
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** What the Server holds before the window, and why it cannot answer further back. */
function RetentionFacts({ source }: { source: InvestigationSourceResponse }) {
  const statements: string[] = [];
  if (source.retentionDays != null) {
    statements.push(
      "This family is declared with " +
        source.retentionDays +
        (source.retentionDays === 1
          ? " day of retention"
          : " days of retention") +
        ", so cleanup is not allowed to release newer evidence than that.",
    );
  }
  if (source.retainedFrom) {
    statements.push(
      "The stretch before " +
        formatObservedAt(source.retainedFrom) +
        " cannot be answered from this family: it is older than that retention.",
    );
  }
  if (source.releasedBefore) {
    statements.push(
      "The newest cleanup this family ran released evidence older than " +
        formatObservedAt(source.releasedBefore) +
        ".",
    );
  }
  if (statements.length === 0) {
    statements.push(
      "This family declares no retention of its own, so its evidence is released by the policy of the Server rather than by a width kept here.",
    );
  }
  return (
    <p
      className="mt-2 text-sm text-muted-foreground"
      data-slot="investigation-retention"
    >
      {statements.join(" ")}
    </p>
  );
}

/** One family of evidence: its verdict, its clock, its grains, its silences and its own answer path. */
function SourceEvidence({
  source,
  occurrence,
}: {
  source: InvestigationSourceResponse;
  occurrence: string | null;
}) {
  const located = occurrence ? sourceAtInstant(source, occurrence) : null;
  const note = timeBasisNote(source.timeBasis);
  return (
    <div data-slot="investigation-source" data-source={source.key}>
      <Disclosure
        surface="none"
        title={source.label}
        description={
          source.coverageLabel +
          " · " +
          source.timeBasisLabel +
          (source.truncated ? " · detail omitted" : "")
        }
      >
        <p
          className="text-sm text-muted-foreground"
          data-slot="investigation-source-verdict"
        >
          {coverageNotice(source.coverage)}
        </p>
        {located && (
          <p className="text-sm" data-slot="investigation-source-occurrence">
            {!located.applicable
              ? "This family does not apply to this Node, so it says nothing about the located instant."
              : located.boundary
                ? "The located instant falls inside " +
                  located.boundary.kindLabel +
                  ": " +
                  located.boundary.detail
                : located.hole
                  ? "The located instant falls inside a silence this family proves."
                  : "No boundary and no silence of this family covers the located instant."}
          </p>
        )}
        <p
          className="text-sm text-muted-foreground"
          data-slot="investigation-source-clock"
        >
          {source.timeBasisLabel}
          {note ? " — " + note : ""}
        </p>
        <Facts>
          <Fact label="Evidence held">
            {instantPair(source.firstObservedAt, source.lastObservedAt)}
          </Fact>
          <Fact label="Newest receipt">
            {formatObservedAt(source.lastReceivedAt)}
          </Fact>
          <Fact label="Subject">
            <span className="break-all font-mono text-xs">
              {source.subjectKind} {source.subject}
            </span>
          </Fact>
          <Fact label="Recorded collection state">
            {source.sourceState ?? "Not recorded for this family"}
          </Fact>
        </Facts>
        <RetentionFacts source={source} />
        {source.truncated && (
          <p
            className="text-sm text-muted-foreground"
            role="status"
            data-slot="investigation-source-truncated"
          >
            This answer omits detail the Server holds for this family; the
            counts above are the ones it kept, and the endpoint below answers
            the rest.
          </p>
        )}
        {source.errorCode && (
          <p className="text-sm text-muted-foreground">
            The last collection of this family failed with {source.errorCode}.
          </p>
        )}
        <GrainTable grains={source.grains} />
        {source.grains.some((grain) => grain.holes.length > 0) && (
          <ul
            className="mt-2 grid gap-1 text-sm"
            data-slot="investigation-holes"
          >
            {source.grains.flatMap((grain) =>
              grain.holes.map((hole) => (
                <li
                  key={grain.grain + hole.from + (hole.series ?? "")}
                  className={SCOPE}
                >
                  <span className="font-medium">{grainLabel(grain)}</span> is
                  silent from {formatObservedAt(hole.from)} to{" "}
                  {formatObservedAt(hole.to)}
                  {hole.seconds != null
                    ? " (" + formatHistoryDuration(hole.seconds) + ")"
                    : ""}
                  {hole.series ? " on " + hole.series : ""}.
                </li>
              )),
            )}
          </ul>
        )}
        <BoundaryTable source={source} />
        <CollectorTable source={source} />
        {source.notes.length > 0 && (
          <ul
            className="grid gap-1 text-sm text-muted-foreground"
            data-slot="investigation-source-notes"
          >
            {source.notes.map((item) => (
              <li key={item} className={SCOPE}>
                {item}
              </li>
            ))}
          </ul>
        )}
        <div className="grid gap-2" data-slot="investigation-answer-paths">
          <p className="text-xs font-medium tracking-wider text-muted-foreground">
            The endpoints that answer this family for this window
          </p>
          {source.answerPaths.map((path) => (
            <p key={path.path} className={SCOPE}>
              <a
                className="inline-flex min-h-11 min-w-11 items-center font-mono text-xs underline underline-offset-4"
                href={path.path}
                target="_blank"
                rel="noreferrer"
              >
                {path.label}
              </a>
              <span className="block break-all font-mono text-xs text-muted-foreground">
                {path.path}
              </span>
              {path.note && (
                <span className="block text-xs text-muted-foreground">
                  {path.note}
                </span>
              )}
            </p>
          ))}
        </div>
      </Disclosure>
    </div>
  );
}

/** All six families in one place: the verdict each reached, and the clock it reached it on. */
function CoverageCard({ answer }: { answer: AdminInvestigation }) {
  const occurrence = null;
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={
        <>
          <h2 className="text-lg font-semibold">Coverage of this window</h2>
          <StatusBadge
            status={answer.sources.length + " families"}
            tone="neutral"
          />
        </>
      }
    >
      <p className="text-sm text-muted-foreground">
        A verdict is what the Server can answer for this stretch, never whether
        the Node is well: partial, empty, unavailable and unsupported are four
        different answers, and a stretch with no evidence is not zero. Each
        family below states its own clock, because an instant an Agent observed
        and an instant the Server received are not the same instant.
      </p>
      <div
        data-slot="investigation-sources-list"
        className="mt-3 overflow-x-auto"
      >
        <table
          data-stack
          data-slot="investigation-sources"
          className="w-full min-w-[52rem] text-sm"
        >
          <caption className="sr-only">
            Evidence families, their coverage verdict, their clock and what they
            hold
          </caption>
          <thead>
            <tr className="border-b border-border">
              <th scope="col" className={TH}>
                Evidence
              </th>
              <th scope="col" className={TH}>
                Coverage
              </th>
              <th scope="col" className={TH}>
                Clock
              </th>
              <th scope="col" className={TH}>
                Evidence held
              </th>
              <th scope="col" className={TH}>
                Retention
              </th>
              <th scope="col" className={TH}>
                Collection
              </th>
            </tr>
          </thead>
          <tbody>
            {answer.sources.map((source) => (
              <tr
                key={source.key}
                className="border-b border-border/60 last:border-0"
              >
                <td className={cn(TD, SCOPE)}>
                  <span className="font-medium">{source.label}</span>
                  <span className="block break-all font-mono text-xs text-muted-foreground">
                    {source.subjectKind} {source.subject}
                  </span>
                </td>
                <td className={TD}>
                  <StatusBadge
                    status={source.coverageLabel}
                    tone={coverageTone(source.coverage)}
                  />
                  <span className="mt-1 block font-mono text-xs text-muted-foreground">
                    {source.coverage}
                  </span>
                </td>
                <td className={cn(TD, SCOPE)}>
                  {source.timeBasisLabel}
                  <span className="block font-mono text-xs text-muted-foreground">
                    {source.timeBasis}
                  </span>
                </td>
                <td className={TD}>
                  {instantPair(source.firstObservedAt, source.lastObservedAt)}
                </td>
                <td className={TD}>
                  {source.retentionDays != null
                    ? source.retentionDays +
                      (source.retentionDays === 1 ? " day" : " days")
                    : "Not declared"}
                </td>
                <td className={cn(TD, SCOPE)}>
                  {source.sourceState ?? "Not recorded"}
                  {source.errorCode && (
                    <span className="block text-xs text-muted-foreground">
                      Last failure: {source.errorCode}
                    </span>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="mt-3 grid gap-2">
        {answer.sources.map((source) => (
          <SourceEvidence
            key={source.key}
            source={source}
            occurrence={occurrence}
          />
        ))}
      </div>
    </CardX>
  );
}

/**
 * The Node metrics and the recorded Node state of the same window.
 *
 * Both cards reuse the panels the Node page already shows, driven by this page's window: their own
 * preset buttons and their own reload control move this page's window instead of quietly reading a
 * different stretch, and the range they read is always the range the Server answered.
 */
function NodeMetricEvidencePanel({
  nodeId,
  page,
}: {
  nodeId: string;
  page: PageWindow;
}) {
  const { generation } = useAuth();
  const view = usePanelWindow(page);
  const [metric, setMetric] = useState<NodeMetricKey>("process_cpu_percent");
  const answer = useAdminNodeMetricHistory(
    generation,
    nodeId,
    metric,
    page.range.from,
    page.range.to,
    view.olderThan ?? undefined,
  );
  const selection: MetricHistorySelection = {
    metric,
    definition: nodeMetricDefinition(metric),
    onMetric(next) {
      setMetric(next as NodeMetricKey);
      view.setOlderThan(null);
    },
  };
  return (
    <MetricHistoryBody
      title="Node metrics in this window"
      subject="Node"
      intro={
        // The panel renders its own paragraph, so this stays inline: a paragraph inside a paragraph
        // is not markup a browser accepts.
        <span>
          The same raw samples and aggregates the Node page charts, read over
          this window only. A stretch with no points is shown as the stretch it
          is: the availability line names what the Server cannot answer from
          this grain, and never as zero.
        </span>
      }
      surface={CARD_SURFACE}
      definitions={NODE_METRIC_SERIES}
      selection={selection}
      view={view}
      answer={answer}
    />
  );
}

function NodeStateEvidencePanel({
  nodeId,
  page,
}: {
  nodeId: string;
  page: PageWindow;
}) {
  const { generation } = useAuth();
  const view = usePanelWindow(page);
  const [component, setComponent] = useState<StateHistoryComponent>("sync");
  const answer = useAdminNodeStateHistory(
    generation,
    nodeId,
    component,
    page.range.from,
    page.range.to,
    view.olderThan ?? undefined,
  );
  return (
    <StateHistoryBody
      title="Recorded Node state in this window"
      subject="Node"
      surface={CARD_SURFACE}
      intro="Entries are the states the Server recorded, not a verdict about the Node now: an entry outside this window is not read as an answer inside it."
      component={component}
      onComponent={(next) => {
        setComponent(stateHistoryComponent(next));
        view.setOlderThan(null);
      }}
      view={view}
      answer={answer}
    />
  );
}

/**
 * Peer receipt evidence of the same window.
 *
 * Peers are read through their own bounded range rather than a fixed newest list, so a bucket older
 * than the fixed tail stays readable and a dense window is paged instead of being cut. A bucket start
 * is the instant the Server accepted the receipt — no Peer identity or address is persisted, so the
 * counts and country codes below are all the evidence there is.
 */
function PeerReceiptPanel({
  nodeId,
  source,
  from,
  to,
}: {
  nodeId: string;
  source: InvestigationSourceResponse;
  from: string;
  to: string;
}) {
  const { generation } = useAuth();
  const options = peerGrainOptions(source);
  const [picked, setPicked] = useState("");
  const grain = options.some((option) => option.value === picked)
    ? picked
    : (options[0]?.value ?? "");
  const identity = from + "|" + to + "|" + grain;
  const [cursor, setCursor] = useState<{
    window: string;
    before: string | null;
  }>({
    window: identity,
    before: null,
  });
  const before = cursor.window === identity ? cursor.before : null;
  const query = useAdminNodePeerRange(
    generation,
    nodeId,
    grain,
    from,
    to,
    before ?? undefined,
  );
  const answer: AdminPeerRangeAnswer | undefined = query.data;
  const range = answer?.range;
  const pick = (next: string | null) =>
    setCursor({ window: identity, before: next });

  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={
        <>
          <h2 className="text-lg font-semibold">Peer receipt evidence</h2>
          {range && (
            <StatusBadge
              status={range.coverage}
              tone={coverageTone(range.coverage)}
            />
          )}
        </>
      }
    >
      <p className="text-sm text-muted-foreground">
        Peer evidence is answered as its own range over one receipt grain rather
        than as the newest buckets: the answer names how many buckets the Server
        holds, how many the aligned grid expected across this window, which
        buckets are missing, and where the next page continues. A bucket exists
        only when the Node reported Peers as healthy, so a bucket the grid
        expected and the Server does not hold is a silence, not a zero.
      </p>
      {options.length === 0 ? (
        <div className="mt-3" data-slot="investigation-peer-no-grain">
          <Empty description="This family's own answer names no receipt grain for this window, so no bucket range can be read from it." />
        </div>
      ) : (
        <>
          <div className="mt-3 flex flex-wrap items-end gap-3">
            <label className="grid min-w-40 gap-1 text-xs font-medium text-muted-foreground">
              Receipt grain
              <Select
                data-slot="investigation-peer-grain"
                className="min-h-11"
                value={grain}
                onChange={(event) => {
                  setPicked(event.currentTarget.value);
                  pick(null);
                }}
              >
                {options.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </Select>
            </label>
            {before !== null && (
              <Button
                variant="ghost"
                className="min-h-11"
                onClick={() => pick(null)}
              >
                Back to the newest buckets
              </Button>
            )}
          </div>
          {query.isPending && (
            <p
              className="mt-3 flex flex-wrap items-center gap-2 text-sm text-muted-foreground"
              role="status"
            >
              <StatusBadge status="Starting" tone="neutral" /> Reading the Peer
              receipt range…
            </p>
          )}
          {query.isError && answer === undefined && (
            <div
              className="mt-3 flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
              role="alert"
            >
              <StatusBadge status="Error" tone="error" />{" "}
              <span className="min-w-0 break-words">
                {query.error instanceof Error
                  ? query.error.message
                  : "Unable to load the Peer receipt range"}
              </span>
              <Button
                variant="link"
                size="sm"
                onClick={() => void query.refetch()}
              >
                Try again
              </Button>
            </div>
          )}
          {query.isRefetchError && answer !== undefined && (
            <div
              className="mt-3 flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
              role="alert"
            >
              <StatusBadge status="Error" tone="error" />{" "}
              <span className="min-w-0 break-words">
                Failed to refresh; the range below is the last successful read of
                this window.
                {query.error instanceof Error ? " " + query.error.message : ""}
              </span>
              <Button
                variant="link"
                size="sm"
                onClick={() => void query.refetch()}
              >
                Try again
              </Button>
            </div>
          )}
          {answer && !range && (
            <p
              className="mt-3 text-sm text-muted-foreground"
              data-slot="investigation-peer-unranged"
            >
              The Server answered this read without a range at all, so no bucket
              of this window can be read from it.
            </p>
          )}
          {range && (
            <>
              <p className="mt-3 text-sm" data-slot="investigation-peer-range">
                {range.returned} of {range.matching} buckets this window covers
                are in this answer,
                {range.missing_buckets === 0
                  ? " and the aligned grid expecting them holds no missing bucket."
                  : " with " +
                    range.missing_buckets +
                    " bucket(s) the grid expected and the Server does not hold."}
              </p>
              <div className="mt-2">
                <Facts>
                  <Fact label="Oldest bucket held">
                    {formatObservedAt(range.first_bucket)}
                  </Fact>
                  <Fact label="Newest bucket held">
                    {formatObservedAt(range.last_bucket)}
                  </Fact>
                  <Fact label="Bucket width">
                    {formatHistoryDuration(range.grain_seconds)}
                  </Fact>
                  <Fact label="Read as">
                    {before === null
                      ? "The newest buckets of this window"
                      : "Buckets older than " + formatObservedAt(before)}
                  </Fact>
                  <Fact label="Recorded collection state">
                    {answer?.state ?? "Unknown"}
                  </Fact>
                  <Fact label="Freshness">
                    {answer?.freshness ?? "Unknown"}
                  </Fact>
                </Facts>
              </div>
              {range.truncated && (
                <p
                  className="mt-2 text-sm text-muted-foreground"
                  role="status"
                  data-slot="investigation-peer-truncated"
                >
                  More buckets of this window are held than this answer carries.
                  Reading the older ones continues from the newest bucket below
                  rather than from this browser's clock, so a bucket is neither
                  skipped nor read twice.
                </p>
              )}
              {range.buckets.length === 0 ? (
                <div className="mt-2" data-slot="investigation-peer-empty">
                  <Empty description="No bucket of this grain covers the stretch being read; the grid above says which buckets were expected." />
                </div>
              ) : (
                <div
                  data-slot="investigation-peer-list"
                  className="mt-2 overflow-x-auto"
                >
                  <table
                    data-stack
                    data-slot="investigation-peer-buckets"
                    className="w-full min-w-[64rem] text-sm"
                  >
                    <caption className="sr-only">
                      Peer receipt buckets of this window, newest first
                    </caption>
                    <thead>
                      <tr className="border-b border-border">
                        <th scope="col" className={TH}>
                          Bucket start (receipt)
                        </th>
                        <th scope="col" className={TH}>
                          Peers
                        </th>
                        <th scope="col" className={TH}>
                          Trusted
                        </th>
                        <th scope="col" className={TH}>
                          Static
                        </th>
                        <th scope="col" className={TH}>
                          Consensus
                        </th>
                        <th scope="col" className={TH}>
                          CFBT lag (blocks)
                        </th>
                        <th scope="col" className={TH}>
                          Arrivals
                        </th>
                        <th scope="col" className={TH}>
                          Departures
                        </th>
                        <th scope="col" className={TH}>
                          Countries
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {range.buckets.map((bucket) => (
                        <tr
                          key={bucket.bucket_start}
                          className="border-b border-border/60 last:border-0"
                        >
                          <td className={TD}>
                            <span className="font-medium">
                              {formatObservedAt(bucket.bucket_start)}
                            </span>
                            <span className="block text-xs text-muted-foreground">
                              Latest observation in the bucket:{" "}
                              {formatObservedAt(bucket.last_observed_at)}
                            </span>
                          </td>
                          <td className={TD}>
                            {bucket.total_peers}
                            <span className="block text-xs text-muted-foreground">
                              {bucket.average_peers == null
                                ? "Average over the bucket: Unknown"
                                : "Average " +
                                  bucket.average_peers.toFixed(1) +
                                  " over " +
                                  bucket.sample_count +
                                  " sample(s)"}
                            </span>
                          </td>
                          <td className={TD}>{bucket.trusted_count}</td>
                          <td className={TD}>{bucket.static_count}</td>
                          <td className={TD}>{bucket.consensus_count}</td>
                          <td className={TD}>
                            {bucket.cbft_lag.average == null
                              ? "Unknown"
                              : bucket.cbft_lag.average.toFixed(1)}
                            <span className="block text-xs text-muted-foreground">
                              {bucket.cbft_lag.minimum == null ||
                              bucket.cbft_lag.maximum == null
                                ? "No peer reported a committed block"
                                : "Between " +
                                  bucket.cbft_lag.minimum +
                                  " and " +
                                  bucket.cbft_lag.maximum +
                                  " over " +
                                  bucket.cbft_lag.sample_count +
                                  " peer(s)"}
                            </span>
                          </td>
                          <td className={TD}>{bucket.arrivals}</td>
                          <td className={TD}>{bucket.departures}</td>
                          <td className={cn(TD, SCOPE)}>
                            {bucket.countries.length === 0
                              ? "No country resolved"
                              : bucket.countries
                                  .slice(0, 3)
                                  .map(
                                    (country) =>
                                      country.country_code +
                                      " " +
                                      country.count,
                                  )
                                  .join(", ") +
                                (bucket.countries.length > 3
                                  ? " +" +
                                    (bucket.countries.length - 3) +
                                    " more"
                                  : "")}
                            <span className="block text-xs text-muted-foreground">
                              {bucket.known_country_count} of{" "}
                              {bucket.total_peers} resolved;{" "}
                              {bucket.unknown_country_count} unknown
                            </span>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
              {range.continuation && (
                <p className="mt-2">
                  <Button
                    variant="outline"
                    className="min-h-11"
                    onClick={() => pick(range.continuation ?? null)}
                  >
                    Load older buckets
                  </Button>
                </p>
              )}
            </>
          )}
        </>
      )}
    </CardX>
  );
}

/**
 * PAGE-ADMIN-NODE-INVESTIGATION: one UTC window over every family of evidence the Server holds.
 *
 * The window is the page's single coordinate and it lives in the URL: the Server resolves it, every
 * card reads the resolved range, and walking into another window keeps the located occurrence so the
 * new window still reads as evidence about the Incident that brought the Operator here.
 */
export default function AdminNodeInvestigation() {
  const { nodeId = "" } = useParams();
  const { generation } = useAuth();
  const [search] = useSearchParams();
  const navigate = useNavigate();
  const link = useMemo(() => readInvestigationLink(search), [search]);
  const query = useAdminNodeInvestigation(generation, nodeId, link.request);
  const answer = query.data;
  const notFound =
    query.isError &&
    query.error instanceof AdminApiError &&
    query.error.code === "not_found";
  const purged =
    query.isError &&
    query.error instanceof AdminApiError &&
    query.error.code === "node_purged";
  const peers = answer?.sources.find((source) => source.key === "peers");
  // The window is the Server's to accept or refuse, so a refusal is shown in the Server's own words
  // beside the controls that ask for another one, never as a window this page substituted for it.
  const refusedWindow =
    query.isError &&
    query.error instanceof AdminApiError &&
    query.error.code === INVALID_WINDOW_CODE
      ? query.error.message
      : null;

  const goToWindow = (request: InvestigationRequest) => {
    navigate({
      search: investigationSearch(request, {
        occurrence: link.occurrence,
        incident: link.incident,
      }),
    });
  };

  const page: PageWindow | null = answer
    ? {
        hours: answer.window.durationSeconds / 3600,
        range: { from: answer.window.from, to: answer.window.to },
        selectRange(nextHours) {
          const preset = answer.window.supportedPresets.find(
            (candidate) => candidate.hours === nextHours,
          );
          if (preset) {
            goToWindow({ window: preset.key });
            return;
          }
          // A width the Server named no preset for is read as a range ending at the answered end, so
          // the new window still shares an edge with the one on screen instead of jumping to now.
          const end = canonicalInstant(answer.window.to);
          const start = canonicalInstant(
            new Date(
              Date.parse(answer.window.to) - nextHours * 3_600_000,
            ).toISOString(),
          );
          if (end === null || start === null) return;
          goToWindow({ from: start, to: end });
        },
        reload() {
          void query.refetch();
        },
      }
    : null;

  // The list applies the same occurrence predicate the Server applied here, so the entry carries the
  // answered window instead of opening on a clock this page invented (issue #220, review finding 8).
  const incidentsHref =
    "/admin/alerts/incidents?subject=node&subject_key=" +
    encodeURIComponent(nodeId) +
    (answer
      ? "&from=" +
        encodeURIComponent(answer.window.from) +
        "&to=" +
        encodeURIComponent(answer.window.to)
      : "");

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className="text-lg font-semibold break-words">
          {answer?.displayName
            ? "Investigation of " + answer.displayName
            : "Node investigation"}
          <span className="mt-0.5 block text-xs font-medium text-muted-foreground break-all">
            {nodeId}
          </span>
        </h1>
        <p className="text-sm text-muted-foreground">
          <Link className={PAGE_LINK} to="/admin/nodes">
            All Nodes
          </Link>{" "}
          ·{" "}
          <Link
            className={PAGE_LINK}
            to={"/admin/nodes/" + encodeURIComponent(nodeId)}
          >
            This Node
          </Link>{" "}
          ·{" "}
          <Link className={PAGE_LINK} to={incidentsHref}>
            {answer
              ? "Incidents for this Node in this window"
              : "Incidents for this Node"}
          </Link>{" "}
          · One window, every family of evidence, read from the Server's own
          records.
        </p>
      </div>
      {!notFound && !purged && (
        <>
          {query.isRefetchError && answer && (
            <div
              className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
              role="alert"
            >
              <StatusBadge status="Error" tone="error" /> Failed to refresh;
              showing the last successful answer, resolved at{" "}
              {formatObservedAt(answer.window.answeredAt)}.
            </div>
          )}
          <WindowControl
            resolved={answer?.window}
            refused={refusedWindow}
            onPreset={(key) => goToWindow({ window: key })}
            onRange={(from, to) => {
              const request: InvestigationRequest = {};
              const start = canonicalInstant(from);
              const end = canonicalInstant(to);
              if (start !== null) request.from = start;
              if (end !== null) request.to = end;
              goToWindow(request);
            }}
            onDefault={() => goToWindow(SERVER_DEFAULT_WINDOW)}
            onReload={() => void query.refetch()}
          />
        </>
      )}
      {!answer && (
        <>
          {query.isPending && (
            <p
              className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground"
              role="status"
            >
              <StatusBadge status="Starting" tone="neutral" /> Reading one
              window of this Node's evidence…
            </p>
          )}
          {query.isError && refusedWindow === null && (
            <div
              className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
              role="alert"
            >
              <StatusBadge status="Error" tone="error" />{" "}
              <span className="min-w-0 break-words">
                {purged
                  ? "This Node was purged, so its evidence was deleted by design and cannot be reconstructed."
                  : notFound
                    ? "This Node is no longer available."
                    : query.error instanceof Error
                      ? query.error.message
                      : "Unable to load the Node investigation"}
              </span>
              {!notFound && !purged && (
                <Button
                  variant="link"
                  size="sm"
                  onClick={() => void query.refetch()}
                >
                  Try again
                </Button>
              )}
            </div>
          )}
        </>
      )}
      {answer && page && (
        <>
          <NodeFactsCard answer={answer} />
          {link.occurrence && (
            <OccurrenceCard
              answer={answer}
              occurrence={link.occurrence}
              incident={link.incident}
            />
          )}
          <CoverageCard answer={answer} />
          {peers && (
            <PeerReceiptPanel
              nodeId={nodeId}
              source={peers}
              from={answer.window.from}
              to={answer.window.to}
            />
          )}
          <NodeMetricEvidencePanel nodeId={nodeId} page={page} />
          <NodeStateEvidencePanel nodeId={nodeId} page={page} />
        </>
      )}
    </section>
  );
}
