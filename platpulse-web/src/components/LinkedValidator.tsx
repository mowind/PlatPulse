import { useId, useState } from 'react'
import { Copy, Eye, EyeOff, Info } from 'lucide-react'
import type { PublicNode, PublicValidatorInsight } from '../api/generated'
import { Button } from './ui/button'
import { formatAmountCompact, formatAmountExact } from '../lib/amount'
import { formatRatePercent2, formatRatePercentExact } from '../lib/rate'
import { MetricRow } from './MetricRow'
import { ExactAmount } from './ExactAmount'
import { CardX } from './ui/card-x'
import { formatUtcDateTime } from './StatusBadge'
import { SURFACE_CARD_STATIC } from '../lib/surface'
import { Disclosure } from './ui/disclosure'
import { DataTooltip } from './ui/data-tooltip'
import { ValidatorActivityBadge } from './ValidatorActivityBadge'
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

/**
 * The Node Detail metric grid: one column on mobile, two at sm and three at lg,
 * with every cell stacking its caption above the value, left aligned and
 * wrapping rather than reserving a compact one-line slot. Held as one named
 * recipe because all six cells need the identical override set, and kept off
 * the card component so the Home card's container-query anatomy is untouched.
 * The 10px row gap keeps the two desktop rows visually grouped instead of the
 * 16px whitespace the grid used to reserve between them.
 */
const DETAIL_METRICS_GRID = 'grid grid-cols-1 items-start gap-x-10 gap-y-2.5 sm:grid-cols-2 lg:grid-cols-3 [&>[data-slot=metric-row]]:grid-cols-1 [&_[data-slot=metric-row-label]]:min-w-0 [&_[data-slot=metric-row-label]]:whitespace-normal [&_[data-slot=metric-row-label]]:[overflow-wrap:anywhere] [&_[data-slot=metric-row-value]]:text-left [&_[data-slot=metric-row-value]]:text-sm [&_[data-slot=metric-row-value]]:font-semibold [&_[data-slot=metric-row-value]]:justify-start [&_[data-slot=metric-row-value]]:before:hidden [&_[data-slot=metric-row-detail]]:col-span-1 [&_[data-slot=metric-row-detail]]:whitespace-normal [&_[data-slot=metric-row-detail]]:overflow-visible [&_[data-slot=metric-row-detail]]:[overflow-wrap:anywhere]'

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
function formatRate(value: string | null | undefined, variant: 'card' | 'detail'): string {
  if (value == null) return 'Unknown'
  return variant === 'detail' ? formatRatePercentExact(value) : formatRatePercent2(value)
}

function blockRateLabel(validator: PublicValidatorInsight, variant: 'card' | 'detail'): string {
  if (validator.blockRateState === 'not_applicable') return 'Not applicable'
  if (validator.blockRateState !== 'ok') return 'Unknown'
  return formatRate(validator.blockRate, variant)
}

/** PlatScan's own source-reported 24-hour rate; never locally reconstructed. */
function genBlocksRateLabel(validator: PublicValidatorInsight, variant: 'card' | 'detail'): string {
  return formatRate(validator.genBlocksRate, variant)
}

/**
 * The currently effective delegation reward distribution percentage. The
 * Server already normalized the source to percentage points, so a legitimate
 * 0 and 100 render as 0.00% and 100.00% and a missing value is Unknown — never
 * a fabricated 0. It is deliberately kept apart from the annualized
 * `rewardRate` and never labelled with a yield or commission meaning (#157).
 */
function delegationRewardShareLabel(validator: PublicValidatorInsight, variant: 'card' | 'detail'): string {
  return formatRate(validator.delegationRewardPercentage, variant)
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
 * A Linked Validator region the Home card shares with Node detail. On the Home
 * card it owns only the six metrics and their compact empty state: identity,
 * identifier, copy, Provider data state and every long explanation were moved
 * to Node detail, which the whole card already links to (#154–#158, #173).
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
 * A Node with no effective Link. The Home card keeps one short, accurate
 * state; Node detail keeps the sanitized explanation. Missing discovery and an
 * identified-but-unavailable identity stay distinct from each other and from an
 * established Link whose metrics are unknown (#173).
 */
function ValidatorIdentityAbsent({ node, variant }: { node: PublicNode; variant: 'card' | 'detail' }) {
  const state = node.validatorIdentityState
  // One short, accurate Home state. Optional null state means no discovery pass
  // has run; identified without insight is data unavailability. The Server's
  // full sanitized explanation is kept for Node detail, so the Home card never
  // reprints a long association-identity paragraph (#173).
  const short = state == null
    ? 'Validator identity has not been observed yet.'
    : state === 'identified'
      ? 'Validator data is unavailable for the identified identity.'
      : 'No Validator identity has been established for this Node.'
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
      className="min-w-0"
      aria-label="Linked Validator"
    >
      <p data-slot="linked-validator-empty" className="m-0 text-[11px] text-muted-foreground" role="status" aria-label={`Validator identity state: ${state ?? 'not observed'}`}>{short}</p>
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
  const qualifierLabel = currentValidatorStatusQualifierLabel(validator.currentValidatorStatusQualifier)
  const rewards = formatAmountExact(validator.rewardAmount)
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
    qualifierLabel ? 'This identity has a confirmed-valid staking identity but is ' + qualifierLabel.toLowerCase() + ', not normally producing.' : null,
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
    aria-label="Linked Validator"
    data-slot="linked-validator"
    className={cn('mt-4 min-w-0 rounded-md border-none', SURFACE_CARD_STATIC)}
    contentClassName="flex min-w-0 flex-col gap-3"
  >
    <header className="flex min-w-0 flex-wrap items-center justify-between gap-2">
      <h3 className="m-0 text-xs font-medium tracking-wider text-muted-foreground">Linked Validator</h3>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        {!stakingUnknown && <span className="rounded-full border border-border/60 px-2 py-0.5 text-[10px] font-normal text-muted-foreground">{statusLabel}</span>}
        {qualifierLabel && <span className="rounded-full border border-amber-500/50 px-2 py-0.5 text-[10px] font-semibold text-amber-600">{qualifierLabel}</span>}
        <span className="text-xs text-muted-foreground" aria-label={'Validator Provider state: ' + validatorStateLabel(validator.state, validator.freshness)}>{validatorStateLabel(validator.state, validator.freshness)}</span>
        <ValidatorActivityBadge validator={validator} identityReason={node.validatorIdentityReason} />
      </div>
    </header>
    {validator.displayName && <p className="m-0 min-w-0 text-sm font-semibold [overflow-wrap:anywhere]">{validator.displayName}</p>}
    {overviewNotes.map(note => <p key={note} className="m-0 text-xs text-muted-foreground" role="status">{note}</p>)}
    {stakingUnknown && <p data-slot="staking-status-note" className="m-0 flex min-w-0 flex-wrap items-center gap-1 text-[11px] text-muted-foreground" role="status">
      {UNKNOWN_STAKING_EXPLANATION}
      <DataTooltip as="span" content={UNKNOWN_STAKING_TOOLTIP} className="align-middle">
        <span tabIndex={0} aria-label={UNKNOWN_STAKING_TOOLTIP} className="inline-flex min-h-6 min-w-6 items-center justify-center">
          <Info className="size-3.5" aria-hidden="true" />
        </span>
      </DataTooltip>
    </p>}
    <div className={DETAIL_METRICS_GRID} role="group" aria-label="Linked Validator metrics">
      <MetricRow label="Cumulative blocks" value={blockCountLabel(validator.blockCount)} detail={metricNote(counterWarning)} />
      {/* Truncate only the overview, using source digits rather than floating point. */}
      <MetricRow label="Cumulative rewards" value={<span title={rewards}><ExactAmount value={rewards.replace(/([.][0-9]{4})[0-9]+$/, '$1')} muteFraction /></span>} />
      <MetricRow label="Network rank" value={rankLabel(validator)} detail={metricNote(rankWarning)} />
      <MetricRow label="Production rate" value={blockRateLabel(validator, 'detail')} detail={metricNote(rateWarning)} />
      <MetricRow label="PlatScan 24h rate" value={genBlocksRateLabel(validator, 'detail')} />
      <MetricRow label="Delegation reward share" value={delegationRewardShareLabel(validator, 'detail')} />
    </div>
    {emptyNote && <p className="m-0 text-xs text-muted-foreground" role="status">{emptyNote}</p>}
    <div className="mt-2 flex min-w-0 flex-col gap-1">
      <div className="flex min-w-0 items-center gap-2">
        <span className="shrink-0 text-xs text-muted-foreground">Validator ID</span>
        <code className="min-w-0 font-mono text-xs [overflow-wrap:anywhere]">{identifier.length > 24 ? identifier.slice(0, 12) + '…' + identifier.slice(-8) : identifier}</code>
        <DataTooltip as="span" content="Copy full Validator identifier">
          <Button variant="ghost" size="icon-sm" aria-label="Copy full Validator identifier" onClick={() => { void copyIdentifier() }}>
            <Copy className="size-3.5" aria-hidden="true" />
          </Button>
        </DataTooltip>
        <DataTooltip as="span" content={identifierOpen ? 'Hide full ID' : 'Show full ID'}>
          <Button variant="ghost" size="icon-sm" aria-label={identifierOpen ? 'Hide full ID' : 'Show full ID'} aria-expanded={identifierOpen} aria-controls={identifierId} onClick={() => setIdentifierOpen(open => !open)}>
            {identifierOpen ? <EyeOff className="size-3.5" aria-hidden="true" /> : <Eye className="size-3.5" aria-hidden="true" />}
          </Button>
        </DataTooltip>
      </div>
      {identifierOpen && <code id={identifierId} className="min-w-0 select-text rounded-md border border-border bg-background p-2 font-mono text-xs [overflow-wrap:anywhere]" aria-label={'Validator identifier: ' + identifier}>{identifier}</code>}
      <p role="status" aria-label="Identifier copy status" className={cn('m-0 text-xs text-muted-foreground', !copyStatus && 'sr-only')}>{copyStatus}</p>
    </div>
    {validator.rewardAmount != null && <p className="m-0 text-[11px] text-muted-foreground">Amounts use the Network native unit; full reward precision is available in Validator diagnostics.</p>}
    {/* Diagnostics stays collapsed on first load and keeps the shared
        disclosure presentation. Only its own state/provenance rows and group
        padding tighten; the primitive stays unaware of the Validator variant. */}
    <Disclosure
      surface="none"
      className="-mx-4 border-none"
      title="Validator diagnostics"
      description="Provider, freshness, ranking and source details"
    >
      <ValidatorPublicStates node={node} validator={validator} />
      <Provenance validator={validator} />
      <dl className="m-0 min-w-0 text-xs">
        <dt className="text-muted-foreground">Cumulative rewards (full precision)</dt>
        <dd className="m-0 select-text tabular-nums [overflow-wrap:anywhere]"><ExactAmount value={rewards} /></dd>
      </dl>
    </Disclosure>
  </CardX>
}

/**
 * The Home card's Linked Validator region: the six Validator metrics, or the
 * one short empty state that keeps the region's reserved height. No heading,
 * identity, identifier, copy control, Details row or long explanation is
 * rendered here — those moved to Node detail (#154–#158, #173).
 */
function ValidatorCard({ validator }: { validator: PublicValidatorInsight }) {
  if (!hasDisplayableMetrics(validator)) {
    const empty = emptyMetricsState(validator)
    return <section data-slot="linked-validator" data-state={empty.state} className="min-w-0" aria-label="Linked Validator">
      <p data-slot="linked-validator-empty" className="m-0 text-[11px] text-muted-foreground" role="status">{empty.text}</p>
    </section>
  }
  const cumulativeRewards = formatAmountCompact(validator.rewardAmount)
  return <section data-slot="linked-validator" className="min-w-0 border-t border-border pt-3" aria-label="Linked Validator">
    <div data-slot="linked-validator-metrics" role="group" aria-label="Linked Validator metrics">
      <ValidatorMetric label="Cumulative blocks" value={blockCountLabel(validator.blockCount)} reason="No cumulative block count is available from the Validator source." />
      <ValidatorMetric label="Cumulative rewards" value={cumulativeRewards} reason="No cumulative reward amount is available from the Validator source." />
      <ValidatorParamRow label="Network rank" shortLabel="Rank" value={rankLabel(validator)} reason={rankNote(validator) ?? 'No Network rank is available.'} />
      <ValidatorParamRow label="Production rate" shortLabel="Production" value={blockRateLabel(validator, 'card')} reason="The source has not supplied a complete cumulative produced/scheduled block pair." />
      <ValidatorParamRow label="PlatScan 24h rate" shortLabel="PlatScan 24h" value={genBlocksRateLabel(validator, 'card')} reason="No PlatScan 24-hour block production rate is available." />
      <ValidatorParamRow label="Delegation reward share" shortLabel="Delegation share" value={delegationRewardShareLabel(validator, 'card')} reason="No effective delegation reward distribution ratio is available." />
    </div>
  </section>
}

/**
 * The two emphasized cumulative cells (Cumulative blocks and rewards) keep the
 * label-over-value form. A missing value uses the em-dash placeholder with its
 * reason announced to assistive technology, and a long compact reward can span
 * the full group width rather than splitting. The ordinary parameters use the
 * compact key-value row instead (ValidatorParamRow).
 */
function ValidatorMetric({ label, value, reason }: { label: string; value: string; reason: string }) {
  const descriptionId = useId()
  const unknown = value === 'Unknown'
  // The two emphasized cumulative cells keep the label-over-value form and
  // span the full group width only when a long compact reward needs it.
  return <div data-slot="validator-metric" className={cn('min-w-0', value.length > 15 && 'col-span-2')}>
    <span className="block pb-1 text-xs leading-4 text-muted-foreground">{label}</span>
    <div className="min-w-0">
      <strong className={cn('block text-sm font-semibold leading-5 tabular-nums text-foreground', value.length > 15 ? '[overflow-wrap:anywhere]' : 'whitespace-nowrap')} aria-describedby={unknown ? descriptionId : undefined}>
        {unknown ? '—' : <ExactAmount value={value} />}
      </strong>
      {unknown && <small id={descriptionId} className="block text-[11px] text-muted-foreground">
        Unknown<span className="sr-only">: {reason}</span>
      </small>}
    </div>
  </div>
}

/**
 * Card-only compact key-value parameter: the full field name stays available
 * (the short label carries it as an accessible name and the Node detail view
 * prints it), and a missing value is an explicit Unknown whose sanitized reason
 * is announced, never a zero. Ordinary parameters use this row; only the two
 * cumulative metrics keep the emphasized label-over-value cell above.
 */
function ValidatorParamRow({ label, shortLabel, value, reason }: { label: string; shortLabel: string; value: string; reason: string }) {
  const descriptionId = useId()
  const unknown = value === 'Unknown'
  return <MetricRow
    layout="compact"
    label={label}
    shortLabel={shortLabel}
    value={unknown
      ? <span aria-describedby={descriptionId}>Unknown<span id={descriptionId} className="sr-only">: {reason}</span></span>
      : <ExactAmount value={value} />}
  />
}

/** The full accessible status vocabulary for the Node detail region. */
function ValidatorPublicStates({ node, validator }: { node: PublicNode; validator: PublicValidatorInsight }) {
  const states = [
    ['Current staking status', validator.currentValidatorStatus],
    ['Staking status state', validator.currentValidatorStatusState],
    ['Staking qualifier', validator.currentValidatorStatusQualifier],
    ['Provider state', validator.state], ['Data freshness', validator.freshness],
    ['Rank state', validator.rankState], ['Rank freshness', validator.rankFreshness],
    ['Production rate state', validator.blockRateState], ['Counter state', validator.counterState],
    ['Activity', validator.activity], ['Activity state', validator.activityState],
    ['Identity state', node.validatorIdentityState],
  ]
  return <dl className="m-0 grid grid-cols-2 gap-x-3 gap-y-2 border-t border-border pt-2" aria-label="Public Validator states">
    {states.map(([label, value]) => <div key={label} className="min-w-0">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="m-0 break-words text-xs">{value ?? 'Not provided'}</dd>
    </div>)}
  </dl>
}

function Provenance({ validator }: { validator: PublicValidatorInsight }) {
  const fact = (label: string, value: string) => <div key={label} className="min-w-0">
    <dt className="text-[11px] font-medium tracking-wider text-muted-foreground">{label}</dt>
    <dd className="m-0 break-words text-xs">{value}</dd>
  </div>
  return <dl className="m-0 grid grid-cols-[repeat(auto-fit,minmax(9rem,1fr))] gap-x-4 gap-y-1.5 border-t border-dashed border-border/60 pt-2">
    {fact('Last success', validator.receivedAt ? formatUtcDateTime(validator.receivedAt) : 'Never observed')}
    {/* The ranking list is fetched independently, so it has its own success time. */}
    {fact('Rank last success', validator.rankReceivedAt ? formatUtcDateTime(validator.rankReceivedAt) : 'Never observed')}
    {fact('Rank cohort', validator.rankCohortSize != null ? `${validator.rankCohortSize}` : 'Unknown')}
    {fact('Source', validator.source ?? 'Unknown')}
    {fact('Source cutoff', validator.providerTimestamp ? formatUtcDateTime(validator.providerTimestamp) : 'Not provided by the source')}
  </dl>
}
