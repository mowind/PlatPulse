import type { PublicNetwork } from '../api/generated'
import { Blocks, Coins, Info } from 'lucide-react'
import { formatAmountExact, formatAmountOverview, sumKnownAmounts } from '../lib/amount'
import { ExactAmount } from './ExactAmount'
import { HomeSummaryCard } from './HomeSummaryCard'
import { Button } from './ui/button'
import { Dialog, DialogTrigger, DialogContent, DialogHeader, DialogTitle, DialogDescription } from './ui/dialog'

type Metric = 'blocks' | 'rewards'
type Coverage = { expectedCount: number; valuedCount: number; staleCount: number; state: string }

function coverageLabel(total: Coverage): string {
  const base = total.valuedCount + '/' + total.expectedCount + ' Validators with values'
  return total.staleCount > 0 ? base + ' · ' + total.staleCount + ' stale' : base
}

function coverageNote(total: Coverage): string | null {
  if (total.state === 'partial') return 'Known values subtotal — not every eligible Validator has a value for this metric.'
  if (total.state === 'unknown') return 'No eligible Validator has a value for this metric; showing Unknown rather than zero.'
  return null
}

/** Sum only Server-owned, already deduplicated Network groups. Never Node values.
 * The explicitly requested cross-Network overview is numerical, not asset conversion;
 * Network remains part of Validator identity even when identifiers match. */
export function selectionTotal(networks: PublicNetwork[], metric: Metric) {
  const scoped = networks.filter(network => network.nodes.length > 0)
  const totals = scoped.flatMap(network => network.validatorSummary ? [network.validatorSummary[metric]] : [])
  const missingNetworks = scoped.length - totals.length
  const knownSum = sumKnownAmounts(totals.map(total => total.knownSum))
  const expectedCount = totals.reduce((sum, total) => sum + total.expectedCount, 0)
  const valuedCount = totals.reduce((sum, total) => sum + total.valuedCount, 0)
  const staleCount = totals.reduce((sum, total) => sum + total.staleCount, 0)
  // Stale contributors are labelled separately: they do not make complete
  // coverage look partial. Partial means some eligible Validator has no value.
  const state = knownSum === null ? 'unknown' : missingNetworks > 0 || valuedCount < expectedCount ? 'partial' : 'complete'
  return { knownSum, expectedCount, valuedCount, staleCount, state, missingNetworks }
}

function NetworkTotals({ network }: { network: PublicNetwork }) {
  const summary = network.validatorSummary
  return <article data-slot="validator-summary-network" data-network-key={network.networkKey}
    className="min-w-0 border-t border-border pt-3" aria-label={'Validator totals for ' + network.displayName}>
    <h3 className="m-0 text-sm font-semibold [overflow-wrap:anywhere]">{network.displayName}</h3>
    {!summary ? <p className="text-xs text-muted-foreground">Summary unavailable; values and coverage are unknown.</p> : <>
      <dl className="mt-2 grid min-w-0 grid-cols-1 gap-3 sm:grid-cols-2">
        {(['blocks', 'rewards'] as const).map(metric => <div key={metric} className="min-w-0" data-slot={'validator-summary-' + metric}>
          <dt className="text-xs text-muted-foreground">{metric === 'blocks' ? 'Cumulative blocks' : 'Cumulative rewards'}</dt>
          <dd className="m-0 mt-1 text-sm font-medium tabular-nums [overflow-wrap:anywhere]">{formatAmountExact(summary[metric].knownSum)}</dd>
          <p className="mt-1 text-xs text-muted-foreground">{coverageLabel(summary[metric])}</p>
          {coverageNote(summary[metric]) && <p className="mt-1 text-xs text-muted-foreground">{coverageNote(summary[metric])}</p>}
        </div>)}
      </dl>
      <p className="mt-2 text-xs text-muted-foreground" data-slot="validator-summary-unlinked">{summary.unlinkedNodeCount} unlinked {summary.unlinkedNodeCount === 1 ? 'Node' : 'Nodes'}</p>
      <p className="mt-1 text-xs text-muted-foreground">{summary.linkedNodeCount} linked Nodes · {summary.eligibleValidatorCount} eligible Validators</p>
    </>}
  </article>
}

export function ValidatorTotalCard({ networks, metric, availability = 'ready' }: {
  networks: PublicNetwork[]
  metric: Metric
  availability?: 'ready' | 'loading' | 'unavailable'
}) {
  const total = selectionTotal(availability === 'ready' ? networks : [], metric)
  const label = metric === 'blocks' ? 'Cumulative blocks' : 'Cumulative rewards'
  // Cumulative block counts always print the complete number. Reward amounts
  // keep the abbreviated tile form; the Breakdown exposes their exact digits.
  const exact = formatAmountExact(total.knownSum)
  const scope = networks.length === 1 ? networks[0].displayName : networks.length + ' Networks'
  const populated = networks.filter(network => network.nodes.length > 0).length
  // The card face stays a number-only tile. Scope, coverage and staleness are
  // read from the accessible Breakdown, which also carries the per-Network
  // evidence; nothing here is delegated to a hover-only tooltip.
  const coverageSummary = total.missingNetworks > 0 && total.missingNetworks === populated
    ? 'Coverage unknown'
    : total.valuedCount + '/' + total.expectedCount + ' known'
      + (total.missingNetworks > 0 ? ' (reported)' : '')
      + (total.state === 'partial' ? ' · Partial' : '')
      + (total.state === 'unknown' ? ' · Unknown' : '')
      + (total.staleCount > 0 ? ' · ' + total.staleCount + ' stale' : '')
  const shown = metric === 'blocks' ? <ExactAmount value={exact} split /> : formatAmountOverview(total.knownSum)
  return <HomeSummaryCard label={label} value={shown} icon={metric === 'blocks' ? Blocks : Coins}
    action={<Dialog>
      <DialogTrigger asChild><Button variant="ghost" size="icon" className="-mr-2 shrink-0" aria-label={label + ' breakdown and exact values'}><Info className="size-[18px]" strokeWidth={2} aria-hidden="true" data-icon={label} /></Button></DialogTrigger>
      <DialogContent className="max-h-[85dvh] overflow-y-auto rounded-md shadow-sm">
        <DialogHeader className="pr-10 text-left">
          <DialogTitle>{label} — Breakdown</DialogTitle>
          <DialogDescription>Current selection, grouped by Network · deduplicated by Validator within each Network.</DialogDescription>
        </DialogHeader>
        <p className="text-sm">Exact known-value {total.state === 'partial' ? 'subtotal' : 'sum'}: <strong className="tabular-nums [overflow-wrap:anywhere]">{exact}</strong></p>
        <dl className="m-0 grid grid-cols-1 gap-2 text-xs sm:grid-cols-2" data-slot="validator-overview-scope">
          <div className="min-w-0"><dt className="text-muted-foreground">Scope</dt><dd className="m-0 break-words">{scope}</dd></div>
          <div className="min-w-0"><dt className="text-muted-foreground">Coverage</dt><dd className="m-0 break-words">{coverageSummary}</dd></div>
        </dl>
        {total.missingNetworks > 0 && <p className="text-xs text-muted-foreground" data-slot="validator-overview-missing">{total.missingNetworks} Network {total.missingNetworks === 1 ? 'summary' : 'summaries'} unavailable</p>}
        <p className="text-xs text-muted-foreground">{networks.length > 1
          ? 'Cross-Network numerical sum. Rewards are added in each Network’s native unit without asset conversion; this is not a single-asset balance or monetary valuation.'
          : 'Amounts use the Network native unit; no asset conversion is applied.'}</p>
        <p className="text-xs text-muted-foreground">Each Network summary comes from the Server. An eligible Validator is counted once per Network even when linked to multiple Active Nodes. Blocks and rewards have independent coverage; missing values are not zero. Retained last-good values remain included and stale contributors are counted. Unlinked Nodes are reported separately, outside the eligible-Validator denominator.</p>
        {availability !== 'ready' ? <p role="status">{availability === 'loading' ? 'Loading Validator summaries…' : 'Validator summaries unavailable.'}</p> : <div className="space-y-4" data-slot="validator-breakdown">
          {networks.filter(network => network.nodes.length > 0).map(network => <NetworkTotals key={network.networkKey} network={network} />)}
          {networks.length === 0 && <p>No Networks in the current selection; no known values.</p>}
        </div>}
      </DialogContent>
    </Dialog>}>
  </HomeSummaryCard>
}
