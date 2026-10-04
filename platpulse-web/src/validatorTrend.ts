/**
 * Owner Validator daily-trend presentation (issue #219, design §15.4;
 * webui.md §15.21).
 *
 * Presentation only: it turns the Server's bounded window into the statements
 * and the drawing the Owner surface owes the Operator. Five rules it never
 * breaks, and why each one exists:
 *
 * - The window belongs to the configured calendar, not to UTC. Every local
 *   date, day boundary and month boundary an answer carries was formed in the
 *   Server's configured IANA zone, so a day is drawn as the stretch of UTC it
 *   really covers — a daylight-saving day is 23 or 25 hours wide, never a
 *   pretended 24 — and the boundaries are printed beside it.
 * - A configured local day the answer proves holds no snapshot is a silence,
 *   never a zero: the plot breaks there and the silence is listed.
 * - A metric a stored row never carried (rank, delegator count, delay) is
 *   Unknown; a delay that cannot be measured is null rather than 0.
 * - Reward and block values are cumulative Provider counters as of each sample;
 *   stake is the balance the Provider reported at that sample. Each is labelled
 *   as what it is and is never differenced into period earnings, net profit, or
 *   a silently re-bucketed series.
 * - Coverage is disclosed rather than assumed: days requested, answered,
 *   expected, observed and missing, plus every day the answer did not reach
 *   (a clamped window, a truncated page, an exclusive cursor) are all said out
 *   loud.
 *
 * The plot reads the stake as a number to place it on the axis; the exact
 * decimal string of every day stays in the table beside the plot.
 */
import type {
  AdminValidatorTrendGap,
  AdminValidatorTrendMonth,
  AdminValidatorTrendPoint,
  AdminValidatorTrendResponse,
} from './api/generated'
import { formatObservedAt } from './components/StatusBadge'
import { formatDuration } from './formatDuration'
import type { ValidatorTone } from './validators'

/** Canonical second-precision RFC 3339, matching the Server's format. */
export function formatTrendInstant(date: Date): string {
  return date.toISOString().replace(/\.\d{3}Z$/, 'Z')
}

/** The widest page the trend surface asks for in a single request. A page is one
 *  local day wider than the elapsed days it covers because the window is asked
 *  in instants while the Server answers whole configured local dates: a window
 *  that begins and ends inside two local days touches one day more than the
 *  elapsed days, and a page that did not carry it would report a truncation
 *  nobody asked for. */
export const VALIDATOR_TREND_PAGE_DAYS = 31

/** A range preset the Owner can pick. */
export interface ValidatorTrendPreset {
  label: string
  days: number
  limit: number
}

/**
 * A short window is answered in a single page; a long window is answered as
 * bounded pages the Owner walks with the older-days control, so a longer
 * preset never turns into a longer response (design §15.21).
 */
export const VALIDATOR_TREND_PRESETS: readonly ValidatorTrendPreset[] = [
  { label: '7 days', days: 7, limit: 8 },
  { label: '30 days', days: 30, limit: 31 },
  { label: '90 days', days: 90, limit: VALIDATOR_TREND_PAGE_DAYS },
  { label: '180 days', days: 180, limit: VALIDATOR_TREND_PAGE_DAYS },
  { label: '365 days', days: 365, limit: VALIDATOR_TREND_PAGE_DAYS },
]

/** The requested window for a preset, computed once per selection so the query
 *  key stays stable while the page is open. */
export function validatorTrendRange(days: number, now: Date): { from: string; to: string } {
  const safeDays = Math.max(1, Math.round(days))
  return {
    from: formatTrendInstant(new Date(now.getTime() - safeDays * 86400_000)),
    to: formatTrendInstant(now),
  }
}

/** The three daily series the trend answers. Rank is a position, so it is named
 *  as one and never as a score. */
export type ValidatorTrendSeries = 'rank' | 'stake' | 'delegators'

export const VALIDATOR_TREND_SERIES: readonly {
  value: ValidatorTrendSeries
  label: string
  unit: string
}[] = [
  { value: 'rank', label: 'Rank', unit: 'position (lower is better)' },
  { value: 'stake', label: 'Stake', unit: 'base units, exact value in the table' },
  { value: 'delegators', label: 'Delegators', unit: 'count' },
]

/** One day's value for one series, or null when the stored row never carried
 *  it: an unknown value is never read as a zero. */
export function validatorTrendValue(
  point: AdminValidatorTrendPoint,
  series: ValidatorTrendSeries,
): number | null {
  if (series === 'rank') return point.rank ?? null
  if (series === 'delegators') return point.delegatorCount ?? null
  const raw = point.stakeAmount
  if (typeof raw !== 'string' || raw.trim().length === 0) return null
  const value = Number(raw)
  return Number.isFinite(value) ? value : null
}

export type ValidatorTrendNotice = {
  label: string
  tone: ValidatorTone
  description: string
}

const COVERAGE: Record<string, ValidatorTrendNotice> = {
  complete: {
    label: 'Complete',
    tone: 'ok',
    description: 'Every configured local day in the answered stretch carries a stored snapshot.',
  },
  unavailable: {
    label: 'Unavailable',
    tone: 'warning',
    description:
      'The answered stretch holds configured local days, but no snapshot was stored for any of them. That is missing evidence, not a zero and not a healthy stretch.',
  },
  empty: {
    label: 'Empty',
    tone: 'neutral',
    description:
      'The requested stretch holds no configured local day to answer, so nothing is missing and nothing is claimed.',
  },
}

/** The coverage verdict this answer carries on its own, in the Operator's
 *  words. A verdict the Server did not send stays Unknown rather than being
 *  read as complete. */
export function validatorTrendCoverage(
  page: AdminValidatorTrendResponse | undefined,
): ValidatorTrendNotice {
  if (!page) return { label: 'Unknown', tone: 'neutral', description: 'No answer yet.' }
  if ((page.coverage ?? '').trim().toLowerCase() === 'partial') return partialNotice(page)
  const known = COVERAGE[(page.coverage ?? '').trim().toLowerCase()]
  if (known) return known
  return {
    label: 'Unknown',
    tone: 'neutral',
    description: 'The Server reported a coverage verdict this surface does not know.',
  }
}

/** Partial has two honest readings and the answer's own numbers choose which
 *  one is said: a proven silence inside the answered stretch, or an answered
 *  stretch that is itself short of the requested window — a truncated page or a
 *  clamped one. Claiming missing days the answer never proved would report a
 *  silence that is only ever an unanswered window. */
function partialNotice(page: AdminValidatorTrendResponse): ValidatorTrendNotice {
  if (typeof page.missingDays === 'number' && page.missingDays > 0) {
    return {
      label: 'Partial',
      tone: 'warning',
      description:
        'Some configured local days in the answered stretch carry no stored snapshot. The days that do are shown; the missing ones are listed as silences, never as zeros.',
    }
  }
  return {
    label: 'Partial',
    tone: 'warning',
    description:
      'Every configured local day in the answered stretch carries a stored snapshot, but the answered stretch is not the whole requested window: the rest of the window is unanswered rather than silent.',
  }
}

/** Whether the answered stretch is the requested one, so a narrowed page never
 *  reads as the whole window. */
export function validatorTrendStretchNotice(
  page: AdminValidatorTrendResponse | undefined,
): string | null {
  if (!page) return null
  const requested = page.requestedFromLocalDate + ' to ' + page.requestedToLocalDate
  const answered = page.answeredFromLocalDate + ' to ' + page.answeredToLocalDate
  const clamped = page.clamped
    ? ' The request was wider than the bounded maximum window, so the Server narrowed it to a stretch of ' +
      page.expectedDays +
      ' configured local days; the oldest requested days are not answered.'
    : ''
  if (page.answeredFromLocalDate === page.requestedFromLocalDate) {
    if (!clamped) return null
    return 'This answer covers ' + answered + ' of the requested ' + requested + '.' + clamped
  }
  return (
    'This page answers ' +
    answered +
    ' of the requested ' +
    requested +
    '; the days it does not reach are not silences in this answer.' +
    clamped
  )
}

/** What a truncated page did not answer, and how the next page continues
 *  strictly older without repeating a day. */
export function validatorTrendTruncationNotice(
  page: AdminValidatorTrendResponse | undefined,
): string | null {
  if (!page?.truncated) return null
  const continuation = page.continuation
  return (
    'The window holds more configured local days than this page answered (' +
    page.expectedDays +
    ' days shown). Only the newest part is here' +
    (continuation
      ? ', and the next page continues strictly older than ' + continuation + '; no day is answered twice.'
      : '.') +
    ' The days this page does not reach are not counted as silences here.'
  )
}

/** Stored rows formed in another timezone: disclosed, never merged into the
 *  configured calendar and never silently re-bucketed from UTC. */
export function validatorTrendForeignNotice(
  page: AdminValidatorTrendResponse | undefined,
): string | null {
  if (!page || page.foreignRows <= 0) return null
  const zones = page.foreignTimezones.length > 0 ? page.foreignTimezones.join(', ') : 'unknown zones'
  return (
    page.foreignRows +
    ' stored snapshot(s) for this Validator inside this stretch were formed in another configured timezone (' +
    zones +
    '). They are disclosed here and are never merged into the configured calendar or quietly re-bucketed.'
  )
}

/** Story 68: Purge removes a Node's own association intervals together with the
 *  Node, so those intervals read as unavailable rather than as never having
 *  existed — while the Validator's retained snapshot days keep answering. */
export function validatorTrendAssociationNotice(
  page: AdminValidatorTrendResponse | undefined,
): string | null {
  if (!page) return null
  const parts: string[] = []
  if (page.associationHistoryPartial && page.deletedNodes > 0) {
    parts.push(
      page.deletedNodes +
        ' Node(s) of this Network were deleted by Purge. A purge removes that Node\'s association intervals with the Node, so those intervals are unavailable here rather than never having existed; they are never re-attached to a surviving Node, and this Validator\'s own recorded days above are retained.',
    )
  }
  if (page.associationsTruncated) {
    parts.push('More association intervals exist than are listed here; only the newest are shown.')
  }
  return parts.length > 0 ? parts.join(' ') : null
}

/** How to read the counters: cumulative Provider counters, said in the words
 *  that keep them from being read as period earnings. */
export function validatorTrendCounterNotice(
  page: AdminValidatorTrendResponse | undefined,
): string {
  const semantics = (page?.counterSemantics ?? '').trim().toLowerCase()
  if (semantics === 'cumulative') {
    return 'Reward and block values are cumulative Provider counters as of each sample; stake is the balance the Provider reported at that sample. This surface never computes period earnings, net profit, or a re-bucketed series from them.'
  }
  if (semantics.length === 0) {
    return 'The Server did not state what these counters mean, so no reading is claimed.'
  }
  return 'The Server states these counters mean "' + page?.counterSemantics + '".'
}

/** One proven silence, in the Operator's words. */
export function validatorTrendGapNotice(gap: AdminValidatorTrendGap): string {
  if (gap.days === 1) {
    return 'No snapshot was stored for ' + gap.fromLocalDate + '. It is a silence, not a zero.'
  }
  return (
    'No snapshot was stored for ' +
    gap.days +
    ' configured local days, ' +
    gap.fromLocalDate +
    ' to ' +
    gap.toLocalDate +
    '. They are a silence, not a zero.'
  )
}

/** Which timestamp chose the day: the Provider's own stamp, or the Server
 *  receipt that stood in for it when the observation carried none. */
export function validatorTrendSampleTimeLabel(sampleTime: string | null | undefined): string {
  const value = (sampleTime ?? '').trim().toLowerCase()
  if (value === 'provider') return 'Provider timestamp'
  if (value === 'receipt') return 'Server receipt (fallback)'
  return 'Unknown'
}

/** One labelled instant of a stored day. */
export type ValidatorTrendTimestamp = { label: string; value: string }

/** The three instants a stored day carries, each named: the instant that chose
 *  the day, the Provider's own timestamp, and the Server receipt. A day whose
 *  sample time came from the Provider is therefore never shown without the
 *  receipt it was measured against, and a Provider timestamp the observation
 *  never carried reads Unknown rather than borrowing the receipt (#219). */
export function validatorTrendTimestamps(
  point: AdminValidatorTrendPoint,
): ValidatorTrendTimestamp[] {
  return [
    { label: 'Sample', value: formatObservedAt(point.sampleAt) },
    {
      label: 'Provider',
      value: point.providerTimestamp ? formatObservedAt(point.providerTimestamp) : 'Unknown',
    },
    { label: 'Receipt', value: formatObservedAt(point.receivedAt) },
  ]
}

/** The delay between the Provider's stamp and the Server receipt. An unknown
 *  delay reads Unknown; it is never a zero, and a negative one says which clock
 *  is ahead instead of hiding it. */
export function validatorTrendDelay(
  delaySeconds: number | null | undefined,
  clockSuspect: boolean,
): string {
  if (delaySeconds == null || !Number.isFinite(delaySeconds)) return 'Unknown'
  if (delaySeconds < 0) {
    return (
      formatDuration(Math.abs(delaySeconds) * 1000) +
      (clockSuspect ? ' behind (Provider clock ahead)' : ' behind')
    )
  }
  return formatDuration(delaySeconds * 1000)
}

export type ValidatorTrendDayWindow = {
  from: string
  to: string
  /** How wide the configured local day really is: 23 or 25 hours on a
   *  daylight-saving day, and null when the boundaries cannot be read. */
  hours: number | null
}

/** The UTC stretch one configured local day really covers, with its real
 *  width, so a 23 or 25 hour day is stated instead of assumed. */
export function validatorTrendDayWindow(point: AdminValidatorTrendPoint): ValidatorTrendDayWindow {
  const from = Date.parse(point.dayStart)
  const to = Date.parse(point.dayEnd)
  const hours =
    Number.isFinite(from) && Number.isFinite(to) && to > from
      ? Math.round(((to - from) / 3600_000) * 100) / 100
      : null
  return { from: point.dayStart, to: point.dayEnd, hours }
}

/** A configured calendar month's boundary, mapped into UTC by the Server. */
export function validatorTrendMonthWindow(month: AdminValidatorTrendMonth): string {
  return month.monthStart + ' → ' + month.monthEnd
}

/** The ordinal of a configured local date, for placing it on the day axis. A
 *  value that is not a real local date has no ordinal, so nothing is drawn at
 *  an invented position. */
export function validatorTrendDayOrdinal(localDate: string): number | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(localDate)
  if (!match) return null
  const year = Number(match[1])
  const month = Number(match[2])
  const day = Number(match[3])
  const time = Date.UTC(year, month - 1, day)
  const date = new Date(time)
  if (
    date.getUTCFullYear() !== year ||
    date.getUTCMonth() !== month - 1 ||
    date.getUTCDate() !== day
  ) {
    return null
  }
  return Math.round(time / 86400_000)
}

const CHART_WIDTH = 600
const CHART_TOP = 8
const CHART_BOTTOM = 142
const CHART_SPAN = CHART_BOTTOM - CHART_TOP
const CHART_PADDING = 8

export type ValidatorTrendColumn = {
  localDate: string
  value: number
  x: number
  y: number
}

export type ValidatorTrendChart = {
  series: ValidatorTrendSeries
  width: number
  top: number
  bottom: number
  /** One column per stored day that carries a value for this series. */
  columns: ValidatorTrendColumn[]
  /** The runs the line is honestly drawn as: a day with no snapshot, and a day
   *  whose row never carried this metric, both break the line. */
  segments: ValidatorTrendColumn[][]
  /** The stretches this answer proved hold no snapshot, drawn as silence. */
  gapBands: { key: string; x: number; width: number }[]
  min: number
  max: number
  /** Days of the answered stretch with no stored row at all. */
  unknownDays: number
  /** Stored days whose row carries no value for this series. */
  unknownValues: number
}

/**
 * The drawing of one series over the answered stretch. The day axis is the
 * configured calendar the answer was formed in: a column sits where its local
 * date really falls, so a silence and a 23 hour day keep their real width.
 */
export function validatorTrendChartGeometry(
  page: AdminValidatorTrendResponse,
  series: ValidatorTrendSeries,
  width: number = CHART_WIDTH,
): ValidatorTrendChart {
  const left = CHART_PADDING
  const right = Math.max(left + 1, width - CHART_PADDING)
  const first = validatorTrendDayOrdinal(page.answeredFromLocalDate)
  const last = validatorTrendDayOrdinal(page.answeredToLocalDate)
  const spanDays = first != null && last != null ? Math.max(1, last - first + 1) : 0
  const step = spanDays > 1 ? (right - left) / (spanDays - 1) : 0
  const place = (ordinal: number): number =>
    spanDays > 1 ? left + (ordinal - first!) * step : (left + right) / 2
  const columns: ValidatorTrendColumn[] = []
  let unknownValues = 0
  for (const point of page.points) {
    const ordinal = validatorTrendDayOrdinal(point.localDate)
    const value = validatorTrendValue(point, series)
    if (value == null) unknownValues += 1
    if (ordinal == null || value == null) continue
    columns.push({ localDate: point.localDate, value, x: place(ordinal), y: 0 })
  }
  const values = columns.map((column) => column.value)
  const lowest = values.length > 0 ? Math.min(...values) : 0
  const highest = values.length > 0 ? Math.max(...values) : 0
  const pad = highest === lowest ? Math.max(1, Math.abs(highest) * 0.1) : 0
  const min = lowest - pad
  const max = highest + pad
  const scale = max - min === 0 ? 1 : max - min
  const placed = columns.map((column) => ({
    ...column,
    y: CHART_BOTTOM - ((column.value - min) / scale) * CHART_SPAN,
  }))
  const segments: ValidatorTrendColumn[][] = []
  let current: ValidatorTrendColumn[] = []
  let previous: number | null = null
  for (const column of placed) {
    const ordinal = validatorTrendDayOrdinal(column.localDate)
    if (ordinal == null || (previous != null && ordinal !== previous + 1)) {
      if (current.length > 0) segments.push(current)
      current = []
    }
    current.push(column)
    previous = ordinal
  }
  if (current.length > 0) segments.push(current)
  const gapBands = page.gaps
    .map((gap) => {
      const from = validatorTrendDayOrdinal(gap.fromLocalDate)
      const to = validatorTrendDayOrdinal(gap.toLocalDate)
      if (from == null || to == null) return null
      const startX = place(Math.max(first ?? from, from))
      const endX = place(Math.min(last ?? to, to))
      const bandWidth = Math.max(step, Math.abs(endX - startX))
      return {
        key: gap.fromLocalDate + '..' + gap.toLocalDate,
        x: Math.min(startX, endX),
        width: bandWidth,
      }
    })
    .filter((band): band is { key: string; x: number; width: number } => band != null)
  return {
    series,
    width,
    top: CHART_TOP,
    bottom: CHART_BOTTOM,
    columns: placed,
    segments: segments.filter((segment) => segment.length > 0),
    gapBands,
    min: lowest,
    max: highest,
    unknownDays: Math.max(0, page.missingDays),
    unknownValues,
  }
}

/** The line path of one drawn run. */
export function validatorTrendLinePath(columns: readonly ValidatorTrendColumn[]): string {
  return columns
    .map(
      (column, index) =>
        (index === 0 ? 'M' : 'L') + ' ' + column.x.toFixed(2) + ' ' + column.y.toFixed(2),
    )
    .join(' ')
}

/** How one Node association is named: its own display name where the Server
 *  resolved one, otherwise its identifier. */
export function validatorTrendAssociationLabel(association: {
  nodeDisplayName?: string | null
  nodeId: string
}): string {
  const name = association.nodeDisplayName?.trim()
  return name && name.length > 0 ? name : association.nodeId
}
