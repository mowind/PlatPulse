import type { ReactNode } from 'react'
import type { AdminValidatorInsight, AdminValidatorHistoryEntry, PublicValidatorInsight, PublicValidatorHistoryEntry } from '../api/generated'
import { ValidatorHistory } from './ValidatorHistory'
import { StatusBadge } from './StatusBadge'
import { DataTooltip } from './ui/data-tooltip'
import { cn } from '../lib/utils'

type Insight = PublicValidatorInsight | AdminValidatorInsight

function stateLabel(state: string): string {
  switch (state) {
    case 'fresh': return 'Fresh'
    case 'stale': return 'Stale'
    case 'error': return 'Error'
    case 'not_configured': return 'Not configured'
    case 'unsupported': return 'Unsupported'
    case 'not_found': return 'Not found'
    case 'empty': return 'Empty'
    default: return 'Unknown'
  }
}

function stateTone(state: string): 'ok' | 'warning' | 'error' | 'neutral' {
  switch (state) {
    case 'fresh': return 'ok'
    case 'stale': return 'warning'
    case 'error': return 'error'
    case 'unsupported': return 'warning'
    case 'not_found': return 'neutral'
    case 'empty': return 'neutral'
    default: return 'neutral'
  }
}

/** Long provider values stay visible in the DOM but are truncated visually,
 * with a touch/keyboard-capable DataTooltip carrying the full value. */
function LongValue({ value }: { value: string | null | undefined }) {
  if (value == null) return <>Unknown</>
  return (
    <DataTooltip content={value} as="span" className="block w-full min-w-0">
      <span className="block w-full truncate" tabIndex={0}>{value}</span>
    </DataTooltip>
  )
}

function Fact({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs font-medium tracking-wider text-muted-foreground">{label}</dt>
      <dd className="m-0 break-words text-sm">{value}</dd>
    </div>
  )
}

export function ValidatorInsight({ insight, compact = false, history }: { insight: Insight; compact?: boolean; history?: Array<AdminValidatorHistoryEntry | PublicValidatorHistoryEntry> }) {
  return (
    <section className={cn('min-w-0', compact && 'text-sm')} aria-label="Validator insight">
      <p className="m-0 flex flex-wrap items-center gap-2">
        <StatusBadge status={stateLabel(insight.state)} tone={stateTone(insight.state)} />
        <span className="text-[11px] text-muted-foreground">{insight.freshness === 'unknown' ? 'Never observed' : insight.freshness}</span>
      </p>
      <dl className="m-0 mt-3 grid grid-cols-[repeat(auto-fit,minmax(10rem,1fr))] gap-x-4 gap-y-2">
        <Fact label="Validator" value={insight.displayName || insight.validatorNodeId} />
        <Fact label="Rank" value={insight.rank ?? 'Unknown'} />
        <Fact label="Stake" value={<LongValue value={insight.stakeAmount} />} />
        <Fact label="Reward rate" value={<LongValue value={insight.rewardRate} />} />
        <Fact label="Blocks" value={insight.blockCount ?? 'Unknown'} />
      </dl>
      {insight.counterState === 'counter_reset' && (
        <p className="m-0 mt-3 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive" role="status">
          Counter reset or correction observed; prior value was not treated as normal growth.
        </p>
      )}
      {history && <ValidatorHistory entries={history} compact={compact} />}
    </section>
  )
}
