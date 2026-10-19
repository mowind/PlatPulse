import type { PublicNetwork } from '../api/generated'
import { Blocks, Coins, Info } from 'lucide-react'
import { formatAmountExact, formatAmountOverview, sumKnownAmounts } from '../lib/amount'
import { ExactAmount } from './ExactAmount'
import { SummaryMetricCard } from './SummaryMetricCard'
import { Button } from './ui/button'
import { Dialog, DialogTrigger, DialogContent, DialogHeader, DialogTitle, DialogDescription } from './ui/dialog'

type Metric = 'blocks' | 'rewards'
/** One metric's own Server-owned coverage inside a Network's summary. */
type MetricCoverage = { expectedCount: number; valuedCount: number; staleCount: number; state: string }
/** The same coverage added across the selected Networks, which is why it also
 *  knows how many populated Networks published no summary at all. */
type SelectionCoverage = MetricCoverage & { missingNetworks: number }

/** One short valued/expected count in the ticket's own words: `5/8 reporting`
 * counts the eligible Validators of the Networks that published a summary and
 * carry a value for this metric. A denominator of zero is not a `0/0` score,
 * because there is nothing to be covered. */
function reportingLabel(total: MetricCoverage): string {
  return total.expectedCount > 0
    ? total.valuedCount + '/' + total.expectedCount + ' reporting'
    : 'No eligible Validators'
}

/** What the cumulative tile and its Breakdown both state about coverage, before
 * the selection's own scope: the reporting count, then what it cannot claim. A
 * Network whose summary the Server did not publish is named beside the count
 * instead of being counted as covered — the reported denominator never stands
 * for the whole selection — and an Unknown subtotal is named as such rather
 * than read as a zero. Display only: `selectionTotal` decides every number. */
function cumulativeCoverage(total: SelectionCoverage, populated: number): string {
  if (populated > 0 && total.missingNetworks >= populated) return 'Coverage unknown'
  const parts = [reportingLabel(total)]
  if (total.missingNetworks > 0) parts.push(missingSummaryLabel(total.missingNetworks))
  if (total.state === 'unknown') parts.push('Unknown')
  if (total.staleCount > 0) parts.push(total.staleCount + ' stale')
  return parts.join(' · ')
}

/** The same count inside one Network's own row of the Breakdown. A Network's
 * own stale contributors are named here, where the Network is the subject. */
function networkCoverage(total: MetricCoverage): string {
  return reportingLabel(total) + (total.staleCount > 0 ? ' · ' + total.staleCount + ' stale' : '')
}

/** The count and plural of the summaries the Server published none of, shared
 *  by the coverage line and the Breakdown sentence so the two cannot drift. */
function missingSummaryLabel(count: number): string {
  return count + ' Network ' + (count === 1 ? 'summary' : 'summaries') + ' missing'
}

function coverageNote(total: MetricCoverage): string | null {
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
          <p className="mt-1 text-xs text-muted-foreground">{networkCoverage(summary[metric])}</p>
          {coverageNote(summary[metric]) && <p className="mt-1 text-xs text-muted-foreground">{coverageNote(summary[metric])}</p>}
        </div>)}
      </dl>
      <p className="mt-2 text-xs text-muted-foreground" data-slot="validator-summary-unlinked">{summary.unlinkedNodeCount} unlinked {summary.unlinkedNodeCount === 1 ? 'Node' : 'Nodes'}</p>
      <p className="mt-1 text-xs text-muted-foreground">{summary.linkedNodeCount} linked Nodes · {summary.eligibleValidatorCount} eligible Validators</p>
    </>}
  </article>
}

export function ValidatorTotalCard({ networks, metric, availability = 'ready', listNarrowed = false }: {
  networks: PublicNetwork[]
  metric: Metric
  availability?: 'ready' | 'loading' | 'unavailable'
  /** Whether a search or status filter narrows the Home list below the Network
   * selection. #233: the cumulative totals stay Network-level even then, so the
   * card says so instead of appearing to follow the list (#245: in a short
   * scope word on the face and one plain sentence in the Breakdown). */
  listNarrowed?: boolean
}) {
  const total = selectionTotal(availability === 'ready' ? networks : [], metric)
  const label = metric === 'blocks' ? 'Cumulative blocks' : 'Cumulative rewards'
  // Cumulative block counts always print the complete number. Reward amounts
  // keep the abbreviated tile form; the Breakdown exposes their exact digits.
  const exact = formatAmountExact(total.knownSum)
  const scope = networks.length === 1 ? networks[0].displayName : networks.length + ' Networks'
  // While a filter narrows the list, the face names the whole scope in one word
  // and the Breakdown says in full which filters leave this total alone.
  const scopeNoted = listNarrowed && networks.length > 0
  const faceScope = scopeNoted ? 'All of ' + scope : scope
  const scopeNote = scopeNoted
    ? 'This total covers every Active Node in ' + (networks.length === 1 ? scope : 'the ' + networks.length + ' selected Networks')
      + '; search and the Health and Validator status filters do not narrow it.'
    : null
  const populated = networks.filter(network => network.nodes.length > 0).length
  // One coverage vocabulary for the tile face and the Breakdown: the same
  // Server-owned per-metric coverage text, never a hover-only tooltip, and every
  // number it prints still comes from the Server's own summary. While the
  // summaries are still loading or could not be read, there is no denominator to
  // state, so both surfaces say exactly that instead of turning the empty input
  // into an authoritative 'No eligible Validators'.
  const coverage = availability === 'ready'
    ? cumulativeCoverage(total, populated)
    : (availability === 'loading' ? 'Loading coverage…' : 'Coverage unavailable')
  // The tile face prints its own Network scope followed by that same coverage
  // text, so the scope and the metric's own count are readable without opening
  // the Breakdown. A load/unavailable result still names the scope it is waiting
  // on rather than dropping it.
  const caption = [availability === 'ready' || networks.length > 0 ? faceScope : null, coverage]
    .filter(Boolean).join(' · ')
  const shown = metric === 'blocks' ? <ExactAmount value={exact} split /> : formatAmountOverview(total.knownSum)
  // The marker slot owns the placement of this control (SummaryMetricCard); the
  // trigger carries no offset of its own, so all six glyphs line up.
  return <SummaryMetricCard label={label} value={shown} caption={caption} icon={metric === 'blocks' ? Blocks : Coins}
    action={<Dialog>
      <DialogTrigger asChild><Button variant="ghost" size="icon" className="shrink-0" aria-label={label + ' breakdown and exact values'}><Info className="size-[18px]" strokeWidth={2} aria-hidden="true" data-icon={label} /></Button></DialogTrigger>
      <DialogContent className="max-h-[85dvh] overflow-y-auto rounded-md shadow-sm">
        <DialogHeader className="pr-10 text-left">
          <DialogTitle>{label} — Breakdown</DialogTitle>
          <DialogDescription>Current selection, grouped by Network · deduplicated by Validator within each Network.</DialogDescription>
        </DialogHeader>
        <p className="text-sm">Exact known-value {total.state === 'partial' ? 'subtotal' : 'sum'}: <strong className="tabular-nums [overflow-wrap:anywhere]">{exact}</strong></p>
        <dl className="m-0 grid grid-cols-1 gap-2 text-xs sm:grid-cols-2" data-slot="validator-overview-scope">
          <div className="min-w-0"><dt className="text-muted-foreground">Scope</dt><dd className="m-0 break-words">{scope}</dd></div>
          <div className="min-w-0"><dt className="text-muted-foreground">Coverage</dt><dd className="m-0 break-words">{coverage}</dd></div>
        </dl>
        {total.missingNetworks > 0 && <p className="text-xs text-muted-foreground" data-slot="validator-overview-missing">{missingSummaryLabel(total.missingNetworks)} from this coverage.</p>}
        {scopeNote && <p className="text-xs text-muted-foreground" data-slot="validator-overview-note">{scopeNote}</p>}
        <p className="text-xs text-muted-foreground">{networks.length > 1
          ? 'Cross-Network numerical sum. Rewards are added in each Network’s native unit without asset conversion; this is not a single-asset balance or monetary valuation.'
          : 'Amounts use the Network native unit; no asset conversion is applied.'}</p>
        <p className="text-xs text-muted-foreground">“5/8 reporting” counts one eligible Validator once per Network, across the Networks whose summary the Server published; that reported denominator never claims the whole selection, and a Network whose summary is missing is named beside it, with its Validators neither zero nor covered. Each Network summary comes from the Server. An eligible Validator is counted once per Network even when linked to multiple Active Nodes. Blocks and rewards have independent coverage; missing values are not zero. Retained last-good values remain included and stale contributors are counted. Unlinked Nodes are reported separately, outside the eligible-Validator denominator.</p>
        {availability !== 'ready' ? <p role="status">{availability === 'loading' ? 'Loading Validator summaries…' : 'Validator summaries unavailable.'}</p> : <div className="space-y-4" data-slot="validator-breakdown">
          {networks.filter(network => network.nodes.length > 0).map(network => <NetworkTotals key={network.networkKey} network={network} />)}
          {networks.length === 0 && <p>No Networks in the current selection; no known values.</p>}
        </div>}
      </DialogContent>
    </Dialog>}>
  </SummaryMetricCard>
}
