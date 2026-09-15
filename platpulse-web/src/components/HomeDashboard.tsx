import { useMemo, useState } from 'react'
import { Link } from 'react-router'
import type { PublicConsensusInsight, PublicNetwork, PublicNode } from '../api/generated'
import { realtimeStreamLabel } from './RealtimeNotice'
import { peerInsightCollectionStatus, peerInsightFreshnessStatus, peerInsightValueStatus } from './PeerInsight'
import { NodeHealthMarker } from './StatusBadge'
import GeoMapBoundary from './GeoMapBoundary'
import GeoWorldMap from './GeoWorldMap'
import { formatNodeDataBytes } from '../formatBytes'
import { nodeDataProgress } from '../nodeData'
import { MetricRow } from './MetricRow'
import { CardX } from './ui/card-x'
import { Alert, AlertDescription } from './ui/alert'
import { Empty } from './ui/empty'
import { Select } from './ui/input'
import { Tabs, TabsList, TabsTrigger } from './ui/tabs'
import { EmeraldActionIcon } from './EmeraldActionIcon'
import { SURFACE_CARD, SURFACE_TOOLBAR } from '../lib/surface'
import { cn } from '../lib/utils'

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

/**
 * Home, laid out as Emerald's HomeView + NodeGeneralCards:
 * a 37:61 statistics/map top band at md and above (the approved reference-image
 * split), with statistics pulled over the map on small screens, then the
 * network toolbar built from Emerald Tabs, then the node grid at
 * repeat(auto-fill, minmax(300px, 1fr)) with a 12px gap.
 *
 * PlatPulse's own four counters stay in the statistics slot: Emerald's six
 * resource tiles include "remaining value", which has no PlatPulse field, and
 * aggregating Host resources across Nodes would double-count Hosts shared by
 * several Nodes (docs/visual-migration/emerald/README.md, deviation 2).
 */
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
  const healthyCount = hasProjection ? records.filter(({ node }) => isHealthy(node.health)).length : null
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

      <div className="grid h-auto grid-cols-12 grid-rows-1 gap-2 p-4 md:h-58 md:grid-cols-[minmax(0,37fr)_minmax(0,61fr)]">
        <div className="col-span-12 col-start-1 min-w-0 h-88 md:col-span-1 md:col-start-2 md:row-start-1 md:h-full">
          <GeoMapBoundary>
            <GeoWorldMap networks={networks} networkFilter={networkFilter} loading={loading} hasProjection={hasProjection} />
          </GeoMapBoundary>
        </div>
        <div
          className="z-9 -mt-42 col-span-12 row-start-3 grid h-42 grid-cols-12 grid-rows-2 gap-2 min-w-0 md:col-span-1 md:col-start-1 md:row-start-1 md:mt-0 md:h-auto"
          aria-label="Home summary"
        >
          <SummaryCard label="Active Nodes" value={hasProjection ? records.length : null} tone="green" icon="server" />
          <SummaryCard label="Healthy Nodes" value={healthyCount} tone="green" icon="heart" />
          <SummaryCard
            label="Attention"
            value={healthyCount === null ? null : records.length - healthyCount}
            tone={healthyCount !== null && records.length === healthyCount ? 'green' : 'red'}
            icon="alert"
          />
          <SummaryCard label="Networks" value={hasProjection ? networks.length : null} tone="green" icon="network" />
        </div>
      </div>

      <div className="p-4 pt-0">
        <div className="flex flex-nowrap items-start gap-2" aria-label="Node filters and sorting">
          <div className="overflow-x-auto rounded-sm">
            <Tabs
              value={networkFilter}
              onValueChange={setNetworkFilter}
              className="w-full flex-col gap-4"
            >
              <TabsList className={cn('h-8 w-max rounded-md', SURFACE_TOOLBAR)} aria-label="Network filter">
                <TabsTrigger value="all" className="h-6.5 shrink-0 flex-none rounded-sm border-none text-xs shadow-none data-active:text-emerald-600">
                  All Networks
                </TabsTrigger>
                {networks.map((network) => (
                  <TabsTrigger
                    key={network.networkKey}
                    value={network.networkKey}
                    className="h-6.5 shrink-0 flex-none rounded-sm border-none text-xs shadow-none data-active:text-emerald-600"
                  >
                    {network.displayName}
                  </TabsTrigger>
                ))}
              </TabsList>
            </Tabs>
          </div>
          <label className="ml-auto flex items-center gap-2 text-xs text-muted-foreground">
            Sort
            <Select
              className={cn('h-8 w-auto rounded-md border-none shadow-none', SURFACE_TOOLBAR)}
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
              className="grid grid-cols-1 gap-3 sm:grid-cols-[repeat(auto-fill,minmax(300px,1fr))]"
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
  server: 'setting',
  heart: 'dark-mode',
  alert: 'sun-one',
  network: 'moon',
} as const

function SummaryCard({
  label,
  value,
  tone,
  icon,
}: {
  label: string
  value: number | null
  tone: 'green' | 'red'
  icon: keyof typeof SUMMARY_ICONS
}) {
  return (
    <CardX
      hoverable
      bordered={false}
      role="article"
      size="small"
      data-slot="summary-card"
      data-tone={tone}
      className={cn('group col-span-6 row-span-1 h-full rounded-md transition-all', SURFACE_CARD)}
      contentClassName="h-full !p-3"
    >
      <div className="flex h-full flex-col justify-between gap-1">
        <div className="flex items-start justify-between gap-1">
          <span className="text-xs font-medium tracking-wider text-muted-foreground">{label}</span>
          <EmeraldActionIcon name={SUMMARY_ICONS[icon]} />
        </div>
        <div className="flex min-w-0 items-baseline gap-1">
          <strong
            data-slot="summary-value"
            className={cn(
              'text-base font-bold leading-none tracking-tight md:text-2xl',
              tone === 'red' && value !== null && value > 0 && 'text-destructive',
            )}
          >
            {value === null ? '—' : value.toLocaleString()}
          </strong>
        </div>
      </div>
    </CardX>
  )
}

/**
 * Compact Home card, styled as Emerald's NodeCard: one whole-card semantic link
 * to Node Detail, an emerald status dot with a ping ring, a two-column metric
 * grid with thin progress bars, and the dotted-leader info rows for the
 * consensus values. Healthy Nodes carry no routine prose; only an exceptional
 * Node keeps a single short diagnostic line (issue #97).
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
        'group/node-card relative h-full w-full rounded-md border-none bg-background/60 transition-all duration-200',
        'hover:z-1 hover:-translate-y-0.5 hover:bg-background hover:shadow-[0_0_20px,0_0_0_1px] hover:shadow-emerald-600/10',
        tone === 'bad' && 'shadow-[0_0_0_1px] shadow-red-600/20',
        tone === 'warn' && 'shadow-[0_0_0_1px] shadow-amber-500/20',
      )}
    >
      <Link
        className="flex h-full flex-col gap-3 rounded-md p-3 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
        to={`/nodes/${node.nodeId}`}
      >
        <header className="flex min-w-0 items-center gap-2">
          <NodeHealthMarker health={node.health} />
          <h2 className="min-w-0 flex-1 truncate text-base font-bold">{nodeLabel(node)}</h2>
        </header>
        <p className="truncate text-[11px] text-muted-foreground">{network.displayName}</p>
        {diagnostic && (
          <p data-slot="node-diagnostic" className="text-[11px] text-destructive">
            {diagnostic}
          </p>
        )}
        <ResourceRow node={node} />
        <div
          data-slot="metric-triple"
          className="grid grid-cols-3 gap-x-3 gap-y-1"
          aria-label="Node highlights"
        >
          <MetricRow label="Head" value={formatNumber(node.currentHead)} />
          <MetricRow label="Txs" value={formatNumber(node.latestBlockTransactionCount)} />
          <MetricRow label="Peers" value={formatPeerCount(node)} />
        </div>
        {formatPeerObservation(node) && (
          <small data-slot="metric-row-detail" className="text-[11px] text-muted-foreground">
            {formatPeerObservation(node)}
          </small>
        )}
        <ConsensusRow consensus={node.consensus} />
      </Link>
    </article>
  )
}

function ResourceRow({ node }: { node: PublicNode }) {
  const nodeDataProgressValue = nodeDataProgress(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)
  return (
    <div
      className="grid grid-cols-2 gap-x-3 gap-y-2"
      aria-label="Node process and host network resources"
    >
      <MetricRow label="CPU" value={formatPercent(node.processCpuPercent)} progress={node.processCpuPercent} />
      <MetricRow label="Memory" value={formatPercent(node.processMemoryPercent)} progress={node.processMemoryPercent} />
      <MetricRow
        label="Node data"
        value={formatPercent(nodeDataProgressValue)}
        detail={formatNodeDataBytes(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)}
        progress={nodeDataProgressValue}
      />
      <MetricRow label="Speed" value={<span className="flex gap-2">
        <span aria-label={`Upload ${formatRate(node.hostNetworkTxBytesPerSec)}`} className="text-green-600">↑{formatRate(node.hostNetworkTxBytesPerSec)}</span>
        <span aria-label={`Download ${formatRate(node.hostNetworkRxBytesPerSec)}`} className="text-blue-600">↓{formatRate(node.hostNetworkRxBytesPerSec)}</span>
      </span>} />
    </div>
  )
}

function formatPercent(value: number | null | undefined) {
  return value == null ? '—' : `${value.toFixed(1)}%`
}

function formatRate(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value) || value < 0) return '—'
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
 * The final compact metric rows: QC, Locked, Committed and Validator come from
 * the Node-scoped last-good consensus observation (issue #99). Missing,
 * unsupported, disabled, and never-observed values render Unknown, never zero
 * or No; failed or stale collections keep the last-good values and visibly mark
 * them Stale.
 */
function ConsensusRow({ consensus }: { consensus: PublicConsensusInsight | undefined }) {
  const status = consensusValueStatus(consensus)
  const detail = status === 'stale' ? 'Stale' : undefined
  return (
    <div className="flex flex-col gap-1" role="group" aria-label="Consensus and validator values">
      <div data-slot="metric-triple" className="grid grid-cols-3 gap-x-3 gap-y-1">
        <MetricRow label="QC" value={formatConsensusBlock(consensus?.highestQcBlock, status)} detail={detail} />
        <MetricRow label="Locked" shortLabel="L" value={formatConsensusBlock(consensus?.highestLockBlock, status)} detail={detail} />
        <MetricRow label="Committed" shortLabel="C" value={formatConsensusBlock(consensus?.highestCommitBlock, status)} detail={detail} />
      </div>
      <MetricRow label="Validator" value={formatConsensusValidator(consensus, status)} detail={detail} />
    </div>
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
  return insight.validator ? 'True' : 'False'
}

/** One sanitized diagnostic line for exceptional Nodes only (issue #97). */
function exceptionalDiagnostic(node: PublicNode): string | null {
  if (node.health !== 'healthy') {
    const reason = node.healthReason?.trim()
    return reason || `Health ${healthLabel(node.health).toLowerCase()}`
  }
  if ((node.resyncState ?? '').toLowerCase() === 'resyncing') {
    const progress = node.resyncProgress?.trim()
    return progress || 'Resync in progress'
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
