import type { PublicNode, PublicValidatorInsight } from '../api/generated'
import { formatAmountCompact, formatAmountExact } from '../lib/amount'
import { formatRatePercent2, formatRatePercentExact } from '../lib/rate'
import { MetricRow } from './MetricRow'
import { CardX } from './ui/card-x'
import { formatUtcDateTime } from './StatusBadge'
import { SURFACE_CARD } from '../lib/surface'
import { cn } from '../lib/utils'

/** Human label for the effective Node Validator Link role. The role never
 *  changes which Validator identity the metrics describe; it only describes
 *  how this Node participates in the shared Validator (#154). */
export function validatorRoleLabel(role: string | null | undefined): string {
  switch ((role ?? '').toLowerCase()) {
    case 'primary': return 'Primary'
    case 'standby': return 'Standby'
    case 'observer': return 'Observer'
    default: return 'Role unknown'
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

/** Text carries the meaning; this dot only supplements it (design §2.1). */
function stateDotClass(state: string): string {
  switch (state) {
    case 'fresh': return 'bg-emerald-500'
    case 'stale':
    case 'not_configured':
    case 'unsupported': return 'bg-amber-500'
    case 'error': return 'bg-destructive'
    default: return 'bg-muted-foreground'
  }
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
 * The extensible linked-Validator area shared by the Home Node card and Node
 * detail. It owns the cumulative Validator block count, gross cumulative
 * rewards, the Network-scoped PlatScan rank, the Server-computed cumulative
 * production rate, PlatScan's own 24-hour rate, and the currently effective
 * delegation reward distribution percentage, plus the explicit unlinked /
 * not-configured / never-observed / stale / unranked / not-applicable /
 * retained states; later Validator metrics extend the same metric group rather
 * than adding a second region (#154, #155, #156, #157, #158).
 */
export function LinkedValidatorSection({ node, variant = 'card' }: { node: PublicNode; variant?: 'card' | 'detail' }) {
  const validator = node.validator
  if (!validator) {
    return <section data-slot="linked-validator" className="min-w-0 border-t border-border pt-3" aria-label="Linked Validator">
      <h3 className="m-0 text-xs font-medium tracking-wider text-muted-foreground">Linked Validator</h3>
      <p className="m-0 mt-1 text-[11px] text-muted-foreground" role="status">Unlinked — no effective Validator Link is configured for this Node.</p>
    </section>
  }

  const state = validatorStateLabel(validator.state, validator.freshness)
  const note = stateNote(validator)
  const rankState = rankNote(validator)
  const retained = validator.state !== 'fresh' && validator.state !== 'stale' && (validator.blockCount != null || validator.rewardAmount != null || validator.rank != null || validator.blockRate != null || validator.genBlocksRate != null || validator.delegationRewardPercentage != null)
  // Detail exposes every digit the source provided; cards may abbreviate, but
  // both come from the same exact-decimal string and never through a float.
  const cumulativeRewards = variant === 'detail'
    ? formatAmountExact(validator.rewardAmount)
    : formatAmountCompact(validator.rewardAmount)
  const body = <>
    <header className="flex min-w-0 flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
      <h3 className="m-0 text-xs font-medium tracking-wider text-muted-foreground">Linked Validator</h3>
      <span className="shrink-0 rounded-full border border-border px-2 py-0.5 text-[10px] font-semibold text-muted-foreground">{validatorRoleLabel(validator.linkRole)}</span>
    </header>
    <p className="m-0 mt-1 min-w-0 break-words text-sm font-semibold">{validator.displayName || validator.validatorNodeId}</p>
    <p className="m-0 mt-1 flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground" role="status">
      <span className={cn('inline-block size-1.5 shrink-0 rounded-full', stateDotClass(validator.state))} aria-hidden="true" />
      <span className="font-medium text-foreground">{state}</span>
      {validator.state === 'stale' && validator.freshness === 'stale' && <span>· last successful value retained</span>}
    </p>
    {note && <p className="m-0 mt-1 text-[11px] text-muted-foreground">{note}</p>}
    <div className="mt-2 grid grid-cols-[repeat(auto-fit,minmax(9rem,1fr))] gap-x-4" role="group" aria-label="Linked Validator metrics">
      <MetricRow label="Cumulative blocks" value={blockCountLabel(validator.blockCount)} />
      <MetricRow label="Cumulative rewards" value={cumulativeRewards} />
      <MetricRow label="Network rank" value={rankLabel(validator)} />
      <MetricRow label="Production rate" value={blockRateLabel(validator, variant)} />
      <MetricRow label="PlatScan 24h rate" value={genBlocksRateLabel(validator, variant)} />
      <MetricRow label="Delegation reward share" value={delegationRewardShareLabel(validator, variant)} />
    </div>
    {rankState && <p className="m-0 mt-1 text-[11px] text-muted-foreground" role="status">{rankState}</p>}
    <p className="m-0 mt-1 text-[11px] text-muted-foreground">Rank is PlatScan's position within this Network's complete live-staking ALL cohort, including candidates; it is adopted from the source and is never recomputed from the monitored Nodes or Home filters.</p>
    <p className="m-0 mt-1 text-[11px] text-muted-foreground">Production rate is cumulative actual ÷ cumulative scheduled blocks from the same observation; whole-round duties count before they elapse, so it is not an exact missed-block rate.</p>
    {validator.blockRateState === 'not_applicable' && <p className="m-0 mt-0.5 text-[11px] text-muted-foreground" role="status">The source reported a zero scheduled-block denominator, so a rate is not applicable — not 0%.</p>}
    <p className="m-0 mt-0.5 text-[11px] text-muted-foreground">PlatScan 24h rate uses PlatScan口径: the preceding seven settlement periods excluding the current one, so it is not a strict rolling 24 hours, and a source 0% can also mean insufficient evidence or an upstream error.</p>
    <p className="m-0 mt-0.5 text-[11px] text-muted-foreground">Delegation reward share is the currently effective proportion of applicable Validator rewards allocated to delegators; it is not annualized yield, operator commission, or the pending next-period ratio.</p>
    {validator.rewardAmount != null && <>
      <p className="m-0 mt-1 text-[11px] text-muted-foreground">Cumulative rewards are gross: they include the operator and delegator allocations and are not operator net earnings.</p>
      {variant === 'detail' && <p className="m-0 mt-0.5 text-[11px] text-muted-foreground">Amounts use the Network native unit; detail shows all precision the source provides.</p>}
    </>}
    {validator.counterState === 'counter_reset' && <p className="m-0 mt-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive" role="status">Counter reset or correction observed; the prior value was not treated as normal growth.</p>}
    {retained && <p className="m-0 mt-1 text-[11px] text-muted-foreground">Showing the last successful value; the current source state is unavailable.</p>}
    {variant === 'detail' && <Provenance validator={validator} />}
  </>

  if (variant === 'detail') {
    return <CardX
      bordered={false}
      role="region"
      aria-label="Linked Validator"
      data-slot="linked-validator"
      className={cn('min-w-0 rounded-md border-none', SURFACE_CARD)}
      contentClassName="flex min-w-0 flex-col gap-2"
    >{body}</CardX>
  }
  return <section data-slot="linked-validator" className="min-w-0 border-t border-border pt-3" aria-label="Linked Validator">{body}</section>
}

function Provenance({ validator }: { validator: PublicValidatorInsight }) {
  const fact = (label: string, value: string) => <div key={label} className="min-w-0">
    <dt className="text-[11px] font-medium tracking-wider text-muted-foreground">{label}</dt>
    <dd className="m-0 break-words text-xs">{value}</dd>
  </div>
  return <dl className="m-0 grid grid-cols-[repeat(auto-fit,minmax(9rem,1fr))] gap-x-4 gap-y-2 border-t border-dashed border-border/60 pt-2">
    {fact('Last success', validator.receivedAt ? formatUtcDateTime(validator.receivedAt) : 'Never observed')}
    // The ranking list is fetched independently, so it has its own success time.
    {fact('Rank last success', validator.rankReceivedAt ? formatUtcDateTime(validator.rankReceivedAt) : 'Never observed')}
    {fact('Rank cohort', validator.rankCohortSize != null ? `${validator.rankCohortSize}` : 'Unknown')}
    {fact('Source', validator.source ?? 'Unknown')}
    {fact('Source cutoff', validator.providerTimestamp ? formatUtcDateTime(validator.providerTimestamp) : 'Not provided by the source')}
  </dl>
}
