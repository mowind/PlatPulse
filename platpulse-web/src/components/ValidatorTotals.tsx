import type {
  PublicNetwork,
  PublicValidatorBlockTotal,
  PublicValidatorRewardTotal,
  PublicValidatorSummary,
} from '../api/generated'
import { formatAmountCompact, formatAmountExact } from '../lib/amount'
import { MetricRow } from './MetricRow'
import { SURFACE_CARD } from '../lib/surface'
import { cn } from '../lib/utils'

type Coverage = {
  expectedCount: number
  valuedCount: number
  staleCount: number
  state: string
}

/** Coverage line for one metric. Blocks and rewards are reported
 *  independently: knowing all block counts says nothing about reward
 *  coverage (#159). */
function coverageLabel(total: Coverage): string {
  const base = `${total.valuedCount}/${total.expectedCount} Validators with values`
  return total.staleCount > 0 ? `${base} · ${total.staleCount} stale` : base
}

/** A total with no known values is Unknown, never a fabricated zero. */
function blockTotalValue(total: PublicValidatorBlockTotal): string {
  return formatAmountExact(total.knownSum)
}

/** Sums arrive as exact decimal strings and are abbreviated only for display;
 *  the browser never re-adds Node projections or round-trips through a float. */
function rewardTotalValue(total: PublicValidatorRewardTotal): string {
  return formatAmountCompact(total.knownSum)
}

function coverageNote(total: Coverage): string | null {
  if (total.state === 'partial') {
    return 'Known values subtotal — not every eligible Validator has a value for this metric.'
  }
  if (total.state === 'unknown') {
    return 'No eligible Validator has a value for this metric; showing Unknown rather than zero.'
  }
  return null
}

function unlinkedLabel(count: number): string {
  return `${count} unlinked ${count === 1 ? 'Node' : 'Nodes'}`
}

function NetworkTotals({ network }: { network: PublicNetwork }) {
  const summary: PublicValidatorSummary | undefined = network.validatorSummary
  if (!summary) return null
  const notes: Array<{ key: string; text: string }> = []
  const blockNote = coverageNote(summary.blocks)
  if (blockNote) notes.push({ key: 'blocks', text: blockNote })
  const rewardNote = coverageNote(summary.rewards)
  if (rewardNote) notes.push({ key: 'rewards', text: rewardNote })
  return (
    <article
      data-slot="validator-summary-network"
      data-network-key={network.networkKey}
      className={cn('min-w-0 rounded-md border-none p-3', SURFACE_CARD)}
      aria-label={`Validator totals for ${network.displayName}`}
    >
      <h3 className="m-0 break-words text-sm font-semibold">{network.displayName}</h3>
      <div className="mt-2 grid grid-cols-1 gap-x-4 sm:grid-cols-2">
        <div className="min-w-0" data-slot="validator-summary-blocks">
          <MetricRow label="Cumulative blocks" value={blockTotalValue(summary.blocks)} />
          <p className="m-0 mt-0.5 text-[11px] text-muted-foreground">{coverageLabel(summary.blocks)}</p>
        </div>
        <div className="min-w-0" data-slot="validator-summary-rewards">
          <MetricRow label="Cumulative rewards" value={rewardTotalValue(summary.rewards)} />
          <p className="m-0 mt-0.5 text-[11px] text-muted-foreground">{coverageLabel(summary.rewards)}</p>
        </div>
      </div>
      {notes.map((note) => (
        <p key={note.key} className="m-0 mt-1 text-[11px] text-muted-foreground" role="status">
          {note.text}
        </p>
      ))}
      <p className="m-0 mt-1 text-[11px] text-muted-foreground" data-slot="validator-summary-unlinked">
        {unlinkedLabel(summary.unlinkedNodeCount)}
      </p>
    </article>
  )
}

/**
 * Home's current-selection Cumulative Validator summary (issue #159).
 *
 * The Server selects the eligible Active Nodes, deduplicates by Network plus
 * Validator, and computes the exact totals and coverage; this section only
 * chooses which already-computed Network groups the Home filter shows. It
 * never adds duplicate Node projections and never groups across Networks.
 */
export function ValidatorTotalsSection({ networks }: { networks: PublicNetwork[] }) {
  const scoped = networks.filter((network) => network.validatorSummary != null && network.nodes.length > 0)
  if (scoped.length === 0) return null
  return (
    <section data-slot="validator-summary" className="mt-4" aria-label="Validator totals">
      <header className="mb-2">
        <h2 className="m-0 text-base font-semibold">Validator totals</h2>
        <p className="m-0 mt-0.5 text-xs text-muted-foreground">
          Current selection, grouped by Network · deduplicated by Validator
        </p>
      </header>
      <div className="grid grid-cols-1 gap-3 lg:grid-cols-2">
        {scoped.map((network) => (
          <NetworkTotals key={network.networkKey} network={network} />
        ))}
      </div>
    </section>
  )
}
