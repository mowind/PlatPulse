import { useMemo, useState } from 'react'
import { Link } from 'react-router'
import type { PublicConsensusInsight, PublicNetwork, PublicNode } from '../api/generated'
import { realtimeStreamLabel } from './RealtimeNotice'
import { peerInsightCollectionStatus, peerInsightFreshnessStatus, peerInsightValueStatus } from './PeerInsight'
import { NodeHealthMarker } from './StatusBadge'
import GeoMapBoundary from './GeoMapBoundary'
import GeoWorldMap from './GeoWorldMap'
import { formatNodeDataBytes } from '../formatBytes'
import { formatDuration } from '../formatDuration'
import { nodeDataProgress } from '../nodeData'
import { MetricRow } from './MetricRow'
import { CardX } from './ui/card-x'
import { Alert, AlertDescription } from './ui/alert'
import { Empty } from './ui/empty'
import { Select } from './ui/input'
import { Tabs, TabsList, TabsTrigger } from './ui/tabs'
import { Server, HeartPulse, TriangleAlert, Network, ChevronUp, ChevronDown, Info } from 'lucide-react'
import { SURFACE_TOOLBAR } from '../lib/surface'
import { cn } from '../lib/utils'
import { LinkedValidatorSection } from './LinkedValidator'
import { ValidatorTotalCard } from './ValidatorTotals'
import { HomeSummaryCard } from './HomeSummaryCard'
import { Button } from './ui/button'
import { Dialog, DialogTrigger, DialogContent, DialogTitle, DialogDescription } from './ui/dialog'

type HomeDashboardProps = {
  networks: PublicNetwork[]
  realtimeStatus: 'connecting' | 'connected' | 'disconnected'
  online: boolean
  resetting: boolean
  error: string | null
  hasLastGood?: boolean
  loading: boolean
}

type NodeRecord = { network: PublicNetwork; node: PublicNode }
type SortKey = 'health' | 'name' | 'head'

const sortOptions: Array<{ value: SortKey; label: string }> = [
  { value: 'health', label: 'Health' },
  { value: 'name', label: 'Name' },
  { value: 'head', label: 'Current Head' },
]

/** Emerald Home: six equal overview cards beside a proportional Peer map.
 * Node cards keep their independent, content-width-driven column rules. */
export default function HomeDashboard({
  networks,
  realtimeStatus,
  online,
  error,
  hasLastGood = true,
  loading,
}: HomeDashboardProps) {
  const [networkFilter, setNetworkFilter] = useState('all')
  const [sortBy, setSortBy] = useState<SortKey>('health')
  const records = useMemo<NodeRecord[]>(
    () => networks.flatMap((network) => network.nodes.map((node) => ({ network, node }))),
    [networks],
  )
  const visibleRecords = useMemo(() => {
    const filtered = networkFilter === 'all'
      ? records
      : records.filter(({ network }) => network.networkKey === networkFilter)
    return [...filtered].sort((left, right) => {
      if (sortBy === 'name') return nodeLabel(left.node).localeCompare(nodeLabel(right.node))
      if (sortBy === 'head') return (right.node.currentHead ?? -1) - (left.node.currentHead ?? -1)
      return healthRank(left.node.health) - healthRank(right.node.health)
    })
  }, [networkFilter, records, sortBy])

  const hasProjection = !loading && (error === null || hasLastGood)
  const scopedNetworks = networkFilter === 'all' ? networks : networks.filter(network => network.networkKey === networkFilter)
  const healthyCount = hasProjection ? visibleRecords.filter(({ node }) => isHealthy(node.health)).length : null
  const streamLabel = realtimeStreamLabel(realtimeStatus)
  return (
    <section aria-label="Home">
      <div className="px-4 pt-4" data-realtime-status={realtimeStatus}>
        {error || streamLabel || !online ? (
          <>
          {error && (
            <Alert variant="destructive" className="border-none bg-red-400/10 rounded-md">
              <AlertDescription role="alert">{error}</AlertDescription>
            </Alert>
          )}
          {streamLabel && (
            <p className="mt-2 flex items-center gap-2 text-xs text-muted-foreground" role="status" aria-live="polite">
              <span className="inline-block size-1.5 rounded-full bg-emerald-600 animate-pulse" aria-hidden="true" />
              {streamLabel}
            </p>
          )}
          {!online && (
            <p className="mt-2 flex items-center gap-2 text-xs text-muted-foreground" role="status" aria-live="polite">
              <span className="inline-block size-1.5 rounded-full bg-yellow-600 animate-pulse" aria-hidden="true" />
              You are offline
            </p>
          )}
          </>
        ) : null}
      </div>
      {loading && (
        <p role="status" className="px-4 pt-4 text-sm text-muted-foreground">
          Starting Home…
        </p>
      )}

      {/* 4:3 keeps each of the six tiles at the width the original four-card
            2x2 grid gave them, instead of stretching the statistics track. */}
        <div data-slot="home-overview" className="grid min-w-0 items-center gap-4 p-4 lg:grid-cols-[minmax(0,4fr)_minmax(0,3fr)]">
        <div className="grid min-w-0 auto-rows-fr grid-cols-2 gap-2 sm:grid-cols-3" aria-label="Home summary">
          <SummaryCard label="Active Nodes" value={hasProjection ? visibleRecords.length : null} tone="green" icon="server" />
          <SummaryCard label="Healthy Nodes" value={healthyCount} tone="green" icon="heart" />
          <ValidatorTotalCard networks={scopedNetworks} metric="blocks" availability={loading ? 'loading' : hasProjection ? 'ready' : 'unavailable'} />
          <SummaryCard label="Attention" value={healthyCount === null ? null : visibleRecords.length - healthyCount}
            tone={healthyCount !== null && visibleRecords.length === healthyCount ? 'green' : 'red'} icon="alert" />
          <SummaryCard label="Networks" value={hasProjection ? scopedNetworks.length : null} tone="green" icon="network" />
          <ValidatorTotalCard networks={scopedNetworks} metric="rewards" availability={loading ? 'loading' : hasProjection ? 'ready' : 'unavailable'} />
        </div>
        <div data-slot="home-map" className="min-w-0 aspect-[2/1]">
          <GeoMapBoundary>
            <GeoWorldMap networks={networks} networkFilter={networkFilter} loading={loading} hasProjection={hasProjection} />
          </GeoMapBoundary>
        </div>
      </div>

      <div className="relative p-4 pt-0 md:static">
        <div className="flex flex-nowrap items-start gap-2 md:items-center" aria-label="Node filters and sorting">
          <div className="overflow-x-auto rounded-sm py-1.5 -my-1.5 md:relative md:z-10">
            <Tabs
              value={networkFilter}
              onValueChange={setNetworkFilter}
              className="w-full flex-col gap-4"
            >
              <TabsList className={cn('compact-tabs min-h-0 group-data-[orientation=horizontal]/tabs:h-8 h-8 w-max rounded-md md:bg-background', SURFACE_TOOLBAR)} aria-label="Network filter">
                <TabsTrigger value="all" className="min-h-0 h-6.5 shrink-0 flex-none rounded-sm border-none text-xs shadow-none data-[state=active]:text-emerald-600 dark:data-[state=active]:text-emerald-600">
                  All Networks
                </TabsTrigger>
                {networks.map((network) => (
                  <TabsTrigger
                    key={network.networkKey}
                    value={network.networkKey}
                    className="min-h-0 h-6.5 shrink-0 flex-none rounded-sm border-none text-xs shadow-none data-[state=active]:text-emerald-600 dark:data-[state=active]:text-emerald-600"
                  >
                    {network.displayName}
                  </TabsTrigger>
                ))}
              </TabsList>
            </Tabs>
          </div>
          <label className="ml-auto flex items-center gap-2 text-xs text-muted-foreground md:relative md:z-10 md:shrink-0 md:rounded-md md:bg-background md:pl-2">
            Sort
            <Select
              aria-label="Sort"
              className={cn('compact-select h-8 w-auto rounded-md border-x-0 border-y-[6px] border-transparent bg-clip-padding -my-1.5 shadow-none md:bg-background md:text-foreground dark:md:bg-background', SURFACE_TOOLBAR)}
              value={sortBy}
              onChange={(event) => setSortBy(event.target.value as SortKey)}
            >
              {sortOptions.map((option) => (
                <option key={option.value} value={option.value}>{option.label}</option>
              ))}
            </Select>
          </label>
        </div>

        <div className="mt-4">
          {loading || (error && !hasLastGood) ? null : visibleRecords.length === 0 ? (
            <Empty description="No Active Nodes in this view.">
              <span className="text-xs">Retired Nodes are not listed on Home.</span>
            </Empty>
          ) : (
            <div
              className="grid grid-cols-1 items-start gap-3 sm:grid-cols-[repeat(auto-fill,minmax(300px,1fr))]"
              data-slot="node-grid"
              aria-label="Active Nodes"
            >
              {visibleRecords.map(({ network, node }) => (
                <div className="min-w-0" key={node.nodeId}>
                  <HomeNodeCard network={network} node={node} />
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </section>
  )
}

const SUMMARY_ICONS = {
  server: Server,
  heart: HeartPulse,
  alert: TriangleAlert,
  network: Network,
} as const

function SummaryCard({ label, value, tone, icon }: {
  label: string; value: number | null; tone: 'green' | 'red'; icon: keyof typeof SUMMARY_ICONS
}) {
  return <HomeSummaryCard label={label} value={value === null ? 'Unknown' : value.toLocaleString()}
    tone={value !== null && value > 0 ? tone : 'green'} icon={SUMMARY_ICONS[icon]} />
}

/**
 * Compact Home card, styled as Emerald's NodeCard: a stretched semantic title
 * link to Node Detail, an emerald status dot with a ping ring, a two-column
 * resource grid with thin progress bars, and a label-over-value consensus
 * grid without per-row leaders. Copy, disclosure and identity controls sit
 * outside the anchor so they never trigger navigation. Healthy Nodes carry no
 * routine prose; only an exceptional Node keeps one short diagnostic line
 * (issue #97).
 */
function HomeNodeCard({ network, node }: NodeRecord) {
  const tone = toneFor(node.health)
  const diagnostic = exceptionalDiagnostic(node)
  return (
    <article
      data-slot="node-card"
      data-tone={tone}
      className={cn(
        // Borderless by design (the surface and its hover ring carry the
        // card): Tailwind's preflight leaves border-style: solid behind, so
        // this has to be stated explicitly.
        'group/node-card relative w-full rounded-md border-none bg-background/60 transition-all duration-200',
        'hover:z-1 hover:-translate-y-0.5 hover:bg-background hover:shadow-[0_0_20px,0_0_0_1px] hover:shadow-emerald-600/10',
        tone === 'bad' && 'shadow-[0_0_0_1px] shadow-red-600/20',
        tone === 'warn' && 'shadow-[0_0_0_1px] shadow-amber-500/20',
      )}
    >
      <CardX bordered={false} className="bg-transparent" contentClassName="flex flex-col gap-3" headerClassName="!grid grid-cols-[minmax(0,1fr)_auto] items-start gap-x-2 gap-y-1.5" header={<>
          <div className="flex min-w-0 items-center gap-2">
            <NodeHealthMarker health={node.health} />
            {/* The 44px target is kept on the anchor; the negative block margin
                keeps it from inflating the identity row, so the name row and
                the Network · Uptime row stay content-driven and can sit 6px
                apart. */}
            <h2 className="min-w-0 text-base font-bold"><Link to={`/nodes/${node.nodeId}`} aria-label={nodeLabel(node)} title={nodeLabel(node)} className="flex -my-2.5 min-h-11 min-w-0 items-center after:absolute after:inset-0 after:rounded-md focus-visible:outline-none focus-visible:after:ring-[3px] focus-visible:after:ring-ring/50"><span className="truncate">{nodeLabel(node)}</span></Link></h2>
          </div>
          <ValidatorBadge consensus={node.consensus} />
          <div className="col-span-2 flex min-w-0 items-center gap-1 text-xs text-muted-foreground">
            <p className="min-w-0 flex-1"><span className="[overflow-wrap:anywhere]">{network.displayName}</span> · <span className="whitespace-nowrap">Uptime {formatDuration(node.processUptimeMs)}</span></p>
            <Dialog><DialogTrigger asChild><Button variant="ghost" size="icon" className="relative z-10 -my-3.5 size-11 shrink-0" aria-label="Node identity details"><Info className="size-3.5" /></Button></DialogTrigger>
              <DialogContent className="max-h-[85dvh] overflow-y-auto rounded-md shadow-sm"><DialogTitle className="pr-10 [overflow-wrap:anywhere]">{nodeLabel(node)}</DialogTitle><DialogDescription className="[overflow-wrap:anywhere]">Network: {network.displayName} · Uptime {formatDuration(node.processUptimeMs)}. Node role describes the Node’s consensus membership, not its linked Validator’s current staking validity or the freshness of Provider data.</DialogDescription></DialogContent>
            </Dialog>
          </div>
        </>}>
        {diagnostic && (
          <p
            data-slot="node-diagnostic"
            data-tone={diagnostic.tone}
            className={cn(
              'text-[11px]',
              diagnostic.tone === 'destructive'
                ? 'text-destructive'
                : 'text-amber-500 dark:text-amber-400',
            )}
          >
            {diagnostic.text}
          </p>
        )}
        <ResourceRow node={node} />
        <div data-slot="node-business-metrics" className="grid grid-cols-2 gap-x-4 gap-y-3 border-t border-border pt-3">
          <MetricRow layout="stacked" label="Head" value={formatNumber(node.currentHead)} />
          <ConsensusRow consensus={node.consensus} />
          <div data-slot="node-counts" data-wide={formatNumber(node.latestBlockTransactionCount).length > 12 || formatPeerCount(node).length > 12 || undefined}>
            <MetricRow label="Txs" value={formatNumber(node.latestBlockTransactionCount)} />
            <MetricRow label="Peers" value={formatPeerCount(node)} />
          </div>
          {formatPeerObservation(node) && (
            <small data-slot="metric-row-detail" className="col-span-2 text-[11px] text-muted-foreground">
              {formatPeerObservation(node)}
            </small>
          )}
        </div>
        <LinkedValidatorSection node={node} />
        </CardX>
    </article>
  )
}

function ResourceRow({ node }: { node: PublicNode }) {
  const nodeDataProgressValue = nodeDataProgress(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)
  return (
    <div
      className="grid grid-cols-2 gap-x-3 gap-y-1"
      aria-label="Node process and host network resources"
    >
      <MetricRow label="CPU" value={formatPercent(node.processCpuPercent)} progress={node.processCpuPercent ?? null} />
      <MetricRow label="Memory" value={formatPercent(node.processMemoryPercent)} progress={node.processMemoryPercent ?? null} />
      <div className="col-span-2" data-slot="node-data-resource">
        <MetricRow
          label="Node data"
          value={formatPercent(nodeDataProgressValue)}
          detail={formatNodeDataBytes(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)}
          progress={nodeDataProgressValue}
        />
      </div>
      <div className="col-span-2" data-slot="host-network-speed" role="group" aria-label="Host network speed">
        <MetricRow label="Speed" value={<span className="flex gap-2">
          <span aria-label={`Upload ${formatRate(node.hostNetworkTxBytesPerSec)}`} className="inline-flex items-baseline text-green-600"><ChevronUp className="size-3 shrink-0 self-center" aria-hidden="true" />{formatRate(node.hostNetworkTxBytesPerSec)}</span>
          <span aria-label={`Download ${formatRate(node.hostNetworkRxBytesPerSec)}`} className="inline-flex items-baseline text-blue-600"><ChevronDown className="size-3 shrink-0 self-center" aria-hidden="true" />{formatRate(node.hostNetworkRxBytesPerSec)}</span>
        </span>} />
      </div>
    </div>
  )
}

function formatPercent(value: number | null | undefined) {
  return value == null ? 'Unknown' : `${value.toFixed(1)}%`
}

function formatRate(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value) || value < 0) return 'Unknown'
  const units = ['bps', 'Kbps', 'Mbps', 'Gbps', 'Tbps']
  let scaled = value * 8
  let unit = 0
  while (scaled >= 1000 && unit < units.length - 1) {
    scaled /= 1000
    unit += 1
  }
  let amount = Number(scaled.toPrecision(3))
  // Rounding at a unit boundary should show 1Mbps, not 1000Kbps.
  if (amount >= 1000 && unit < units.length - 1) {
    amount /= 1000
    unit += 1
  }
  return `${amount}${units[unit]}`
}

/**
 * QC, Locked, Committed and the header role badge come from
 * the Node-scoped last-good consensus observation (issue #99). Missing,
 * unsupported, disabled, and never-observed values render Unknown, never zero
 * or No; failed or stale collections keep the last-good values and visibly mark
 * them Stale.
 */
function ConsensusRow({ consensus }: { consensus: PublicConsensusInsight | undefined }) {
  const status = consensusValueStatus(consensus)
  const detail = status === 'stale' ? 'Stale' : undefined
  return (
    <div className="contents" role="group" aria-label="Consensus values">
      <MetricRow layout="stacked" label="QC" value={formatConsensusBlock(consensus?.highestQcBlock, status)} detail={detail} />
      <MetricRow layout="stacked" label="Locked" value={formatConsensusBlock(consensus?.highestLockBlock, status)} detail={detail} />
      <MetricRow layout="stacked" label="Committed" value={formatConsensusBlock(consensus?.highestCommitBlock, status)} detail={detail} />
    </div>
  )
}

/** Neutral membership, independent of Node health; keep last-good freshness visible.
 *  One line when the card has room: `Node: Non-validator`. */
function ValidatorBadge({ consensus }: { consensus: PublicConsensusInsight | undefined }) {
  const status = consensusValueStatus(consensus)
  const value = formatConsensusValidator(consensus, status)
  const stale = status === 'stale'
  return (
    <span data-slot="validator-role" aria-label={`Role: ${value}${stale ? ' (Stale)' : ''}`}
      className="ml-auto inline-flex min-w-0 max-w-full items-baseline gap-1 rounded border border-border/60 px-1.5 py-0.5 text-[11px] leading-4 text-muted-foreground">
      <span className="shrink-0">Node:</span><span className="truncate">{value}</span>
      {stale && <span className="shrink-0">Stale</span>}
    </span>
  )
}

function consensusValueStatus(insight: PublicConsensusInsight | undefined): 'current' | 'stale' | 'unknown' {
  if (!insight || insight.validator == null) return 'unknown'
  // Starting/Disabled/Unsupported never carry a usable consensus value;
  // only an accepted successful observation provides membership truth.
  if (['starting', 'disabled', 'unsupported'].includes(insight.state)) return 'unknown'
  // Unknown freshness means the Server cannot certify currency; never
  // present a retained value as current (issue #99).
  if (insight.freshness === 'unknown') return 'unknown'
  // A failed collection or an aged value is Stale while the last-good value
  // remains visible; Server state and freshness remain separate dimensions.
  if (insight.state === 'error' || insight.freshness === 'stale') return 'stale'
  return 'current'
}

function formatConsensusBlock(value: number | null | undefined, status: 'current' | 'stale' | 'unknown'): string {
  if (value == null || status === 'unknown') return 'Unknown'
  return value.toLocaleString()
}

function formatConsensusValidator(insight: PublicConsensusInsight | undefined, status: 'current' | 'stale' | 'unknown'): string {
  if (status === 'unknown' || insight?.validator == null) return 'Unknown'
  return insight.validator ? 'Validator' : 'Non-validator'
}

type NodeDiagnostic = { text: string; tone: 'destructive' | 'warning' }

/** One sanitized diagnostic line for exceptional Nodes only (issue #97).
 *  A resync is a Warning rather than a failure, so it is labelled and kept
 *  out of the destructive red that marks an unhealthy Node. */
function exceptionalDiagnostic(node: PublicNode): NodeDiagnostic | null {
  if (node.health !== 'healthy') {
    const reason = node.healthReason?.trim()
    return {
      text: reason || `Health ${healthLabel(node.health).toLowerCase()}`,
      tone: 'destructive',
    }
  }
  if ((node.resyncState ?? '').toLowerCase() === 'resyncing') {
    const progress = node.resyncProgress?.trim()
    return { text: `Resyncing · ${progress || 'progress unknown'}`, tone: 'warning' }
  }
  return null
}

function formatPeerCount(node: PublicNode) {
  return formatNumber(node.peers?.peerCount)
}
function formatPeerObservation(node: PublicNode): string | undefined {
  const peer = node.peers
  if (!peer) return 'Unknown observation'

  const collection = peerInsightCollectionStatus(peer)
  const freshness = peerInsightFreshnessStatus(peer)
  const value = peerInsightValueStatus(peer)
  const hasValue = value !== 'Unknown'
  const qualifiers: string[] = []

  if (collection === 'Error') qualifiers.push('Collection failed')
  else if (collection !== 'Current') qualifiers.push('Collection ' + collection.toLowerCase())
  if (freshness === 'Stale') qualifiers.push('Stale')
  else if (freshness !== 'Current') qualifiers.push('Freshness unknown')

  if (!hasValue) {
    if (qualifiers.length === 0) qualifiers.push('Value unknown')
    qualifiers.push('No successful Peer snapshot is available')
    return qualifiers.join('; ')
  }

  if (qualifiers.length > 0) {
    qualifiers.push('Showing last successful snapshot')
    if (value === 'Empty') qualifiers.push('authoritative zero')
    return qualifiers.join('; ')
  }

  if (value === 'Empty') return 'Empty; authoritative zero'
  // A fresh peer count is self-explanatory; exceptional observation details
  // remain visible below the value.
  return undefined
}
function healthLabel(value: string): string {
  if (value === 'healthy') return 'Healthy'
  if (value === 'unhealthy') return 'Unhealthy'
  return 'Unknown'
}

function nodeLabel(node: PublicNode) { return node.displayName ?? node.nodeId }
function formatNumber(value: number | null | undefined) { return value == null ? 'Unknown' : value.toLocaleString() }
function isHealthy(value: string) { return value.toLowerCase() === 'healthy' }
function healthRank(value: string) { const tone = toneFor(value); return tone === 'bad' ? 0 : tone === 'warn' ? 1 : tone === 'good' ? 2 : 3 }
function toneFor(value: string): 'good' | 'warn' | 'bad' | 'neutral' {
  const normalized = value.toLowerCase()
  if (/(error|failed|unhealthy|offline|unavailable)/.test(normalized)) return 'bad'
  if (normalized === 'live' || /(healthy|current|connected|ready|synced|active|running|ok|fresh)/.test(normalized)) return 'good'
  if (/(starting|unknown|unsupported|disabled|empty|stale|resync|degraded|connecting)/.test(normalized)) return 'warn'
  return 'neutral'
}
