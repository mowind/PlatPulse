import { Fragment, useId, useState, type ReactNode } from 'react'
import { Copy, Eye, EyeOff, Info } from 'lucide-react'
import type { PublicNode, PublicValidatorInsight } from '../api/generated'
import { Button } from './ui/button'
import { formatAmountCompact, formatAmountExact } from '../lib/amount'
import { formatRatePercentCompact, formatRatePercentExact } from '../lib/rate'
import { MetricRow } from './MetricRow'
import { ExactAmount } from './ExactAmount'
import { CardX } from './ui/card-x'
import { formatUtcDateTime } from './StatusBadge'
import { SURFACE_CARD_STATIC } from '../lib/surface'
import { Disclosure } from './ui/disclosure'
import { DataTooltip } from './ui/data-tooltip'
import { cn } from '../lib/utils'

/**
 * Server-owned Current Validator Status label (#173). It describes whether the
 * automatically identified chain identity has a currently valid staking
 * identity; it is not consensus membership or Node Health. A locked/exiting
 * qualifier is rendered separately by the caller.
 */
export function currentValidatorStatusLabel(status: string | null | undefined): string {
  switch ((status ?? '').toLowerCase()) {
    case 'validator': return 'Validator'
    case 'not_validator': return 'Not a Validator'
    default: return 'Validator status unknown'
  }
}

/** One shared wording for an unestablished staking verdict, used by the status
 *  explanation and the always-visible staking-status note. */
const UNKNOWN_STAKING_EXPLANATION = 'Current staking validity is not established; this is not a negative conclusion.'

/**
 * The one Current Validator Status value that is not a verdict. An
 * unestablished staking validity must not present with the weight of a decided
 * identity (ADR 0005, #173), so it drops the pill and becomes a muted note
 * instead of a third equally-weighted status chip.
 */
const UNKNOWN_STATUS_LABEL = 'Validator status unknown'

/** The plain-language reading of that verdict, for the inline info control. It
 *  restates UNKNOWN_STAKING_EXPLANATION rather than introducing a new word for
 *  the same domain state. */
const UNKNOWN_STAKING_TOOLTIP = 'This does not indicate a negative validator state.'

/** Both identifier controls are secondary to the identifier text they act on,
 *  so they share one resting tone instead of each hand-rolling it. */
const IDENTIFIER_CONTROL_CLASS = 'text-muted-foreground'

/**
 * The Node Detail metric grid: two columns on mobile and three at lg, with
 * every cell stacking its muted caption above a clearer value, left aligned and
 * wrapping rather than reserving a compact one-line slot. Held as one named
 * recipe because all six cells need the identical override set, and kept off
 * the card component so the Home card's container-query anatomy is untouched.
 * The 8px row gap and 2px caption/value gap read as one stats matrix instead of
 * the 16px whitespace the grid used to reserve between rows. The column-gap
 * stays 16px (the geometry test measures it), and the faint lg column rules are
 * placement-aware classes from detailMetricColumnRules rather than a pseudo
 * separator, so a full-width value cannot leave a rule on the wrong column.
 * The cells stretch to the row so those rules span its full height (a warning
 * caption on one metric would otherwise leave a shorter rule beside it), while
 * content-start keeps each cell's own rows at their natural height instead of
 * letting the stretch reopen the caption/value gap the tests bound.
 */
const DETAIL_METRICS_GRID = 'grid grid-cols-2 items-stretch gap-x-4 gap-y-2 lg:grid-cols-3 [&>[data-slot=metric-row]]:grid-cols-1 [&>[data-slot=metric-row]]:gap-y-0.5 [&>[data-slot=metric-row]]:content-start [&_[data-slot=metric-row-label]]:min-w-0 [&_[data-slot=metric-row-label]]:whitespace-normal [&_[data-slot=metric-row-label]]:text-[11px] [&_[data-slot=metric-row-label]]:leading-4 [&_[data-slot=metric-row-label]]:[overflow-wrap:anywhere] [&_[data-slot=metric-row-value]]:text-left [&_[data-slot=metric-row-value]]:justify-start [&_[data-slot=metric-row-value]]:before:hidden [&_[data-slot=metric-row-detail]]:col-span-1 [&_[data-slot=metric-row-detail]]:whitespace-normal [&_[data-slot=metric-row-detail]]:overflow-visible [&_[data-slot=metric-row-detail]]:[overflow-wrap:anywhere]'

/** The two weights of the six metrics. The lifetime totals and the rank lead at
 *  17px semibold; the three rates follow at 13px medium, so the grid reads as
 *  three headline figures over three supporting ones without new cards. */
const PRIMARY_METRIC = '[&_[data-slot=metric-row-value]]:text-[17px] [&_[data-slot=metric-row-value]]:leading-6 [&_[data-slot=metric-row-value]]:font-semibold'
const SECONDARY_METRIC = '[&_[data-slot=metric-row-value]]:text-[13px] [&_[data-slot=metric-row-value]]:leading-5 [&_[data-slot=metric-row-value]]:font-medium'

/** Keep normal values paired on phones; unusually long exact values get the
 * whole row before they can squeeze or overflow a column. The last-resort wrap
 * remains available even at increased text size, with every digit preserved. */
function detailMetricSpan(value: string): string | undefined {
  if (value.length > 24) return 'col-span-full'
  if (value.length > 15) return 'col-span-full sm:col-span-1'
}

/** The faint rule that turns the desktop 3x2 metric grid into a stats matrix.
 *  Whether a cell spans the whole row is detailMetricSpan's decision, and only
 *  its unprefixed `col-span-full` spans at lg (the mid-length case falls back
 *  to a single column from sm up), so the column walk restarts exactly where
 *  the lg grid restarts. Asking detailMetricSpan keeps that one rule in one
 *  place. The rules are lg-only: below the three-column breakpoint the cells
 *  are paired and a separator would only add noise. The rule is the cell's own
 *  1px left border, drawn inside its border-box so every cell keeps the width
 *  the geometry test measures, and the 16px left padding restores the shared
 *  content offset that the 16px column gap gives the unruled columns. */
function detailMetricColumnRules(values: string[]): (string | undefined)[] {
  let column = 0
  return values.map(value => {
    if (detailMetricSpan(value) === 'col-span-full') {
      column = 0
      return undefined
    }
    column = column === 0 ? 1 : column + 1
    const rule = column > 1 ? 'lg:border-l lg:border-border/60 lg:pl-4' : undefined
    if (column === 3) column = 0
    return rule
  })
}

/** The explicit special state of a confirmed-valid identity. */
export function currentValidatorStatusQualifierLabel(qualifier: string | null | undefined): string | null {
  switch ((qualifier ?? '').toLowerCase()) {
    case 'locked': return 'Locked'
    case 'exiting': return 'Exiting'
    default: return null
  }
}

/**
 * Safe public label for a Provider state. The Server never sends raw provider
 * diagnostics to the Public API, so the UI must not invent or echo any: it
 * renders a fixed, sanitized explanation per canonical state (#154).
 */
export function validatorStateLabel(state: string, freshness: string): string {
  switch (state) {
    case 'fresh': return 'Current'
    case 'stale': return 'Stale'
    case 'error': return 'Collection failed'
    case 'not_configured': return 'Not configured'
    case 'unsupported': return 'Unsupported'
    case 'not_found': return 'Validator not found'
    case 'empty': return 'No live Validator'
    default: return freshness === 'unknown' ? 'Never observed' : 'Unknown'
  }
}

export type ValidatorDataStatus = {
  /** Short, visible chip text for the Home card's unified status position. */
  label: string
  tone: 'warning' | 'destructive'
  /** Full sanitized reason; the Home card only carries the short label and
   *  exposes this through its accessible name, while Node detail prints it. */
  description: string
}

/**
 * The one abnormal Provider data status worth a Home card slot. A routine
 * `Current` value deliberately returns null: the removed `Data: Current` line
 * must not be replaced by another always-on row (#154). `empty` is also null
 * because the Validator region's own empty state carries that authoritative
 * verdict. Staleness and request failure stay visible as a short status/icon;
 * the full reason lives in the explanation and Node detail.
 */
export function validatorDataStatus(validator: PublicValidatorInsight): ValidatorDataStatus | null {
  const state = (validator.state ?? '').toLowerCase()
  const freshness = (validator.freshness ?? '').toLowerCase()
  switch (state) {
    case 'error':
      return { label: 'Failed', tone: 'destructive', description: 'The Validator source could not be read; the last successful values are retained where available.' }
    case 'not_configured':
      return { label: 'Not configured', tone: 'warning', description: 'No Validator source is configured for this Network; no value was queried.' }
    case 'unsupported':
      return { label: 'Unsupported', tone: 'warning', description: 'The Validator source does not support this Validator identifier.' }
    case 'not_found':
      return { label: 'Not found', tone: 'warning', description: 'The source has no current record for this Validator.' }
    case 'empty':
      return null
    default:
      break
  }
  if (state === 'stale' || freshness === 'stale') {
    return { label: 'Stale', tone: 'warning', description: 'The last successful Validator value is retained; the source has not refreshed it recently.' }
  }
  // Ranking is fetched independently: a fresh detail with a failed or aged
  // ranking list must stay visible rather than leaving only the retained rank.
  if (validator.rankState === 'error') {
    return { label: 'Ranking failed', tone: 'warning', description: rankNote(validator) ?? 'The Network ranking list could not be read.' }
  }
  if (validator.rankFreshness === 'stale') {
    return { label: 'Rank stale', tone: 'warning', description: rankNote(validator) ?? 'The Network ranking list has not refreshed recently.' }
  }
  return null
}

function blockCountLabel(blockCount: number | null | undefined): string {
  // A source-reported zero is a real cumulative count, never rendered as
  // Unknown; only a missing value is Unknown.
  return blockCount == null ? 'Unknown' : blockCount.toLocaleString()
}

/**
 * The cumulative completion rate is a Server computation over the same
 * observation's numerator and denominator. Its state is explicit: a known zero
 * denominator is Not applicable (no scheduled duties), an incomplete pair is
 * Unknown, and only `ok` carries a rate (#156).
 */
function formatRate(value: string | null | undefined, precision: RatePrecision = 'display'): string {
  if (value == null) return 'Unknown'
  return precision === 'exact' ? formatRatePercentExact(value) : formatRatePercentCompact(value)
}

/** Node detail shows a rate at two decimals at most (`display`); the source
 *  digits (`exact`) stay in the diagnostics' full-precision area and in the
 *  value's own title. Only the presentation differs: the source string is never
 *  recomputed, converted to a float or capped at 100%. */
type RatePrecision = 'display' | 'exact'

function blockRateLabel(validator: PublicValidatorInsight, precision: RatePrecision = 'display'): string {
  if (validator.blockRateState === 'not_applicable') return 'Not applicable'
  if (validator.blockRateState !== 'ok') return 'Unknown'
  return formatRate(validator.blockRate, precision)
}

/** PlatScan's own source-reported 24-hour rate; never locally reconstructed. */
function genBlocksRateLabel(validator: PublicValidatorInsight, precision: RatePrecision = 'display'): string {
  return formatRate(validator.genBlocksRate, precision)
}

/**
 * The currently effective delegation reward distribution percentage. The
 * Server already normalized the source to percentage points, so a legitimate
 * 0 and 100 render as 0.00% and 100.00% and a missing value is Unknown — never
 * a fabricated 0. It is deliberately kept apart from the annualized
 * `rewardRate` and never labelled with a yield or commission meaning (#157).
 */
function delegationRewardShareLabel(validator: PublicValidatorInsight, precision: RatePrecision = 'display'): string {
  return formatRate(validator.delegationRewardPercentage, precision)
}

/**
 * PlatScan's rank within the Network's complete live-staking ALL cohort. Only
 * a complete successful list can show Unranked; a failed or incomplete list
 * keeps the last-good rank (marked stale) or Unknown. Rank is never rendered
 * as zero, and Home filters never redefine it (#158).
 */
export function rankLabel(validator: PublicValidatorInsight): string {
  if (validator.rankState === 'unranked') return 'Unranked'
  if (validator.rank == null) return 'Unknown'
  return `#${validator.rank}`
}

/** Sanitized explanation for the independent ranking state (#158). */
function rankNote(validator: PublicValidatorInsight): string | null {
  switch (validator.rankState) {
    case 'ranked': return validator.rankFreshness === 'stale'
      ? 'The Network ranking list has not refreshed recently; the last successful rank is shown.'
      : null
    case 'unranked': return validator.rankFreshness === 'stale'
      ? 'The Network ranking list has not refreshed recently; the last complete list did not include this Validator.'
      : "Not in this Network's complete live-staking ALL cohort."
    case 'error': return validator.rank == null
      ? 'The Network ranking list could not be read; no last-good rank is available.'
      : 'The Network ranking list could not be refreshed; the last successful rank is retained.'
    case 'not_configured': return 'No Validator source is configured for this Network; no ranking was queried.'
    case 'unsupported': return 'The Validator source does not support Network ranking.'
    case 'unknown': return 'No Network ranking has been observed yet.'
    default: return null
  }
}

function stateNote(validator: PublicValidatorInsight): string | null {
  switch (validator.state) {
    case 'not_configured': return 'No Validator source is configured for this Network; no value was queried.'
    case 'error': return 'The Validator source could not be read; the last successful values are retained.'
    case 'unsupported': return 'The Validator source does not support this Validator identifier.'
    case 'not_found': return 'The source has no current record for this Validator.'
    case 'empty': return 'The source reported no live Validator for this identifier.'
    default: return null
  }
}

/**
 * Raw, business-valued metric slots. A status word is not a value: an
 * `unranked` outcome means the retained `rank` number is not rendered as a
 * value, so it must not make the six-slot structure appear either.
 */
function hasMetricValues(validator: PublicValidatorInsight): boolean {
  if (validator.blockCount != null || validator.rewardAmount != null || validator.blockRate != null
    || validator.genBlocksRate != null || validator.delegationRewardPercentage != null) return true
  return validator.rank != null && validator.rankState !== 'unranked'
}

/**
 * Whether the Home card has anything meaningful to print in the six metric
 * slots. A known `not_applicable` production rate is a displayable outcome even
 * when nothing else is known; an `unranked` rank is a ranking conclusion, not a
 * metric value, so it alone keeps the compact empty state. When this is false
 * the region keeps its reserved height with one short, accurate empty state
 * instead of six Unknown slots (#154, #158, #173).
 */
function hasDisplayableMetrics(validator: PublicValidatorInsight): boolean {
  return hasMetricValues(validator) || validator.blockRateState === 'not_applicable'
}

/**
 * One short, accurate Home empty state that keeps the real Server states
 * distinct. It never renders six placeholder values, never invents a zero and
 * never collapses identity, failure and freshness into one "Not a Validator"
 * line; the full reason stays in Node detail.
 */
function emptyMetricsState(validator: PublicValidatorInsight): { text: string; state: string } {
  const state = (validator.state ?? '').toLowerCase()
  const freshness = (validator.freshness ?? '').toLowerCase()
  switch (state) {
    case 'loading': return { text: 'Loading Validator metrics…', state: 'loading' }
    case 'error': return { text: 'The Validator source could not be read; no metrics are available.', state: 'error' }
    case 'not_configured': return { text: 'No Validator source is configured for this Network.', state: 'not_configured' }
    case 'unsupported': return { text: 'The Validator source does not support this Validator identifier.', state: 'unsupported' }
    case 'not_found': return { text: 'The source has no current record for this Validator.', state: 'not_found' }
    case 'empty': return { text: 'No validator metrics', state: 'empty' }
    case 'stale': return { text: 'The Validator source has not refreshed; no metrics are available.', state: 'stale' }
    default: break
  }
  if (validator.rankState === 'unranked') {
    return { text: 'Not in the Network’s complete live-staking cohort; no Validator metrics are available.', state: 'unranked' }
  }
  if (validator.currentValidatorStatus === 'not_validator') {
    return { text: 'No current Validator identity; no metrics are available.', state: 'not_validator' }
  }
  if (validator.currentValidatorStatus === 'unknown') {
    return { text: 'Current staking validity is not established; no metrics are available.', state: 'unknown_status' }
  }
  if (state === 'unknown' || freshness === 'stale' || freshness === 'unknown') {
    return { text: 'No Validator data has been observed yet.', state: 'unknown' }
  }
  return { text: 'No Validator metrics are available.', state: state || 'unknown' }
}

/**
 * A Linked Validator region the Home card shares with Node detail. The Home
 * card owns only what the Server's Current Validator Status confirms — the three
 * summary metrics of a Validator, one short word for a confirmed non-Validator,
 * or a short Validator unknown line with its reason (#238). Identity, identifier,
 * copy, the remaining metrics and every long explanation were moved to Node
 * detail, which the whole card already links to (#154–#158, #173).
 */
export function LinkedValidatorSection({ node, variant = 'card' }: { node: PublicNode; variant?: 'card' | 'detail' }) {
  const validator = node.validator
  // Missing identity discovery is not authoritative absence of staking. Keep
  // it distinct from an established Link whose six metrics are still unknown.
  if (!validator) return <ValidatorIdentityAbsent node={node} variant={variant} />
  if (variant === 'detail') return <ValidatorDetail node={node} validator={validator} />
  return <ValidatorCard validator={validator} />
}

/**
 * A Node with no effective Link. The Home card states one short Validator
 * unknown line and the reason it comes with; Node detail keeps the sanitized
 * explanation. Missing discovery and an identified-but-unavailable identity stay
 * distinct from each other and from an established Link whose verdict is unknown
 * (#173, #238).
 */
function ValidatorIdentityAbsent({ node, variant }: { node: PublicNode; variant: 'card' | 'detail' }) {
  const state = node.validatorIdentityState
  // One short, accurate Home state. Optional null state means no discovery pass
  // has run; identified without insight is data unavailability. The Server's
  // full sanitized explanation is kept for Node detail, so the Home card never
  // reprints a long association-identity paragraph (#173).
  const short = state == null
    ? 'Identity has not been observed yet.'
    : state === 'identified'
      ? 'Validator data is unavailable.'
      : 'Identity has not been established.'
  const reason = node.validatorIdentityReason || short
  if (variant === 'detail') {
    return (
      <section
        data-slot="linked-validator"
        className="mt-4 min-w-0 border-t border-border pt-3"
        aria-label="Linked Validator identity"
      >
        <h3 className="m-0 text-xs font-medium tracking-wider text-muted-foreground">Validator identity</h3>
        <p className="m-0 mt-1 text-[11px] text-muted-foreground" role="status" aria-label={`Validator identity state: ${state ?? 'not observed'}`}>{reason}</p>
      </section>
    )
  }
  return (
    <section
      data-slot="linked-validator"
      data-identity="unknown"
      className="min-w-0 border-t border-border pt-3"
      aria-label="Linked Validator"
    >
      <p
        data-slot="linked-validator-identity"
        className="m-0 text-[11px] leading-4 text-muted-foreground"
        role="status"
        aria-label={`Validator identity state: ${state ?? 'not observed'}`}
      >
        <span data-slot="linked-validator-identity-label" className="font-medium text-foreground">Validator unknown</span>
        <span aria-hidden="true"> — </span>
        <span>{short}</span>
      </p>
    </section>
  )
}

/**
 * Node detail keeps every fact the Home card no longer carries: the identity
 * name, the full identifier with an explicit copy control, the six metrics with
 * full reward precision available in diagnostics, independent Provider/ranking freshness
 * dimensions, the sanitized state strings and the provenance facts.
 */
function ValidatorDetail({ node, validator }: { node: PublicNode; validator: PublicValidatorInsight }) {
  const statusLabel = currentValidatorStatusLabel(validator.currentValidatorStatus)
  const stakingUnknown = statusLabel === UNKNOWN_STATUS_LABEL
  const rewards = formatAmountExact(validator.rewardAmount)
  // Truncate only the overview, using source digits rather than floating point.
  const overviewRewards = rewards.replace(/([.][0-9]{4})[0-9]+$/, '$1')
  const blocks = blockCountLabel(validator.blockCount)
  const rank = rankLabel(validator)
  const production = blockRateLabel(validator)
  const platscanRate = genBlocksRateLabel(validator)
  const rewardShare = delegationRewardShareLabel(validator)
  // A rounded rate keeps its source digits as the value's title; the
  // diagnostics' full-precision area carries them for touch and keyboard.
  const rateValue = (display: string, exact: string) => display === exact ? display : <span title={exact}>{display}</span>
  const metricColumnRules = detailMetricColumnRules([blocks, overviewRewards, rank, production, platscanRate, rewardShare])
  const identifier = validator.validatorNodeId
  const identifierId = useId()
  const [identifierOpen, setIdentifierOpen] = useState(false)
  const [copyStatus, setCopyStatus] = useState('')
  // These are independent observations, not a priority list: a failed Provider
  // must not mask a failed ranking list, stale staking verdict or counter reset.
  const providerNote = stateNote(validator)
  const retainedMetrics = hasMetricValues(validator) && !['fresh', 'stale', 'error'].includes(validator.state)
  const overviewNotes = [
    validator.state === 'error' && validator.freshness === 'stale'
      ? 'The Validator source could not be read; the last successful values are retained and stale.'
      : retainedMetrics
        ? [providerNote, 'Showing the last successful metrics; the current source state is unavailable.'].filter(Boolean).join(' ')
        : providerNote,
    validator.state !== 'error' && (validator.state === 'stale' || validator.freshness === 'stale')
      ? 'The last successful Validator value is stale; the source has not refreshed it recently.' : null,
    node.validatorIdentityReason
      ? node.validatorIdentityReason + ' The last established association is retained until identification succeeds.' : null,
    validator.currentValidatorStatusState === 'stale'
      ? 'The last confirmed Validator status is retained; the source has not refreshed it recently.' : null,
    validator.currentValidatorStatus === 'not_validator'
      ? 'Authoritative evidence reports no current staking identity for this chain key.' : null,
  ].filter((note): note is string => note != null)
  const rankWarning = validator.state === 'not_configured' && validator.rankState === 'not_configured' ? null : rankNote(validator)
  const rateWarning = validator.blockRateState === 'not_applicable'
    ? 'The source reported a zero scheduled-block denominator, so a rate is not applicable — not 0%.'
    : validator.blockRateState !== 'ok' || validator.blockRate == null
      ? 'The source has not supplied a complete cumulative produced/scheduled block pair; production rate is unknown.' : null
  const counterWarning = validator.counterState === 'counter_reset'
    ? 'Counter reset or correction observed; the prior value was not treated as normal growth.' : null
  const metricNote = (note: string | null) => note ? <span role="status">{note}</span> : undefined
  // Keep a meaningful empty outcome without reprinting the same Provider or
  // unknown-staking explanation already visible in the overview.
  const emptyNote = !hasDisplayableMetrics(validator) && !stateNote(validator)
    && statusLabel !== 'Validator status unknown' && validator.currentValidatorStatus !== 'not_validator'
    && validator.state !== 'stale' && validator.freshness !== 'stale'
    ? emptyMetricsState(validator).text : null
  const copyIdentifier = async () => {
    try {
      await navigator.clipboard.writeText(identifier)
      setCopyStatus('Validator identifier copied.')
    } catch {
      setIdentifierOpen(true)
      setCopyStatus('Copy failed. Select the full identifier to copy it manually.')
    }
  }
  return <CardX
    bordered={false}
    role="region"
    aria-label="Validator performance"
    data-slot="linked-validator"
    className={cn('mt-4 min-w-0 rounded-md border-none', SURFACE_CARD_STATIC)}
    contentClassName="flex min-w-0 flex-col gap-1.5"
  >
    <header>
      <h2 className="m-0 text-sm font-medium leading-5">Validator performance</h2>
    </header>
    {overviewNotes.length > 0 && <div className="grid gap-1">
      {overviewNotes.map(note => <p key={note} className="m-0 text-[11px] text-muted-foreground" role="status">{note}</p>)}
    </div>}
    {stakingUnknown && <p data-slot="staking-status-note" className="m-0 flex min-w-0 flex-wrap items-center gap-1 text-[11px] text-muted-foreground" role="status">
      {UNKNOWN_STAKING_EXPLANATION}
      <DataTooltip as="span" content={UNKNOWN_STAKING_TOOLTIP} className="align-middle">
        <span tabIndex={0} aria-label={UNKNOWN_STAKING_TOOLTIP} className="inline-flex min-h-6 min-w-6 items-center justify-center">
          <Info className="size-3.5" aria-hidden="true" />
        </span>
      </DataTooltip>
    </p>}
    <div data-slot="validator-performance-metrics" className={DETAIL_METRICS_GRID} role="group" aria-label="Validator performance metrics">
      <MetricRow label="Cumulative blocks" className={cn(PRIMARY_METRIC, detailMetricSpan(blocks), metricColumnRules[0])} value={<ExactAmount value={blocks} />} detail={metricNote(counterWarning)} />
      <MetricRow label="Cumulative rewards" className={cn(PRIMARY_METRIC, detailMetricSpan(overviewRewards), metricColumnRules[1])} value={<span title={rewards}><ExactAmount value={overviewRewards} muteFraction /></span>} />
      <MetricRow label="Network rank" className={cn(PRIMARY_METRIC, detailMetricSpan(rank), metricColumnRules[2])} value={rank} detail={metricNote(rankWarning)} />
      <MetricRow label="Production rate" className={cn(SECONDARY_METRIC, detailMetricSpan(production), metricColumnRules[3])} value={rateValue(production, blockRateLabel(validator, 'exact'))} detail={metricNote(rateWarning)} />
      <MetricRow label="PlatScan 24h rate" className={cn(SECONDARY_METRIC, detailMetricSpan(platscanRate), metricColumnRules[4])} value={rateValue(platscanRate, genBlocksRateLabel(validator, 'exact'))} />
      <MetricRow label="Delegation reward share" className={cn(SECONDARY_METRIC, detailMetricSpan(rewardShare), metricColumnRules[5])} value={rateValue(rewardShare, delegationRewardShareLabel(validator, 'exact'))} />
    </div>
    {emptyNote && <p className="m-0 text-xs text-muted-foreground" role="status">{emptyNote}</p>}
    <div className="flex min-w-0 flex-col gap-1">
      <div className="flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1">
        <span className="shrink-0 text-xs text-muted-foreground">Validator ID</span>
        <code className="min-w-0 font-mono text-xs [overflow-wrap:anywhere]">{identifier.length > 24 ? identifier.slice(0, 12) + '…' + identifier.slice(-8) : identifier}</code>
        <span className="inline-flex shrink-0 items-center gap-1">
          <DataTooltip as="span" content="Copy full Validator identifier">
            <Button variant="ghost" size="icon-sm" aria-label="Copy full Validator identifier" className={IDENTIFIER_CONTROL_CLASS} onClick={() => { void copyIdentifier() }}>
              <Copy className="size-3.5" aria-hidden="true" />
            </Button>
          </DataTooltip>
          <DataTooltip as="span" content={identifierOpen ? 'Hide full ID' : 'Show full ID'}>
            <Button variant="ghost" size="icon-sm" aria-label={identifierOpen ? 'Hide full ID' : 'Show full ID'} className={IDENTIFIER_CONTROL_CLASS} aria-expanded={identifierOpen} aria-controls={identifierId} onClick={() => setIdentifierOpen(open => !open)}>
              {identifierOpen ? <EyeOff className="size-3.5" aria-hidden="true" /> : <Eye className="size-3.5" aria-hidden="true" />}
            </Button>
          </DataTooltip>
        </span>
      </div>
      {identifierOpen && <code id={identifierId} className="min-w-0 select-text rounded-md border border-border bg-background p-2 font-mono text-xs [overflow-wrap:anywhere]" aria-label={'Validator identifier: ' + identifier}>{identifier}</code>}
      <p role="status" aria-label="Identifier copy status" className={cn('m-0 text-xs text-muted-foreground', !copyStatus && 'sr-only')}>{copyStatus}</p>
    </div>
  </CardX>
}

/**
 * The Validator diagnostics, in their own region after the monitoring deck
 * (#242 follow-up). They used to close the performance card, where expanding
 * them pushed the six charts off the screen; as an advanced region below the
 * deck the charts keep their place while every Provider, ranking, freshness,
 * source and full-precision reward fact stays exactly where a reader looks for
 * it. Collapsed on first load, and absent entirely for a Node that is not a
 * confirmed Validator — the same gate as the performance card itself.
 */
export function LinkedValidatorDiagnostics({ node, validator }: { node: PublicNode; validator: PublicValidatorInsight }) {
  const qualifierLabel = currentValidatorStatusQualifierLabel(validator.currentValidatorStatusQualifier)
  const rewards = formatAmountExact(validator.rewardAmount)
  const state = (value: string | null | undefined) => value ?? 'Not provided'
  const time = (value: string | null | undefined, missing: string) => value ? formatUtcDateTime(value) : missing
  return <Disclosure
    title="Validator diagnostics"
    description="Provider, freshness, ranking and source details"
    className="min-w-0"
  >
    {/* Three compact key/value groups instead of one long label-over-value
        list. Every independent state keeps its own row and its own wording:
        nothing is merged into a summary verdict. */}
    <div data-slot="validator-diagnostics-groups" role="group" aria-label="Public Validator states" className="grid min-w-0 grid-cols-1 gap-x-6 gap-y-4 sm:grid-cols-2 lg:grid-cols-3">
      <DiagnosticsGroup group="identity" title="Identity & activity" facts={[
        ...(validator.displayName ? [['PlatScan identity name', validator.displayName] as const] : []),
        ['Current staking status', state(validator.currentValidatorStatus)],
        ['Staking status state', state(validator.currentValidatorStatusState)],
        ['Staking qualifier', state(validator.currentValidatorStatusQualifier)],
        ['Identity state', state(node.validatorIdentityState)],
        ['Activity', state(validator.activity)],
        ['Activity state', state(validator.activityState)],
      ]}>
        {qualifierLabel && <p className="m-0 mt-1 text-[11px] leading-4 text-muted-foreground">This identity has a confirmed-valid staking identity but is {qualifierLabel.toLowerCase()}, not normally producing.</p>}
      </DiagnosticsGroup>
      <DiagnosticsGroup group="source" title="Source & freshness" facts={[
        ['Provider state', state(validator.state)],
        ['Data freshness', state(validator.freshness)],
        ['Last success', time(validator.receivedAt, 'Never observed')],
        ['Source', validator.source ?? 'Unknown'],
        ['Source cutoff', time(validator.providerTimestamp, 'Not provided by the source')],
      ]} />
      {/* The ranking list is fetched independently, so it has its own success time. */}
      <DiagnosticsGroup group="ranking" title="Ranking & counters" facts={[
        ['Rank state', state(validator.rankState)],
        ['Rank freshness', state(validator.rankFreshness)],
        ['Rank last success', time(validator.rankReceivedAt, 'Never observed')],
        ['Rank cohort', validator.rankCohortSize != null ? String(validator.rankCohortSize) : 'Unknown'],
        ['Counter state', state(validator.counterState)],
        ['Production rate state', state(validator.blockRateState)],
      ]} />
    </div>
    {/* The source digits the overview rounds, in one quiet footer: selectable,
        copyable and never re-derived. */}
    <div data-slot="validator-full-precision" className="min-w-0 border-t border-dashed border-border/60 pt-2">
      <dl className="m-0 grid min-w-0 grid-cols-1 gap-x-6 gap-y-1.5 text-xs sm:grid-cols-2 lg:grid-cols-4" aria-label="Full-precision source values">
        <FullPrecisionFact label="Cumulative rewards (full precision)"><ExactAmount value={rewards} /></FullPrecisionFact>
        <FullPrecisionFact label="Production rate (full precision)">{blockRateLabel(validator, 'exact')}</FullPrecisionFact>
        <FullPrecisionFact label="PlatScan 24h rate (full precision)">{genBlocksRateLabel(validator, 'exact')}</FullPrecisionFact>
        <FullPrecisionFact label="Delegation reward share (full precision)">{delegationRewardShareLabel(validator, 'exact')}</FullPrecisionFact>
      </dl>
      {validator.rewardAmount != null && <p className="m-0 mt-1.5 text-[11px] leading-4 text-muted-foreground">Amounts use the Network native unit.</p>}
    </div>
  </Disclosure>
}

/** One diagnostics group: a small heading over left-label/right-value rows. A
 *  long timestamp or source name wraps inside its own value cell. */
function DiagnosticsGroup({ group, title, facts, children }: {
  group: 'identity' | 'source' | 'ranking'
  title: string
  facts: ReadonlyArray<readonly [string, string]>
  children?: ReactNode
}) {
  const headingId = useId()
  return <section data-slot="validator-diagnostics-group" data-group={group} aria-labelledby={headingId} className="min-w-0">
    <h3 id={headingId} className="m-0 pb-1 text-xs font-medium text-foreground">{title}</h3>
    <dl className="m-0 grid min-w-0 grid-cols-[minmax(0,max-content)_minmax(0,1fr)] text-xs">
      {facts.map(([label, value]) => <Fragment key={label}>
        <dt className="min-w-0 border-t border-border/60 py-1 text-muted-foreground">{label}</dt>
        <dd className="m-0 min-w-0 border-t border-border/60 py-1 pl-3 text-right tabular-nums [overflow-wrap:anywhere]">{value}</dd>
      </Fragment>)}
    </dl>
    {children}
  </section>
}

function FullPrecisionFact({ label, children }: { label: string; children: ReactNode }) {
  return <div className="min-w-0">
    <dt className="text-[11px] leading-4 text-muted-foreground">{label}</dt>
    <dd className="m-0 select-text tabular-nums [overflow-wrap:anywhere]">{children}</dd>
  </div>
}

/**
 * The Home card's Linked Validator region (#238). Only the Server-owned Current
 * Validator Status decides what it says: a confirmed Validator shows the three
 * key summary metrics, a confirmed non-Validator shows one short word, and
 * evidence that cannot confirm either stays Validator unknown with its real
 * reason. Activity, consensus membership, a missing rank or reward and retained
 * history never decide the identity. The six-metric grid, its six Unknown
 * placeholders and the reserved `No validator metrics` region are gone from
 * Home; Node detail keeps every metric, state and explanation (#154–#158, #173).
 */
function ValidatorCard({ validator }: { validator: PublicValidatorInsight }) {
  const status = (validator.currentValidatorStatus ?? '').toLowerCase()
  if (status !== 'validator') {
    // Only current evidence may state the negative verdict: a retained
    // not_validator status is unconfirmed, so it stays Validator unknown with the
    // retained verdict as its reason (issue #238).
    return <ValidatorIdentityNote validator={validator}
      confirmedNonValidator={status === 'not_validator' && (validator.currentValidatorStatusState ?? '').toLowerCase() === 'current'} />
  }
  const rank = rankLabel(validator)
  const rankReason = rankNote(validator)
  return (
    <section data-slot="linked-validator" data-identity="validator" className="min-w-0 border-t border-border pt-3" aria-label="Linked Validator">
      <div data-slot="linked-validator-metrics" role="group" aria-label="Linked Validator metrics">
        {/* Rank stays the Network's own PlatScan rank: Unranked, an unattested
            rank and a retained last-good rank stay three different statements. */}
        <ValidatorSummaryCell
          metric="rank"
          label="Network rank"
          shortLabel="Rank"
          value={rank === 'Unknown' ? null : rank}
          title={rankReason ?? undefined}
          reason={rankReason ?? 'No Network rank is available.'}
        />
        <ValidatorSummaryCell
          metric="blocks"
          label="Cumulative blocks"
          shortLabel="Blocks"
          value={validator.blockCount == null ? null : blockCountLabel(validator.blockCount)}
          reason="No cumulative block count is available from the Validator source."
        />
        {/* The gross lifetime reward, abbreviated for the compact cell; the exact
            source digits stay reachable and it is never presented as operator net
            earnings. */}
        <ValidatorSummaryCell
          metric="rewards"
          label="Cumulative rewards"
          shortLabel="Rewards"
          value={validator.rewardAmount == null ? null : formatAmountCompact(validator.rewardAmount)}
          title={validator.rewardAmount == null ? undefined : formatAmountExact(validator.rewardAmount)}
          reason="No cumulative reward amount is available from the Validator source."
        />
      </div>
    </section>
  )
}

/**
 * The identity statement behind the Home card's short line for everything
 * Current Validator Status does not confirm as a Validator (#238, round 2).
 * Only that Server-owned verdict decides which word appears: a `current`
 * non-Validator is confirmed, a retained non-Validator verdict is not, and
 * unestablished validity always reads Validator unknown — never Non-validator.
 * `null` means the Server does confirm a Validator, so the line has nothing to
 * say and the summary cells speak instead.
 */
export function validatorIdentityStatement(validator: PublicValidatorInsight): { label: string; detail: string } | null {
  const status = (validator.currentValidatorStatus ?? '').toLowerCase()
  if (status === 'validator') return null
  if (status !== 'not_validator') return { label: 'Validator unknown', detail: 'Staking validity is not established.' }
  const state = (validator.currentValidatorStatusState ?? '').toLowerCase()
  return state === 'current'
    ? { label: 'Non-validator', detail: 'No current staking identity.' }
    : { label: 'Validator unknown', detail: 'Confirmed non-Validator status retained.' }
}

/**
 * The Home card's short identity line for everything Current Validator Status
 * does not confirm as a Validator (#238). A confirmed non-Validator shows the
 * word alone: its reason moves to the line's own title and to the identity
 * entry, so the repeated sentence stops crowding a footer that has nothing left
 * to explain. That word is the quietest thing the footer can say — the card has
 * no Validator metrics to show and no verdict to weigh — so it takes the muted
 * tone the reason beside it uses rather than the foreground tone a value would.
 * The unconfirmed case keeps its reason on the face and its foreground word,
 * because an unestablished identity must never be read as a confirmed
 * non-Validator.
 */
function ValidatorIdentityNote({ validator, confirmedNonValidator }: {
  validator: PublicValidatorInsight
  confirmedNonValidator: boolean
}) {
  const state = (validator.currentValidatorStatusState ?? '').toLowerCase()
  const statement = validatorIdentityStatement(validator)
    ?? { label: 'Validator unknown', detail: 'Staking validity is not established.' }
  return (
    <section
      data-slot="linked-validator"
      data-identity={confirmedNonValidator ? 'not_validator' : 'unknown'}
      className="min-w-0 border-t border-border pt-3"
      aria-label="Linked Validator"
    >
      <p
        data-slot="linked-validator-identity"
        className="m-0 text-[11px] leading-4 text-muted-foreground"
        role="status"
        title={statement.detail}
        aria-label={`Validator identity state: ${state || 'unknown'}`}
      >
        <span
          data-slot="linked-validator-identity-label"
          className={cn('font-medium', confirmedNonValidator ? 'text-muted-foreground' : 'text-foreground')}
        >
          {statement.label}
        </span>
        {!confirmedNonValidator && (
          <>
            <span aria-hidden="true"> — </span>
            <span data-slot="linked-validator-identity-reason">{statement.detail}</span>
          </>
        )}
      </p>
    </section>
  )
}

/**
 * One compact cell of the Home Validator summary (#238): the short label a
 * narrow cell holds, with the full metric name as its accessible name and title,
 * and the value beneath it. A value the Server never attested reads Unknown with
 * its reason announced — never a fabricated zero — while a real zero stays a
 * value.
 */
function ValidatorSummaryCell({ metric, label, shortLabel, value, title, reason }: {
  metric: 'rank' | 'blocks' | 'rewards'
  label: string
  shortLabel: string
  /** null when the Server never attested this value. */
  value: string | null
  title?: string
  reason: string
}) {
  const descriptionId = useId()
  return (
    // A compact value too long for one track takes the whole row rather than
    // splitting its digits (validator-metrics.md, Emerald deviation 22).
    <div data-slot="validator-summary-cell" data-metric={metric} data-wide={value !== null && value.length > 12 ? 'true' : undefined} className="min-w-0">
      <span data-slot="validator-summary-label" className="block text-[11px] leading-4 text-muted-foreground">
        <span data-full-label>{label}</span>
        <span data-short-label aria-label={label} title={label}>{shortLabel}</span>
      </span>
      <strong data-slot="validator-summary-value" className="block min-w-0 text-[13px] font-medium leading-[1.375rem] tabular-nums text-foreground">
        {value === null
          ? <span aria-describedby={descriptionId}>Unknown<span id={descriptionId} className="sr-only">: {reason}</span></span>
          : <span className="block whitespace-nowrap" title={title}><ExactAmount value={value} /></span>}
      </strong>
    </div>
  )
}
