import { useEffect, useMemo, useState } from 'react'
import { useNodeRegionHeights } from './useNodeRegionHeights'
import { Link } from 'react-router'
import type { PublicConsensusInsight, PublicNetwork, PublicNode } from '../api/generated'
import { realtimeStreamLabel } from './RealtimeNotice'
import { peerInsightCollectionStatus, peerInsightFreshnessStatus, peerInsightValueStatus } from './PeerInsight'
import { NodeHealthMarker, formatRelativeTime, formatUtcDateTime } from './StatusBadge'
import GeoMapBoundary from './GeoMapBoundary'
import GeoWorldMap from './GeoWorldMap'
import { formatNodeDataBytes } from '../formatBytes'
import { formatDuration } from '../formatDuration'
import { nodeDataProgress } from '../nodeData'
import { MetricRow } from './MetricRow'
import type { ProgressStatus } from './ui/progress-thin'
import { CardX } from './ui/card-x'
import { Alert, AlertDescription } from './ui/alert'
import { Empty } from './ui/empty'
import { Select } from './ui/input'
import { Tabs, TabsList, TabsTrigger } from './ui/tabs'
import { Server, HeartPulse, TriangleAlert, Network, ChevronUp, ChevronDown, Info } from 'lucide-react'
import { SURFACE_TOOLBAR } from '../lib/surface'
import { cn } from '../lib/utils'
import { LinkedValidatorSection, validatorDataStatus, type ValidatorDataStatus } from './LinkedValidator'
import { ValidatorTotalCard } from './ValidatorTotals'
import { ValidatorActivityBadge } from './ValidatorActivityBadge'
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
  const nodeGridRef = useNodeRegionHeights(visibleRecords, hasProjection)
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

      {/* The map track is deliberately wider than the statistics track (5:6),
            matching the Emerald reference where the map is the larger half of
            the band. When the six tiles are shorter than the map band they sit
            on its floor, so the gap down to the Network group is the same 16px
            rhythm that separates that group from the Node cards. */}
        <div data-slot="home-overview" className="grid min-w-0 items-end gap-4 p-4 lg:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]">
        <div className="grid min-w-0 auto-rows-fr grid-cols-2 gap-2 sm:grid-cols-3" aria-label="Home summary">
          <SummaryCard label="Active Nodes" value={hasProjection ? visibleRecords.length : null} tone="green" icon="server" />
          <SummaryCard label="Healthy Nodes" value={healthyCount} tone="green" icon="heart" />
          <ValidatorTotalCard networks={scopedNetworks} metric="blocks" availability={loading ? 'loading' : hasProjection ? 'ready' : 'unavailable'} />
          <SummaryCard label="Attention" value={healthyCount === null ? null : visibleRecords.length - healthyCount}
            tone={healthyCount !== null && visibleRecords.length === healthyCount ? 'green' : 'red'} icon="alert" />
          <SummaryCard label="Networks" value={hasProjection ? scopedNetworks.length : null} tone="green" icon="network" />
          <ValidatorTotalCard networks={scopedNetworks} metric="rewards" availability={loading ? 'loading' : hasProjection ? 'ready' : 'unavailable'} />
        </div>
        {/* Upstream's DOM places the map first and the six cards after it, so on a
            phone the map sits at the top of the overview and the cards follow.
            The DOM keeps the statistics first for assistive reading and CSS
            order restores upstream's visual order below lg.
            Below xl the track is proportional (2:1) so phones and tablets keep
            the compact map the mobile acceptance measured. From xl, where Home
            reaches its 1280px ceiling and the map column stops changing, the
            track uses upstream's fixed 22rem band, which is tall enough for the
            world's own aspect ratio and so no longer crops it. */}
        <div data-slot="home-map" className="order-first min-w-0 aspect-[2/1] lg:order-none xl:aspect-auto xl:h-88">
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
          {/* 22.5rem (360px at the default root font size) is the same width at
              which the card's own container query turns on the two-column
              consensus and Validator-parameter grids (see the node-card rules
              in emerald.css). Aligning the minimum track with that switch means
              every auto-filled card is wide enough to use its width, and the
              grid still exposes four columns on a container that can hold four
              of them. Both use rem so the threshold scales with the root font
              size. */}
          {loading || (error && !hasLastGood) ? null : visibleRecords.length === 0 ? (
            <Empty description="No Active Nodes in this view.">
              <span className="text-xs">Retired Nodes are not listed on Home.</span>
            </Empty>
          ) : (
            <div
              className="grid auto-rows-fr grid-cols-1 gap-3 sm:grid-cols-[repeat(auto-fill,minmax(22.5rem,1fr))]"
              ref={nodeGridRef}
              data-slot="node-grid"
              aria-label="Active Nodes"
            >
              {visibleRecords.map(({ network, node }) => (
                <div data-slot="node-card-frame" className="min-w-0" key={node.nodeId}>
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
 * routine prose. Exceptional health keeps a short diagnostic line (issue #97);
 * resync progress is a separate, lightweight status area.
 */
function HomeNodeCard({ network, node }: NodeRecord) {
  const tone = toneFor(node.health)
  const diagnostic = exceptionalDiagnostic(node)
  // Every displayed chain value is computed once: the grid borrows the same
  // strings to decide whether a compact two-column cell can hold them.
  const consensusStatus = consensusValueStatus(node.consensus)
  const business = {
    head: formatNumber(node.currentHead),
    qc: formatConsensusBlock(node.consensus?.highestQcBlock, consensusStatus),
    locked: formatConsensusBlock(node.consensus?.highestLockBlock, consensusStatus),
    committed: formatConsensusBlock(node.consensus?.highestCommitBlock, consensusStatus),
    txs: formatNumber(node.latestBlockTransactionCount),
    peers: formatPeerCount(node),
  }
  const businessWide = Object.values(business).some((value) => value.length > 12)
  const resyncing = (node.resyncState ?? '').toLowerCase() === 'resyncing'
  // One compact, abnormal-only Provider data cue for the identity row. A
  // routine Current value stays silent; the region below owns the authoritative
  // no-live-Validator empty state.
  const dataStatus = node.validator ? validatorDataStatus(node.validator) : null
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
      <CardX bordered={false} className="bg-transparent" contentClassName="gap-2.5" headerClassName="!block !p-0" header={<div data-node-region="identity"><div data-node-region-content className="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-x-2 gap-y-1.5 px-4 pt-3 pb-2.5">
          <div className="flex min-w-0 items-center gap-2">
            <NodeHealthMarker health={node.health} />
            {/* The 44px target is kept on the anchor; the negative block margin
                keeps it from inflating the identity row, so the name row and
                the Network · Uptime row stay content-driven and can sit 6px
                apart. */}
            <h2 className="min-w-0 text-base font-semibold"><Link to={`/nodes/${node.nodeId}`} aria-label={nodeLabel(node)} title={nodeLabel(node)} className="flex -my-2.5 min-h-11 min-w-0 items-center after:absolute after:inset-0 after:rounded-md focus-visible:outline-none focus-visible:after:ring-[3px] focus-visible:after:ring-ring/50"><span className="truncate">{nodeLabel(node)}</span></Link></h2>
          </div>
          <ValidatorActivityBadge validator={node.validator} identityReason={node.validatorIdentityReason} />
          <div data-slot="node-identity-meta" className="col-span-2 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
            <p className="min-w-0 flex-1"><span className="[overflow-wrap:anywhere]">{network.displayName}</span> · <span className="whitespace-nowrap">Uptime {formatDuration(node.processUptimeMs)}</span></p>
            {/* One unified top status position: an abnormal Node-health summary,
                a live resync percentage and an abnormal Provider data state sit
                here as short chips. The normal Data: Current line is gone; the
                full reason for each stays in the explanation and Node detail. */}
            <span data-slot="node-status-slot" className="ml-auto flex min-w-0 flex-wrap items-center justify-end gap-x-1.5 gap-y-1">
              {resyncing && <ResyncCue node={node} />}
              {dataStatus && <ValidatorDataCue status={dataStatus} />}
            {/* State audit of the reference capture's conspicuous frame: the control is
                transparent by default (transparent background, transparent 1px border, no
                outline or shadow), paints the ghost accent on hover, and paints the shared
                3px focus-visible ring while keyboard-focused. The open dialog also gives the
                trigger aria-expanded's accent background. The captured frame is that
                focus-visible ring — required keyboard feedback, so it is kept; the icon
                matches the copy/Details controls at 14px. */}
            <Dialog><DialogTrigger asChild><Button variant="ghost" size="icon" className="relative z-10 -my-3.5 size-11 shrink-0" aria-label="Node identity details"><Info className="size-3.5" /></Button></DialogTrigger>
              <DialogContent className="max-h-[85dvh] overflow-y-auto rounded-md shadow-sm"><DialogTitle className="pr-10 [overflow-wrap:anywhere]">{nodeLabel(node)}</DialogTitle><DialogDescription className="[overflow-wrap:anywhere]">Network: {network.displayName} · Uptime {formatDuration(node.processUptimeMs)}. Node role describes the Node’s consensus membership, not its linked Validator’s current staking validity or the freshness of Provider data.</DialogDescription>
                <p className="text-sm text-muted-foreground">Active Nodes are in the latest Agent Inventory, not necessarily online. Healthy reflects successful, fresh RPC, sync and consensus observations. Process errors, a stopped or Unknown process state, or Network Identity Mismatch prevent Healthy; disabled process monitoring does not. Healthy does not mean synchronization is complete; Resyncing is shown independently.</p>
                <p className="text-sm text-muted-foreground">Home Attention counts Active Nodes that are not Healthy, including Unknown. Counts and Node cards use the same selected Node data.</p>
                {diagnostic && <p className="text-sm text-muted-foreground">Current health diagnostic: {diagnostic.text}</p>}
                {dataStatus && <p className="text-sm text-muted-foreground">Validator data: {dataStatus.label}. {dataStatus.description}</p>}
              </DialogContent>
            </Dialog>
            </span>
          </div>
          {/* Abnormal reasons remain readable; healthy cards create no empty row. */}
          {diagnostic && <p
            data-slot="node-diagnostic"
            data-tone={diagnostic?.tone}
            className={cn(
              'col-span-2 m-0 text-[11px] [overflow-wrap:anywhere]',
              diagnostic?.tone === 'destructive' ? 'text-destructive' : 'text-amber-500 dark:text-amber-400',
            )}
          >
            {diagnostic.text}
          </p>}
        </div></div>}>
        <div data-node-region="resource"><div data-node-region-content><ResourceRow node={node} /></div></div>
        <div data-node-region="chain"><div data-node-region-content>
        <div data-slot="node-business-metrics" data-wide={businessWide || undefined} className="border-t border-border pt-3">
          <MetricRow layout="compact" label="Head" value={business.head} />
          <ConsensusRow status={consensusStatus} values={business} />
          <div data-slot="node-counts" data-wide={business.txs.length > 12 || business.peers.length > 12 || undefined}>
            <MetricRow layout="compact" label="Txs" value={business.txs} />
            <MetricRow layout="compact" label="Peers" value={business.peers} />
          </div>
          {formatPeerObservation(node) && (
            <small data-slot="metric-row-detail" className="col-span-2 text-[11px] text-muted-foreground">
              {formatPeerObservation(node)}
            </small>
          )}
        </div>
        </div></div>
        <div data-node-region="validator"><div data-node-region-content><LinkedValidatorSection node={node} /></div></div>
        </CardX>
    </article>
  )
}

/** Resync targets the retained Historical High-Water Mark, not Network Head.
 * Keep it independent of health, consensus membership and Validator identity.
 * Only the live percentage stays on the identity row; the retained target, the
 * last progress and its full time remain reachable from the same focusable
 * control, so nothing the removed three-line block carried is deleted. */
function ResyncCue({ node }: { node: PublicNode }) {
  const [, refreshAge] = useState(0)
  useEffect(() => {
    // Display age only: Server-owned health, freshness and resync state stay untouched.
    const timer = window.setInterval(() => refreshAge(tick => tick + 1), 15_000)
    return () => window.clearInterval(timer)
  }, [])
  const current = validHeight(node.currentHead)
  const target = validHeight(node.historicalHighWatermark)
  const percent = current !== null && target !== null && target > 0
    ? (current / target * 100).toFixed(2) + '%'
    : 'Unknown'
  const timestamp = node.resyncLastProgressAt
  const date = validProgressDate(timestamp)
  return (
    <Dialog>
      <DialogTrigger asChild>
        {/* Keep the 44px touch target without reserving its full height in the
            compact metadata line, matching the neighbouring information entry. */}
        <button type="button" data-slot="node-resync-cue"
          className="relative z-10 -my-3.5 inline-flex min-h-11 items-center rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label={`Resync progress: ${percent} toward the Historical High-Water Mark`}>
          <span className="inline-flex items-center gap-1 rounded border border-amber-500/40 px-1.5 py-0.5 text-[11px] font-medium leading-4 text-amber-500 dark:text-amber-400">
            <span>Resyncing</span><span className="tabular-nums">{percent}</span>
          </span>
        </button>
      </DialogTrigger>
      <DialogContent className="max-h-[85dvh] overflow-y-auto rounded-md shadow-sm">
        <DialogTitle>Resync progress</DialogTitle>
        <DialogDescription>The last recorded advance toward the Historical High-Water Mark, not the latest report time. The target is the Node’s retained historical height, not the Network Head.</DialogDescription>
        <p className="m-0 text-sm tabular-nums">{formatNumber(current)} / {formatNumber(target)}</p>
        {date ? <p className="m-0 text-sm">Last progress <time dateTime={date.toISOString()}>{formatRelativeTime(date)}</time> · {formatUtcDateTime(date)}</p> : <p className="m-0 text-sm">Last progress: Unknown</p>}
        {date && timestamp && <p className="m-0 min-w-0 overflow-x-auto whitespace-nowrap font-mono text-xs text-muted-foreground" tabIndex={0} aria-label="Full source timestamp with UTC offset">{timestamp}</p>}
      </DialogContent>
    </Dialog>
  )
}

/** The abnormal Provider data state, kept separate from the online/health
 *  marker and from the freshness of the Node’s own observations. */
function ValidatorDataCue({ status }: { status: ValidatorDataStatus }) {
  return <span data-slot="validator-data-cue" data-tone={status.tone} role="status" title={status.description}
    aria-label={`Validator data: ${status.label}. ${status.description}`}
    className={cn('inline-flex min-w-0 items-center gap-1 rounded border px-1.5 py-0.5 text-[11px] leading-4',
      status.tone === 'destructive'
        ? 'border-destructive/40 text-destructive'
        : 'border-amber-500/40 text-amber-600 dark:text-amber-400')}>
    <span className={cn('inline-block size-1.5 shrink-0 rounded-full', status.tone === 'destructive' ? 'bg-destructive' : 'bg-amber-500')} aria-hidden="true" />
    <span className="whitespace-nowrap">Data: {status.label}</span>
  </span>
}

/** Require a zoned RFC3339 value; Date alone normalizes impossible dates and
 * interprets zone-less strings in the viewer's timezone. Neither is evidence. */
function validProgressDate(value: string | null | undefined): Date | null {
  if (typeof value !== 'string' || !/^\d{4}-(?:0[1-9]|1[0-2])-(?:0[1-9]|[12]\d|3[01])T(?:[01]\d|2[0-3]):[0-5]\d:[0-5]\d(?:\.\d+)?(?:Z|[+-](?:[01]\d|2[0-3]):[0-5]\d)$/i.test(value)) return null
  const date = new Date(value)
  const calendarDate = value.slice(0, 10)
  const calendar = new Date(calendarDate + 'T00:00:00Z')
  if (!Number.isFinite(date.getTime()) || calendar.toISOString().slice(0, 10) !== calendarDate) return null
  return date
}

function validHeight(value: number | null | undefined): number | null {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : null
}

/** PlatPulse owns no occupancy threshold rule for process CPU/Memory or the
 *  Node data directory, so none is invented here: the ordinary fill uses
 *  Emerald's normal-state success token, the same green family as the online
 *  dot and the theme accent, instead of the informational blue or the default
 *  primary (white in the dark theme). A Server-owned warning/error rule would
 *  replace this, and it would still have to map onto the existing tokens. */
const RESOURCE_BAR_STATUS: ProgressStatus = 'success'

function ResourceRow({ node }: { node: PublicNode }) {
  const nodeDataProgressValue = nodeDataProgress(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)
  const nodeDataBytes = formatNodeDataBytes(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)
  // Wide cards fold the used / total caption into the value line; a narrow
  // card keeps it as its own caption under the track. The bytes and the
  // percentage are both always rendered, only regrouped.
  const nodeDataValue = formatPercent(nodeDataProgressValue)
  return (
    <div
      data-slot="node-resource-region"
      className="grid grid-cols-2 gap-x-3 gap-y-1"
      aria-label="Node process and host network resources"
    >
      <MetricRow layout="compact" label="CPU" value={formatPercent(node.processCpuPercent)} progress={node.processCpuPercent ?? null} progressStatus={RESOURCE_BAR_STATUS} />
      <MetricRow layout="compact" label="Memory" value={formatPercent(node.processMemoryPercent)} progress={node.processMemoryPercent ?? null} progressStatus={RESOURCE_BAR_STATUS} />
      <div className="col-span-2" data-slot="node-data-resource">
        <MetricRow
          layout="compact"
          label="Node data"
          value={nodeDataValue}
          wideValue={nodeDataBytes && nodeDataProgressValue != null ? nodeDataBytes + ' · ' + nodeDataValue : undefined}
          detail={nodeDataBytes}
          progress={nodeDataProgressValue}
          progressStatus={RESOURCE_BAR_STATUS}
        />
      </div>
      <div className="col-span-2" data-slot="host-network-speed" role="group" aria-label="Host network speed">
        <MetricRow layout="compact" label="Speed" value={<span className="flex items-center gap-2 whitespace-nowrap">
          <span aria-label={`Upload ${formatRate(node.hostNetworkTxBytesPerSec)}`} className="inline-flex items-center gap-0.5 text-green-600"><ChevronUp className="size-3 shrink-0" aria-hidden="true" />{formatRate(node.hostNetworkTxBytesPerSec)}</span>
          <span aria-label={`Download ${formatRate(node.hostNetworkRxBytesPerSec)}`} className="inline-flex items-center gap-0.5 text-blue-600"><ChevronDown className="size-3 shrink-0" aria-hidden="true" />{formatRate(node.hostNetworkRxBytesPerSec)}</span>
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
function ConsensusRow({ status, values }: {
  status: 'current' | 'stale' | 'unknown'
  values: { qc: string; locked: string; committed: string }
}) {
  const detail = status === 'stale' ? 'Stale' : undefined
  return (
    <div className="contents" role="group" aria-label="Consensus values">
      <MetricRow layout="compact" label="QC" value={values.qc} detail={detail} />
      <MetricRow layout="compact" label="Locked" value={values.locked} detail={detail} />
      <MetricRow layout="compact" label="Committed" value={values.committed} detail={detail} />
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

type NodeDiagnostic = { text: string; tone: 'destructive' | 'warning' }

/** One sanitized health diagnostic line for exceptional Nodes (issue #97).
 * Resync progress is independent and never hidden by an unhealthy diagnostic. */
function exceptionalDiagnostic(node: PublicNode): NodeDiagnostic | null {
  if (node.health !== 'healthy') {
    const reason = node.healthReason?.trim()
    return {
      text: reason || `Health ${healthLabel(node.health).toLowerCase()}`,
      tone: 'destructive',
    }
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
