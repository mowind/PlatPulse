/**
 * Recorded sync and consensus state history (issue #217, design §11.7).
 *
 * This module is presentation only: it turns the Server's answer into the entries, the silences and
 * the statements the card owes the Operator. Three rules it never breaks. A state that was never
 * observed is never rendered as a value, so an unasked component is not current, not healthy and
 * not zero. A flag is never read as false where the collection failed, because the Server only
 * carries `syncing` where it actually read it. And a retained value keeps the instant it was really
 * observed at, so a last-good state shows its real age instead of a fresh-looking timestamp.
 */
import type {
  AdminStateEntry,
  AdminStateGap,
  AdminStateHistoryResponse,
  AdminStateSeries,
} from './api/generated'
import {
  formatHistoryDuration,
  formatSampleDelay,
  metricAvailabilityNotice,
  metricGapKindLabel,
} from './metricHistory'

/** The two components recorded state history exists for. */
export const STATE_HISTORY_COMPONENTS = ['sync', 'consensus'] as const

export type StateHistoryComponent = (typeof STATE_HISTORY_COMPONENTS)[number]

/** Narrow a Server-provided component back to the switch's two arms. */
export function stateHistoryComponent(value: string): StateHistoryComponent {
  return value === 'consensus' ? 'consensus' : 'sync'
}

export function stateHistoryComponentLabel(component: string): string {
  if (component === 'sync') return 'Sync'
  if (component === 'consensus') return 'Consensus'
  return component
}

/**
 * What one recorded state is evidence of. `retained` is a collection that failed while the Agent
 * still held the value it observed earlier: the state is a failure with an old value behind it, not
 * a fresh reading.
 */
export type StateEvidence =
  | 'current'
  | 'retained'
  | 'failed'
  | 'disabled'
  | 'unsupported'
  | 'starting'
  | 'unknown'

export type StateTone = 'ok' | 'warning' | 'error' | 'neutral'

/**
 * The recording state and the source of the value it carries, judged together: a failure is never
 * dressed as a value, and an unknown pair is unknown rather than healthy.
 */
export function stateEvidence(collectionState: string, valueSource: string): StateEvidence {
  switch (collectionState) {
    case 'ok':
      if (valueSource === 'current') return 'current'
      if (valueSource === 'last_good') return 'retained'
      return 'unknown'
    case 'error':
      // A failed collection that still carries the Agent's retained reading is a failure with an
      // old value behind it, not a fresh reading and not a state without one.
      return valueSource === 'last_good' ? 'retained' : 'failed'
    case 'disabled':
      return 'disabled'
    case 'unsupported':
      return 'unsupported'
    case 'starting':
      return 'starting'
    default:
      return 'unknown'
  }
}

export function stateEntryEvidence(entry: AdminStateEntry): StateEvidence {
  return stateEvidence(entry.collectionState, entry.valueSource)
}

const STATE_EVIDENCE_PRESENTATION: Record<StateEvidence, { label: string; tone: StateTone }> = {
  current: { label: 'Current', tone: 'ok' },
  retained: { label: 'Last good', tone: 'warning' },
  failed: { label: 'Failed', tone: 'error' },
  disabled: { label: 'Not collected', tone: 'neutral' },
  unsupported: { label: 'Unsupported', tone: 'neutral' },
  starting: { label: 'Starting', tone: 'neutral' },
  unknown: { label: 'Unknown', tone: 'neutral' },
}

export function stateEvidenceLabel(evidence: StateEvidence): string {
  return STATE_EVIDENCE_PRESENTATION[evidence].label
}

export function stateEvidenceTone(evidence: StateEvidence): StateTone {
  return STATE_EVIDENCE_PRESENTATION[evidence].tone
}

/** How the collection itself went, in the Server's own vocabulary. */
export function stateCollectionLabel(collectionState: string): string {
  switch (collectionState) {
    case 'ok':
      return 'Collection succeeded'
    case 'error':
      return 'Collection failed'
    case 'disabled':
      return 'Collection is off'
    case 'unsupported':
      return 'Collection is unsupported'
    case 'starting':
      return 'Collection has just started'
    default:
      return 'Collection state ' + collectionState
  }
}

export function stateValueSourceLabel(valueSource: string): string {
  switch (valueSource) {
    case 'current':
      return 'Value observed now'
    case 'last_good':
      return 'Retained last good value'
    case 'none':
      return 'No value'
    default:
      return 'Value source ' + valueSource
  }
}

export function stateEntryKindLabel(entryKind: string): string {
  if (entryKind === 'anchor') return 'Anchor'
  if (entryKind === 'change') return 'Change'
  return entryKind
}

/** The flag as it was actually read: absent means unknown, never false. */
export function formatStateSyncing(entry: AdminStateEntry): string {
  if (entry.syncing === true) return 'Syncing'
  if (entry.syncing === false) return 'Not syncing'
  return 'Unknown'
}

export function stateHistorySyncingClaim(entry: AdminStateEntry): string {
  if (entry.syncing === true) return 'The Node reported that it was syncing when this state was recorded.'
  if (entry.syncing === false) {
    return 'The Node reported that it was not syncing when this state was recorded.'
  }
  return 'Whether the Node was syncing is unknown: the collection did not succeed, so no flag was read and none is claimed.'
}

/** Seconds between the instant of the value and the instant of the state, or null for no value. */
export function stateValueAgeSeconds(entry: AdminStateEntry): number | null {
  if (!entry.valueObservedAt) return null
  const observed = Date.parse(entry.observedAt)
  const value = Date.parse(entry.valueObservedAt)
  if (!Number.isFinite(observed) || !Number.isFinite(value)) return null
  return Math.max(0, Math.round((observed - value) / 1000))
}

export function formatStateValueAge(entry: AdminStateEntry): string {
  if (entry.valueSource === 'none' && !entry.valueObservedAt) {
    return 'This state carries no value: the Server holds no observation to show for it.'
  }
  const age = stateValueAgeSeconds(entry)
  if (age === null) return 'The instant of this value is not recorded, so its age is unknown.'
  if (age === 0) return 'The value was observed at the same instant as this state.'
  return (
    'The value this state refers to was ' + formatHistoryDuration(age) + ' old when the state was recorded.'
  )
}

export type StateHistoryEntryRow = {
  observedAt: string
  receivedAt: string
  entryKind: string
  entryKindLabel: string
  anchor: boolean
  evidence: StateEvidence
  evidenceLabel: string
  tone: StateTone
  collection: string
  valueSource: string
  valueSourceLabel: string
  valueAge: string
  syncing: string
  syncingClaim: string
  errorCode: string | null
  delay: string
  clockSuspect: boolean
  clockNote: string | null
}

export function stateHistoryEntrySummary(entry: AdminStateEntry): StateHistoryEntryRow {
  const evidence = stateEntryEvidence(entry)
  return {
    observedAt: entry.observedAt,
    receivedAt: entry.receivedAt,
    entryKind: entry.entryKind,
    entryKindLabel: stateEntryKindLabel(entry.entryKind),
    anchor: entry.entryKind === 'anchor',
    evidence,
    evidenceLabel: stateEvidenceLabel(evidence),
    tone: stateEvidenceTone(evidence),
    collection: stateCollectionLabel(entry.collectionState),
    valueSource: entry.valueSource,
    valueSourceLabel: stateValueSourceLabel(entry.valueSource),
    valueAge: formatStateValueAge(entry),
    syncing: formatStateSyncing(entry),
    syncingClaim: stateHistorySyncingClaim(entry),
    errorCode: entry.errorCode ?? null,
    delay: formatSampleDelay(entry.delaySeconds),
    clockSuspect: entry.clockSuspect === true,
    clockNote: entry.clockNote ?? null,
  }
}

export type StateHistoryGapRow = {
  from: string
  to: string
  seconds: number
  duration: string
  kind: string
  kindLabel: string
  pause: boolean
  reason: string
  skippedCount: number | null
  claim: string
}

export function stateHistoryGapSummary(gap: AdminStateGap): StateHistoryGapRow {
  const pause = gap.kind === 'protection_pause'
  const skippedCount = gap.skippedCount ?? null
  const skipped =
    skippedCount === null
      ? ''
      : ' ' + skippedCount + (skippedCount === 1 ? ' observation was' : ' observations were') +
        ' skipped behind it.'
  const claim = pause
    ? 'The operator paused history collection across this stretch, so the record shows no state here: ' +
      'a paused stretch is neither a state nor a change.' +
      skipped
    : 'Nobody observed this Node across this stretch, so nothing is claimed about its state in it.' +
      skipped
  return {
    from: gap.from,
    to: gap.to,
    seconds: gap.seconds,
    duration: formatHistoryDuration(gap.seconds),
    kind: gap.kind,
    kindLabel: metricGapKindLabel(gap.kind),
    pause,
    reason: gap.reason,
    skippedCount,
    claim,
  }
}

/** One sentence for every paused stretch in the answer, or null when nothing was paused. */
export function stateHistoryPauseNotice(pauses: StateHistoryGapRow[]): string | null {
  if (pauses.length === 0) return null
  const skipped = pauses.reduce((total, pause) => total + (pause.skippedCount ?? 0), 0)
  const stretches = pauses.length === 1 ? '1 stretch' : pauses.length + ' stretches'
  const lost = skipped > 0 ? ' and ' + skipped + ' skipped observation(s) behind them' : ''
  return (
    'This window holds ' + stretches + ' of paused history collection' + lost +
    ': a paused stretch is not a state of the Node and not a change of one.'
  )
}

/** Why an unchanged state is still provable. */
export function stateHistoryAnchorNotice(anchorSeconds: number): string {
  if (!(anchorSeconds > 0)) {
    return 'The Server does not record anchors for this family, so an unchanged state is not re-recorded on its own.'
  }
  return (
    'An unchanged state is recorded again at most once every ' + formatHistoryDuration(anchorSeconds) +
    ', which is what keeps a constant state provable: an anchor row proves the Node was still reported on, ' +
    'so a stretch without an anchor is a missing report rather than a quiet Node.'
  )
}

export function stateHistoryCadenceNotice(cadenceSeconds: number): string {
  if (!(cadenceSeconds > 0)) {
    return 'The reporting cadence of this Node is not known, so no silence below is measured against one.'
  }
  return (
    'This Node reported every ' + formatHistoryDuration(cadenceSeconds) +
    '; the silences below are what the record holds, not what a cadence was assumed to allow.'
  )
}

/**
 * How much of the window the record proves. Coverage counts the stretch between two recorded states
 * only, so silence after the newest state is never claimed as covered.
 */
export function stateHistoryCoverageNotice(coverageSeconds: number, windowSeconds: number): string {
  const window = windowSeconds > 0 ? formatHistoryDuration(windowSeconds) : 'this window'
  if (!(coverageSeconds > 0)) {
    return (
      'No stretch between two recorded states is proven inside ' + window +
      ', so none of it is covered: nothing after the newest state is claimed, and the record alone ' +
      'cannot say whether the Node kept reporting.'
    )
  }
  return (
    'The record proves ' + formatHistoryDuration(coverageSeconds) + ' of ' + window +
    ': only the stretch between two recorded states is covered, and the time after the newest state is ' +
    'left to the reader instead of being claimed.'
  )
}

/** A component the Server never recorded a state for is absent, never healthy. */
export function stateHistoryUnaskedNotice(component: string): string {
  return (
    'This Node has no recorded ' + stateHistoryComponentLabel(component) +
    ' state in any retained window, so the card shows no state at all: a component that was never ' +
    'observed is not current, not healthy and not zero.'
  )
}

/** Age the newest recorded state against the end of the window. */
export function stateHistoryNewestNotice(
  series: AdminStateSeries,
  windowEnd: string,
): string | null {
  const newest = series.lastObservedAt ?? null
  if (!newest) return null
  const newestAt = Date.parse(newest)
  const endAt = Date.parse(windowEnd)
  if (!Number.isFinite(newestAt) || !Number.isFinite(endAt)) return null
  const age = Math.round((endAt - newestAt) / 1000)
  const tail =
    'silence after the newest state is not reported as a gap, so nothing past it is claimed.'
  if (age <= 0) {
    return 'The newest recorded state sits at the end of this window (' + newest + '); ' + tail
  }
  return (
    'The newest recorded state is ' + formatHistoryDuration(age) + ' older than the end of this window (' +
    newest + '); ' + tail
  )
}

/** The newest counted delivery: the Server's last word even where its row was released. */
export function stateHistoryLatestNotice(series: AdminStateSeries): string | null {
  const collectionState = series.latestCollectionState ?? null
  if (!collectionState) return null
  const valueSource = series.latestValueSource ?? null
  let sentence =
    'The newest counted delivery the Server holds recorded ' + stateCollectionLabel(collectionState).toLowerCase()
  if (valueSource) {
    sentence += ' with ' + stateValueSourceLabel(valueSource).toLowerCase()
  }
  const source = series.latestValueObservedAt ?? null
  sentence += source ? ' observed at ' + source : ''
  if (series.latestErrorCode) sentence += ' and the failure ' + series.latestErrorCode
  if (series.latestSyncing === true) sentence += ', and the Node was syncing'
  if (series.latestSyncing === false) sentence += ', and the Node was not syncing'
  return sentence + '.'
}

/** A truncated answer reports the newer entries and says that older ones were left out. */
export function stateHistoryTruncationNotice(
  component: string,
  entriesReturned: number,
): string {
  const shown = entriesReturned === 1 ? '1 recorded state' : entriesReturned + ' recorded states'
  return (
    'This window holds more recorded ' + stateHistoryComponentLabel(component).toLowerCase() +
    ' state than this answer carries: the newest ' + shown +
    ' are shown, and the older ones are reported here rather than dropped silently.'
  )
}

/** An answer that starts later than the range that was asked for is a page, and says so. */
export function stateHistoryPagingNotice(from: string, requestedFrom: string): string | null {
  if (!requestedFrom || from === requestedFrom) return null
  return (
    'This answer starts at ' + from + ' rather than at the ' + requestedFrom +
    ' that was asked for: it is the older page the cursor points at, and the newer page is not repeated here.'
  )
}

export function stateHistoryOrder(): string {
  return 'Oldest recorded state first, in the order the Server answered them.'
}

export const STATE_HISTORY_CHART_WIDTH = 600
export const STATE_HISTORY_CHART_TOP = 8
export const STATE_HISTORY_CHART_BOTTOM = 142

export type StateHistoryBand = { x: number; width: number }

export type StateHistoryMarker = {
  x: number
  observedAt: string
  entryKind: string
  anchor: boolean
  evidence: StateEvidence
  evidenceLabel: string
  tone: StateTone
  label: string
  title: string
}

export type StateHistoryChartGeometry = {
  width: number
  top: number
  bottom: number
  baseline: number
  markerY: number
  markerHeight: number
  gapY: number
  gapHeight: number
  provenY: number
  provenHeight: number
  markers: StateHistoryMarker[]
  gapBands: StateHistoryBand[]
  provenBands: StateHistoryBand[]
}

/** An instant mapped into the chart's own coordinates, clamped to its span. */
function chartX(instant: number, from: number, to: number): number {
  if (!Number.isFinite(instant) || to <= from) return 0
  const ratio = (instant - from) / (to - from)
  return Math.max(0, Math.min(STATE_HISTORY_CHART_WIDTH, ratio * STATE_HISTORY_CHART_WIDTH))
}

/**
 * The timeline's geometry: one marker per recorded state, one band per silence, and the stretches
 * two adjacent states prove. A stretch the Server reported as a gap is never drawn as proven.
 */
export function stateHistoryChartGeometry(
  entries: AdminStateEntry[],
  gaps: AdminStateGap[],
  from: string,
  to: string,
): StateHistoryChartGeometry {
  const fromAt = Date.parse(from)
  const toAt = Date.parse(to)
  const markers: StateHistoryMarker[] = []
  for (const entry of entries) {
    const at = Date.parse(entry.observedAt)
    if (!Number.isFinite(at)) continue
    const evidence = stateEntryEvidence(entry)
    markers.push({
      x: chartX(at, fromAt, toAt),
      observedAt: entry.observedAt,
      entryKind: entry.entryKind,
      anchor: entry.entryKind === 'anchor',
      evidence,
      evidenceLabel: stateEvidenceLabel(evidence),
      tone: stateEvidenceTone(evidence),
      label: stateEntryKindLabel(entry.entryKind),
      title:
        stateEntryKindLabel(entry.entryKind) + ' at ' + entry.observedAt + ': ' +
        stateEvidenceLabel(evidence),
    })
  }
  const gapBands: StateHistoryBand[] = []
  for (const gap of gaps) {
    const start = chartX(Date.parse(gap.from), fromAt, toAt)
    const end = chartX(Date.parse(gap.to), fromAt, toAt)
    if (end - start > 0) gapBands.push({ x: start, width: end - start })
  }
  const provenBands: StateHistoryBand[] = []
  const parsed = entries
    .map((entry) => Date.parse(entry.observedAt))
    .filter((at) => Number.isFinite(at))
  for (let index = 1; index < parsed.length; index += 1) {
    const previous = parsed[index - 1]
    const next = parsed[index]
    if (next - previous <= 0) continue
    const silenced = gaps.some(
      (gap) => Date.parse(gap.to) > previous && Date.parse(gap.from) < next,
    )
    if (silenced) continue
    const start = chartX(previous, fromAt, toAt)
    const end = chartX(next, fromAt, toAt)
    if (end - start > 0) provenBands.push({ x: start, width: end - start })
  }
  return {
    width: STATE_HISTORY_CHART_WIDTH,
    top: STATE_HISTORY_CHART_TOP,
    bottom: STATE_HISTORY_CHART_BOTTOM,
    baseline: 130,
    markerY: 36,
    markerHeight: 76,
    gapY: 112,
    gapHeight: 18,
    provenY: 118,
    provenHeight: 6,
    markers,
    gapBands,
    provenBands,
  }
}

export type StateHistoryView = {
  component: string
  componentLabel: string
  observed: boolean
  entries: StateHistoryEntryRow[]
  gaps: StateHistoryGapRow[]
  pauses: StateHistoryGapRow[]
  pause: string | null
  entryCount: number
  changeCount: number
  anchorCount: number
  replayedCount: number
  correctedCount: number
  entriesReturned: number
  firstObservedAt: string | null
  lastObservedAt: string | null
  lastReceivedAt: string | null
  releasedBefore: string | null
  latest: string | null
  anchorSeconds: number
  anchor: string
  cadenceSeconds: number
  cadence: string
  coverageSeconds: number
  coverage: string
  windowSeconds: number
  retentionDays: number
  availability: string | null
  availabilityNotice: string | null
  truncated: boolean
  truncation: string | null
  continuation: string | null
  paging: string | null
  unasked: string | null
  newest: string | null
  order: string
  chart: StateHistoryChartGeometry
}

/** Everything the card renders, derived from the Server's answer alone. */
export function stateHistoryView(answer: AdminStateHistoryResponse): StateHistoryView {
  const series = answer.series
  const entries = answer.entries.map(stateHistoryEntrySummary)
  const gaps = answer.gaps.map(stateHistoryGapSummary)
  const pauses = gaps.filter((gap) => gap.pause)
  const observed = series.observed === true
  return {
    component: answer.component,
    componentLabel: stateHistoryComponentLabel(answer.component),
    observed,
    entries,
    gaps,
    pauses,
    pause: stateHistoryPauseNotice(pauses),
    entryCount: series.entryCount ?? 0,
    changeCount: series.changeCount ?? 0,
    anchorCount: series.anchorCount ?? 0,
    replayedCount: series.replayedCount ?? 0,
    correctedCount: series.correctedCount ?? 0,
    entriesReturned: series.entriesReturned ?? entries.length,
    firstObservedAt: series.firstObservedAt ?? null,
    lastObservedAt: series.lastObservedAt ?? null,
    lastReceivedAt: series.lastReceivedAt ?? null,
    releasedBefore: series.releasedBefore ?? null,
    latest: stateHistoryLatestNotice(series),
    anchorSeconds: answer.anchorSeconds ?? 0,
    anchor: stateHistoryAnchorNotice(answer.anchorSeconds ?? 0),
    cadenceSeconds: answer.cadenceSeconds ?? 0,
    cadence: stateHistoryCadenceNotice(answer.cadenceSeconds ?? 0),
    coverageSeconds: answer.coverageSeconds ?? 0,
    coverage: stateHistoryCoverageNotice(answer.coverageSeconds ?? 0, answer.windowSeconds ?? 0),
    windowSeconds: answer.windowSeconds ?? 0,
    retentionDays: answer.retentionDays ?? 0,
    availability: answer.availability ?? null,
    availabilityNotice: metricAvailabilityNotice(answer.availability ?? null),
    truncated: answer.truncated === true,
    truncation: answer.truncated
      ? stateHistoryTruncationNotice(answer.component, entries.length)
      : null,
    continuation: answer.continuation ?? null,
    paging: stateHistoryPagingNotice(answer.from, answer.requestedFrom),
    unasked: !observed && entries.length === 0 ? stateHistoryUnaskedNotice(answer.component) : null,
    newest: stateHistoryNewestNotice(series, answer.to),
    order: stateHistoryOrder(),
    chart: stateHistoryChartGeometry(answer.entries, answer.gaps, answer.from, answer.to),
  }
}
