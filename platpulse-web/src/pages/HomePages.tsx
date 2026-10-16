import { ArrowLeft, Check, Copy } from 'lucide-react'
import { useState } from 'react'
import type { ReactNode } from 'react'
import { Link, useLocation, useParams } from 'react-router'
import {
  usePublicNode,
  usePublicNodeHistory,
  usePublicNodeMetrics,
} from '../api/public'
import type { PublicMetricPoint, PublicMetricSeriesCoverage, PublicNode, PublicNodeMetricHistory } from '../api/generated'
import { useHomeRealtimeContext } from '../layouts/HomeLayout'
import { homeReturnHref, homeReturnTab, readHomeReturn } from '../homeReturn'
import { peerInsightCollectionStatus, peerInsightFreshnessStatus, peerInsightValueStatus } from '../components/PeerInsight'
import { formatUtcDateTime, NodeHealthMarker } from '../components/StatusBadge'
import { LinkedValidatorDiagnostics, LinkedValidatorSection } from '../components/LinkedValidator'
import GeoMapBoundary from '../components/GeoMapBoundary'
import GeoWorldMap from '../components/GeoWorldMap'
import { RealtimeNotice } from '../components/RealtimeNotice'
import { formatNodeDataBytes } from '../formatBytes'
import { formatDuration } from '../formatDuration'
import { nodeDataProgress } from '../nodeData'
import { coverageNotes, coverageSegments, metricCoverage } from '../metricCoverage'
import NodeMetricChart from '../components/NodeMetricChart'
import { METRIC_SERIES_COLORS, type MetricChartSeriesInput, type MetricSeriesKey } from '../components/nodeMetricChartOption'
import {
  formatCountAxis,
  formatCountSample,
  formatMetricRate,
  formatMillisecondsAxis,
  formatMillisecondsSample,
  formatPercentAxis,
  formatPercentSample,
  formatRateAxis,
  formatRateSample,
  integerChartMax,
  niceChartMax,
  percentChartMax,
} from '../components/metricChartAxis'
import { MetricRow } from '../components/MetricRow'
import { ValidatorActivityBadge } from '../components/ValidatorActivityBadge'
import { HEAD_COMPARISON_TONE_CLASS, LastReportAge, networkHeadComparison, networkHeadOffset, validHeight } from '../components/NodeDetailObservations'
import { SummaryMetricCard } from '../components/SummaryMetricCard'
import { OverviewBand } from '../components/OverviewBand'
import { geoMapStatus, nodeGeoOverview } from '../homeGeo'
import { NODE_PEER_COUNTRIES_HEADING } from '../components/geoPresentation'
import { CardX } from '../components/ui/card-x'
import { Alert, AlertDescription } from '../components/ui/alert'
import { Spinner } from '../components/ui/spinner'
import { SURFACE_CARD_STATIC } from '../lib/surface'
import { DataTooltip } from '../components/ui/data-tooltip'
import { Button } from '../components/ui/button'
import { cn } from '../lib/utils'

const PAGE = 'min-w-0 p-4'
// Info/chart tier; the summary tiles sit one step brighter, the disclosures one step dimmer.
const CARD = cn('min-w-0 rounded-md border-none', SURFACE_CARD_STATIC)

export function NodePage() {
  const { nodeId = '' } = useParams()
  const { generation, resetting, realtime } = useHomeRealtimeContext()
  const nodeQuery = usePublicNode(nodeId, generation)
  const historyQuery = usePublicNodeHistory(nodeId, generation)
  const metricsQuery = usePublicNodeMetrics(nodeId, generation)
  // The same-tab return (#224): Home's Node links record where the reader was
  // reading, and every way back hands that departure on again — rebuilt into an
  // ordinary Home URL through Home's own filter vocabulary, never navigated to
  // verbatim, so nothing a Node detail page could carry becomes a redirect.
  const location = useLocation()
  const departure = readHomeReturn(location.state, homeReturnTab())
  const returnState = departure === null ? undefined : { homeReturn: departure }

  if (resetting) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><p role="status" className="mt-3 text-sm text-muted-foreground">Revalidating Node access…</p></section>
  if (nodeQuery.isPending) return <section className={cn(PAGE, 'relative min-h-32')}><RealtimeNotice realtime={realtime} /><Spinner label="Loading Node"><span className="text-sm text-muted-foreground">Loading Node…</span></Spinner></section>
  if (nodeQuery.error && !nodeQuery.data) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><Alert variant="destructive" className="mt-3 rounded-md border-none bg-red-400/10"><AlertDescription>{nodeQuery.error instanceof Error ? nodeQuery.error.message : 'Unable to load Node'}</AlertDescription></Alert><Link className="mt-3 inline-flex min-h-11 min-w-11 items-center text-sm font-medium text-emerald-600 hover:underline dark:text-emerald-400" to={homeReturnHref(departure)} state={returnState}>Back to Home</Link></section>
  if (!nodeQuery.data) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><p role="status" className="mt-3 text-sm text-muted-foreground">Node unavailable.</p><Link className="mt-3 inline-flex min-h-11 min-w-11 items-center text-sm font-medium text-emerald-600 hover:underline dark:text-emerald-400" to={homeReturnHref(departure)} state={returnState}>Back to Home</Link></section>

  const node = nodeQuery.data
  // The Node Peer Country View is a projection of exactly this Node. The Node
  // response has already loaded, so the map slot is never Starting or
  // Unavailable here; its status is the Server's own Geo state.
  const nodeGeo = nodeGeoOverview(node)
  const nodeGeoStatus = geoMapStatus(nodeGeo, { loading: false, hasProjection: true })
  const health = nodeHealthPresentation(node.health)
  const blockInterval = latestBlockInterval(historyQuery.data)
  const metricHistory = metricsQuery.data
  const metricHistoryMessage = metricHistory
    ? undefined
    : metricsQuery.isPending
      ? 'Loading metric history…'
      : metricsQuery.error
        ? 'Metric history unavailable'
        : undefined
  // The shared helper validates both byte counts, so the hero reads the one ratio
  // the Home card's Disk column and the Home Disk sort read (#242) instead of
  // guarding the same two fields a second time.
  const nodeDataProgressValue = nodeDataProgress(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)
  const metricWindowTitle = describeMetricWindow(metricHistory)
  const validatorNode = nodeUsesValidatorComposition(node)

  return <section className={PAGE}>
    <div data-slot="node-detail-breadcrumb" className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs font-medium text-muted-foreground">
        <Link className="inline-flex min-h-11 min-w-11 items-center hover:text-foreground" to={homeReturnHref(departure)} state={returnState}><ArrowLeft size={16} aria-hidden="true" />Back to Home</Link>
        <span aria-hidden="true">/</span>
        <span>Node detail</span>
      </div>
      <RealtimeNotice realtime={realtime} />
    </div>
    {nodeQuery.isRefetchError && <p role="status" className="mt-2 text-sm text-destructive">Node refresh failed; showing the last successful Node data.</p>}

    <NodeIdentityHeader node={node} />

    {health.tone !== 'ok' && <p className="m-0 mt-2 break-words text-sm text-warning-foreground dark:text-warning">{node.healthReason}</p>}

    {/* Both roles share geometry and DOM order; only performance is conditional.
        The band is Home's own OverviewBand, so card size, the 5:6 column split,
        the map track and the below-lg map-first order cannot drift between the
        two pages. */}
    <OverviewBand
      className="mt-4"
      dataSlot="node-overview"
      mapSlot="node-map"
      mapFirst
      metricsLabel="Node key summary"
      metrics={<>
        <SummaryTile label="Head" value={formatNumber(node.currentHead)} detail={headLagDetail(node)} />
        <SummaryTile label="Sync" value={nodeComponentStateLabel(node.syncState)} detail={syncStateDetail(node)} />
        <SummaryTile label="Peers" value={peerCount(node.peers)} detail={peerDirectionSummary(node.peers)} />
        <SummaryTile label="Uptime" value={formatDuration(node.processUptimeMs)} />
        <SummaryTile label="Block interval" value={blockInterval.value} detail={blockInterval.value === 'Unknown' ? blockInterval.detail : undefined} />
        <SummaryTile label="Transactions / block" value={formatNumber(node.latestBlockTransactionCount)} />
      </>}
      map={<GeoMapBoundary label={NODE_PEER_COUNTRIES_HEADING}>
        <GeoWorldMap overview={nodeGeo} status={nodeGeoStatus} heading={NODE_PEER_COUNTRIES_HEADING} />
      </GeoMapBoundary>}
    />

    <div className="mt-4 grid grid-cols-1 gap-3 lg:grid-cols-3">
      <ChainStateGroup node={node} />
      <ProcessGroup node={node} nodeDataProgressValue={nodeDataProgressValue} />
      <HostResourcesGroup node={node} />
    </div>

    {validatorNode && <LinkedValidatorSection node={node} variant="detail" />}

    <section data-slot="node-metrics-section" className="mt-4" aria-labelledby="node-metrics-title">
      <header className="mb-2">
        <h2 id="node-metrics-title" className="m-0 text-lg font-semibold">{metricWindowTitle}</h2>
      </header>
      <div className="grid auto-rows-fr grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
        <NodeMetricCard
          label="Process CPU"
          unit="%"
          value={formatPercent(node.processCpuPercent)}
          detail={processStatusDetail(node.processState)}
          series={[{ key: 'cpu', label: 'Process CPU', points: metricHistory?.processCpuPercent ?? [], coverage: metricCoverage(metricHistory, 'processCpuPercent'), area: true }]}
          history={metricHistory}
          max={percentChartMax(metricHistory?.processCpuPercent ?? [])}
          axisFormat={formatPercentAxis}
          valueFormat={formatPercentSample}
          historyMessage={metricHistoryMessage}
        />
        <NodeMetricCard
          label="Process memory"
          unit="%"
          value={formatPercent(node.processMemoryPercent)}
          detail={processStatusDetail(node.processState)}
          series={[{ key: 'memory', label: 'Process memory', points: metricHistory?.processMemoryPercent ?? [], coverage: metricCoverage(metricHistory, 'processMemoryPercent'), area: true }]}
          history={metricHistory}
          max={percentChartMax(metricHistory?.processMemoryPercent ?? [])}
          axisFormat={formatPercentAxis}
          valueFormat={formatPercentSample}
          historyMessage={metricHistoryMessage}
        />
        <NodeMetricCard
          label="Host network"
          value={formatMetricRate(node.hostNetworkTxBytesPerSec)}
          valueLabel="Upload"
          series={[
            { key: 'upload', label: 'Upload', points: metricHistory?.networkTxBytesPerSec ?? [], coverage: metricCoverage(metricHistory, 'networkTxBytesPerSec') },
            { key: 'download', label: 'Download', points: metricHistory?.networkRxBytesPerSec ?? [], coverage: metricCoverage(metricHistory, 'networkRxBytesPerSec') },
          ]}
          showLegend
          history={metricHistory}
          axisFormat={formatRateAxis}
          valueFormat={formatRateSample}
          historyMessage={metricHistoryMessage}
        />
        <NodeMetricCard
          label="Peer connections"
          value={peerCount(node.peers)}
          detail={peerBreakdown(node.peers)}
          integer
          series={[
            { key: 'inbound', label: 'Inbound', points: metricHistory?.peerInboundCount ?? [], coverage: metricCoverage(metricHistory, 'peerInboundCount') },
            { key: 'outbound', label: 'Outbound', points: metricHistory?.peerOutboundCount ?? [], coverage: metricCoverage(metricHistory, 'peerOutboundCount') },
          ]}
          showLegend
          history={metricHistory}
          axisFormat={formatCountAxis}
          valueFormat={formatCountSample}
          historyMessage={metricHistoryMessage}
        />
        <NodeMetricCard
          label="Block interval"
          kind="bar"
          value={blockInterval.value}
          detail={historyQuery.error ? 'History unavailable' : blockInterval.detail}
          series={[{ key: 'blockInterval', label: 'Block interval', points: metricHistory?.blockIntervalMs ?? [], coverage: metricCoverage(metricHistory, 'blockIntervalMs') }]}
          history={metricHistory}
          axisFormat={formatMillisecondsAxis}
          valueFormat={formatMillisecondsSample}
          historyMessage={metricHistoryMessage}
        />
        <NodeMetricCard
          label="Transactions / block"
          kind="bar"
          integer
          value={formatNumber(node.latestBlockTransactionCount)}
          series={[{ key: 'transactions', label: 'Transactions / block', points: metricHistory?.transactionCount ?? [], coverage: metricCoverage(metricHistory, 'transactionCount') }]}
          history={metricHistory}
          axisFormat={formatCountAxis}
          valueFormat={formatCountSample}
          historyMessage={metricHistoryMessage}
        />
      </div>
    </section>

    {/* Advanced diagnostics, after the deck rather than inside the performance
        card, so expanding them never hides the monitoring charts. Only a
        confirmed Validator has them at all. */}
    {validatorNode && node.validator && <LinkedValidatorDiagnostics node={node} validator={node.validator} />}

  </section>
}

/** Node Detail's six key metrics: Home's exact summary card, read-only (no
 *  hover ring) and keeping its own `node-summary-tile` slot. */
function SummaryTile({ label, value, detail }: { label: string; value: ReactNode; detail?: ReactNode }) {
  return <SummaryMetricCard label={label} value={value} caption={detail} hoverable={false} dataSlot="node-summary-tile" />
}

/** `dense` tightens the row rhythm only: the card, its title line and its note
 *  stay the shared Node Detail card, so the three panels keep one geometry. */
function NodeInfoGroup({ title, label, note, dense = false, children }: { title: string; label: string; note?: string; dense?: boolean; children: ReactNode }) {
  return (
    <CardX
      bordered={false}
      role="region"
      aria-label={label}
      data-slot="node-info-group"
      className={CARD}
      contentClassName={cn('flex h-full min-w-0 flex-col', dense ? 'gap-1' : 'gap-1.5')}
    >
      <h2 className="m-0 flex flex-wrap items-baseline gap-x-2 gap-y-1 text-sm font-medium leading-5">
        {title}
      </h2>
      <div className={cn('grid grid-cols-1', dense ? 'gap-1' : 'gap-1.5')}>{children}</div>
      {note && <div className="mt-auto border-t border-border/60 pt-2"><p className="m-0 text-[11px] leading-4 text-muted-foreground">{note}</p></div>}
    </CardX>
  )
}



function nodeUsesValidatorComposition(node: PublicNode): boolean {
  // Identity correspondence alone is not a staking verdict. Retain a last-good
  // positive verdict, but qualify its currency in the header and diagnostics.
  return node.validator?.currentValidatorStatus === 'validator'
}

function nodeRoleLabel(node: PublicNode): string | null {
  if (nodeUsesValidatorComposition(node)) return 'Validator'
  return node.validator?.currentValidatorStatus === 'not_validator'
    && node.validator.currentValidatorStatusState === 'current' ? 'Observer' : null
}

function nodeSyncStateLabel(node: PublicNode): string | null {
  const label = nodeComponentStateLabel(node.syncState)
  return label === 'Unknown' ? null : label
}

/** The Sync tile names the Report's component state, and says so: the Head tile
 *  already carries the head lag, and repeating it here read as a chain verdict
 *  the evidence does not support. An unnamed state keeps the comparison's own
 *  reason, so the tile never reads as "fully synced" by omission. */
function syncStateDetail(node: PublicNode): string | undefined {
  return nodeSyncStateLabel(node) === null
    ? networkHeadComparison(node).reason
    : 'Data collection state; not chain sync'
}

type HeadLagPresentation = { label: string; toneClass: string }

/** Never certify a last-good or low-confidence comparison as at network head.
 *  The gate, the signed offset and the display tone are the shared ones the Home
 *  card reads too (#239); only this page's wording is local to it. */
function headLagPresentation(node: PublicNode): HeadLagPresentation | null {
  const offset = networkHeadOffset(node)
  if (offset === null) return null
  const blocks = offset.blocks
  const label = blocks === 0 ? 'At network head'
    : Math.abs(blocks).toLocaleString() + ' block' + (Math.abs(blocks) === 1 ? '' : 's') + (blocks > 0 ? ' behind' : ' ahead')
  return { label, toneClass: HEAD_COMPARISON_TONE_CLASS[offset.tone] }
}

function headLagDetail(node: PublicNode): string | undefined {
  return headLagPresentation(node)?.label ?? networkHeadComparison(node).reason
}

function peerDirectionSummary(insight: PublicNode['peers']): string {
  // Keep collection/freshness warnings even when retained counts are available.
  return peerBreakdown(insight)
}


function processStateDot(state: string | null | undefined): string {
  const value = typeof state === 'string' ? state.trim().toLowerCase() : ''
  if (value === 'ok') return 'bg-success'
  if (value === 'starting') return 'bg-warning'
  if (value === 'error') return 'bg-destructive'
  return 'bg-muted-foreground'
}

function NodeIdentityHeader({ node }: { node: PublicNode }) {
  const syncStateLabel = nodeSyncStateLabel(node)
  const role = nodeRoleLabel(node)
  const validatorNode = nodeUsesValidatorComposition(node)
  const verdictState = node.validator?.currentValidatorStatusState
  return <header className="mt-1 flex min-w-0 flex-col gap-1" aria-labelledby="node-detail-title">
    <div data-slot="node-identity-main" className="flex min-w-0 flex-1 items-start gap-2">
      <NodeHealthMarker health={node.health} />
      <div className="min-w-0">
        <h1 id="node-detail-title" className="m-0 min-w-0 max-w-full break-words text-lg font-semibold leading-tight md:text-2xl">{nodeDisplayName(node)}</h1>
        <div className="mt-0.5 flex flex-wrap items-center gap-x-1.5 gap-y-1 text-xs text-muted-foreground">
          {syncStateLabel && <><span>{syncStateLabel}</span><span aria-hidden="true">·</span></>}
          {role && <><span>{role}{validatorNode && verdictState !== 'current' ? (verdictState === 'stale' ? ' (last confirmed · stale)' : ' (currency unconfirmed)') : ''}</span><span aria-hidden="true">·</span></>}
          {validatorNode && <><ValidatorActivityBadge validator={node.validator} identityReason={node.validatorIdentityReason} variant="inline" /><span aria-hidden="true">·</span></>}
          <LastReportAge timestamp={node.lastReportAt} variant="inline" />
        </div>
        <NodeIdLine key={node.nodeId} nodeId={node.nodeId} />
      </div>
    </div>
  </header>
}

function NodeIdLine({ nodeId }: { nodeId: string }) {
  const [copied, setCopied] = useState(false)
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(nodeId)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 2000)
    } catch {
      // Clipboard access can be denied; the full identifier stays selectable.
    }
  }
  return <p className="m-0 mt-0.5 flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1 text-[11px] text-muted-foreground">
    <span>Node ID</span>
    <code className="break-all font-mono">{nodeId}</code>
    <DataTooltip as="span" content="Copy full Node ID">
      <Button variant="ghost" size="icon-sm" aria-label="Copy full Node ID" className="text-muted-foreground" onClick={() => { void copy() }}>
        {copied ? <Check className="size-3.5" aria-hidden="true" /> : <Copy className="size-3.5" aria-hidden="true" />}
      </Button>
    </DataTooltip>
  </p>
}

function ChainStateGroup({ node }: { node: PublicNode }) {
  const consensus = node.consensus
  const qcValue = consensus?.highestQcBlock
  const lockedValue = consensus?.highestLockBlock
  const committedValue = consensus?.highestCommitBlock
  const observedValue = node.currentHead
  const networkValue = node.networkReferenceHead
  const qc = validHeight(qcValue) ? qcValue : null
  const locked = validHeight(lockedValue) ? lockedValue : null
  const committed = validHeight(committedValue) ? committedValue : null
  const observed = validHeight(observedValue) ? observedValue : null
  const network = validHeight(networkValue) ? networkValue : null
  const lag = headLagPresentation(node)
  const hasRows = qc != null || locked != null || committed != null || observed != null || network != null || lag != null
  const staleParts = [
    typeof consensus?.freshness === 'string' && consensus.freshness !== 'current' ? 'freshness ' + consensus.freshness : null,
    typeof consensus?.state === 'string' && consensus.state !== 'ok' ? 'collection ' + consensus.state : null,
  ].filter((part): part is string => part != null)
  // Three heights that agree in the healthy case are noise repeated three
  // times; Observed Head, Network Head and the lag are what the reader compares.
  // The labels stay exactly as they were — only weight and rhythm change.
  return <NodeInfoGroup dense title="Chain state" label="Node chain state">
    {(qc != null || locked != null || committed != null) && <div className="grid grid-cols-1 gap-1 border-b border-border/60 pb-1.5" role="group" aria-label="Consensus heads">
      {qc != null && <MetricRow layout="compact" label="QC Head" value={qc.toLocaleString()} />}
      {locked != null && <MetricRow layout="compact" label="Locked Head" value={locked.toLocaleString()} />}
      {committed != null && <MetricRow layout="compact" label="Committed Head" value={committed.toLocaleString()} />}
    </div>}
    {observed != null && <MetricRow label="Observed Head" value={observed.toLocaleString()} />}
    {network != null && <MetricRow label="Network Head" value={network.toLocaleString()} />}
    {lag != null && <MetricRow label="Head lag" value={<span className={lag.toneClass}>{lag.label}</span>} />}
    {!hasRows && <p className="m-0 text-[11px] text-muted-foreground">Chain state has not been observed yet.</p>}
    {staleParts.length > 0 && <p data-slot="chain-state-stale" className="m-0 text-[11px] text-muted-foreground">Last-good consensus observation · {staleParts.join(' · ')}</p>}
  </NodeInfoGroup>
}

function processCollectionLabel(state: string | null | undefined): string {
  return state?.trim().toLowerCase() === 'ok' ? 'Successful' : nodeComponentStateLabel(state)
}

function ProcessGroup({ node, nodeDataProgressValue }: { node: PublicNode; nodeDataProgressValue: number | null }) {
  const bytes = formatNodeDataBytes(finiteNonNegative(node.nodeDataDirectorySizeBytes) ? node.nodeDataDirectorySizeBytes : null, finiteNonNegative(node.nodeDataDirectoryCapacityBytes) ? node.nodeDataDirectoryCapacityBytes : null)
  const started = formatUtcDateTime(node.processStartedAt)
  return <NodeInfoGroup dense title="Process" label="PlatON process" note="Last collection result; not a live process status">
    <MetricRow label="Collection" value={<span className="inline-flex items-center gap-1.5"><span className={cn('inline-block size-2 shrink-0 rounded-full', processStateDot(node.processState))} aria-hidden="true" />{processCollectionLabel(node.processState)}</span>} />
    {started !== 'Unknown' && <MetricRow label="Started" value={started} />}
    {(bytes != null || nodeDataProgressValue != null) && <div className="grid grid-cols-1 gap-1.5" role="group" aria-label="Node data directory">
      {bytes != null && <MetricRow label="Storage" value={bytes} />}
      {nodeDataProgressValue != null && <MetricRow label="Data usage" value={formatPercent(nodeDataProgressValue)} detail="Directory size against its filesystem capacity, not whole-Host disk usage" />}
    </div>}
  </NodeInfoGroup>
}

function finiteNonNegative(value: number | null | undefined): value is number {
  return value != null && Number.isFinite(value) && value >= 0
}

function HostResourcesGroup({ node }: { node: PublicNode }) {
  const resources = [['CPU', node.hostCpuPercent], ['Memory', node.hostMemoryPercent], ['Storage', node.hostStoragePercent]] as const
  const upload = finiteNonNegative(node.hostNetworkTxBytesPerSec)
  const download = finiteNonNegative(node.hostNetworkRxBytesPerSec)
  return <NodeInfoGroup dense title="Host resources" label="Shared Host resources" note="Collected once per Agent; shared by every Node it monitors">
    {resources.map(([label, value]) => finiteNonNegative(value) ? <MetricRow key={label} label={label} value={formatPercent(value)} progress={value} progressStatus="info" /> : null)}
    {(upload || download) && <MetricRow label="Network" value={<span className="inline-flex flex-wrap items-center justify-end gap-x-2 tabular-nums">{upload && <span>↑ {formatMetricRate(node.hostNetworkTxBytesPerSec)}</span>}{download && <span>↓ {formatMetricRate(node.hostNetworkRxBytesPerSec)}</span>}</span>} />}
    {!resources.some(([, value]) => finiteNonNegative(value)) && !upload && !download && <p className="m-0 text-[11px] text-muted-foreground">Host resources have not been observed yet.</p>}
  </NodeInfoGroup>
}

type MetricSeries = {
  /** Emerald series colour slot; the card, the curve and the legend read it once. */
  key: MetricSeriesKey
  label: string
  points: PublicMetricPoint[]
  /** Server-answered observation/coverage evidence for this direction. */
  coverage?: PublicMetricSeriesCoverage
  /** Emerald's transparent vertical gradient, for the single-direction CPU and
   *  memory curves only. */
  area?: boolean
}

type MetricChartKind = 'line' | 'bar'

/** The chart's tooltip states each retained sample as the Server reported it —
 *  unrounded percent, unscaled byte rate — while the card's own top-right value
 *  keeps the page's display formatting. The formatters are module-level so the
 *  option is not rebuilt on every render. */

function NodeMetricCard({ label, unit, value, valueLabel, detail, kind = 'line', series, showLegend = false, history, max: fixedMax, axisFormat, valueFormat, integer = false, historyMessage, className = '' }: {
  label: string
  unit?: string
  value: string
  valueLabel?: string
  detail?: string
  kind?: MetricChartKind
  series: MetricSeries[]
  showLegend?: boolean
  history?: PublicNodeMetricHistory
  /** Display ceiling; without one the window's own observations set a nice ceiling. */
  max?: number
  axisFormat: (value: number) => string
  valueFormat: (value: number) => string
  integer?: boolean
  historyMessage?: string
  className?: string
}) {
  const seconds = Number.isFinite(history?.windowSeconds) && (history?.windowSeconds ?? 0) > 0 ? Math.round(history?.windowSeconds as number) : 60
  const fromMs = history ? Date.parse(history.from) : Number.NaN
  const toMs = history ? Date.parse(history.to) : Number.NaN
  const validWindow = Number.isFinite(fromMs) && Number.isFinite(toMs) && toMs > fromMs
  // A direction series that has no retained sample must not disappear behind
  // its available partner: name it explicitly while the partner keeps plotting.
  const missingDirections = series.length > 1 && series.some((item) => item.points.length > 0)
    ? series.filter((item) => item.points.length === 0).map((item) => item.label)
    : []
  // Gaps, an unobserved tail and a never-observed direction are stated beside
  // the value: the reader must not have to infer absence from a straight line.
  const coverageText = coverageNotes(series)
  // The window is the evidence: observations outside it are dropped and the
  // Server's own silence list decides where a run ends, so nothing is joined
  // across a stretch nobody observed and nothing is filled with a zero.
  const segments = series.map((item) => (validWindow ? coverageSegments(item.points, fromMs, toMs, item.coverage?.gaps) : []))
  const plots: MetricChartSeriesInput[] = series.map((item, index) => ({
    label: item.label,
    color: METRIC_SERIES_COLORS[item.key],
    area: item.area,
    segments: segments[index] ?? [],
  }))
  const observations = segments.flat(2)
  const observedMax = observations.length > 0 ? Math.max(...observations.map((item) => item.value)) : 0
  const max = fixedMax ?? (integer ? integerChartMax(observedMax) : niceChartMax(observedMax))
  // Multi-direction cards explain their curves under the plot, never under the
  // title, so the header stays title-left / current-value-right on every card.
  const legend = showLegend ? <MetricSeriesLegend label={label} series={series} /> : undefined
  // The accessible description states the same thing the curve does: which
  // directions, over which real window, and what the Server measured missing.
  const description = (observations.length > 0
    ? series.map((item) => item.label).join(' and ') + ' values from ' + seconds + ' seconds ago to now'
    : label + ': ' + (historyMessage ?? 'No samples in the last minute'))
    + (coverageText ? '. ' + coverageText : '')
  return (
    <CardX bordered={false} role="article" data-slot="node-metric-card" className={cn('min-w-0', CARD, className)} contentClassName="flex h-full min-w-0 flex-col gap-1.5">
      <div data-slot="node-metric-header" className="flex min-h-10 min-w-0 items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="m-0 text-sm font-medium">{label}</h3>
          {unit && <p className="m-0 mt-0.5 text-[11px] leading-4 text-muted-foreground">{unit}</p>}
        </div>
        <div className="flex shrink-0 flex-col items-end gap-0.5">
          <strong data-slot="node-metric-value" className="text-right text-xl font-bold leading-none tracking-tight tabular-nums">{value}</strong>
          {valueLabel && <span className="text-[10px] leading-none text-muted-foreground">{valueLabel}</span>}
        </div>
      </div>
      <div className="min-h-8 flex-1">
        {detail && <p className="m-0 break-words text-[11px] leading-4 text-muted-foreground">{detail}</p>}
        {missingDirections.length > 0 && <p className="m-0 text-[11px] italic text-muted-foreground">{missingDirections.join(' and ')} unavailable in this window</p>}
        {coverageText && <p data-slot="node-metric-coverage" className="m-0 text-[11px] leading-4 text-muted-foreground">{coverageText}</p>}
      </div>
      <NodeMetricChart
        label={label}
        kind={kind}
        from={fromMs}
        to={toMs}
        max={max}
        series={plots}
        integer={integer}
        axisFormat={axisFormat}
        valueFormat={valueFormat}
        seconds={seconds}
        message={historyMessage}
        description={description}
        legend={legend}
      />
    </CardX>
  )
}

function MetricSeriesLegend({ label, series }: { label: string; series: MetricSeries[] }) {
  return (
    <div role="group" className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1" aria-label={label + ' chart legend'}>
      {series.map((item) => (
        <span key={item.label} className="inline-flex min-w-0 items-center gap-1.5 text-[10px] text-muted-foreground">
          <i className="inline-block size-2 shrink-0 rounded-full" style={{ backgroundColor: METRIC_SERIES_COLORS[item.key] }} aria-hidden="true" />
          {item.label}
        </span>
      ))}
    </div>
  )
}









function describeMetricWindow(history: PublicNodeMetricHistory | undefined): string {
  if (!history) return 'Latest 60 seconds'
  const seconds = Number.isFinite(history.windowSeconds) && history.windowSeconds > 0 ? history.windowSeconds : 60
  return 'Latest ' + seconds + ' seconds'
}


function latestBlockInterval(history: ReturnType<typeof usePublicNodeHistory>['data']): { value: string; detail: string } {
  const blocks = history?.filter((item) => item.height != null && item.blockTimeMs != null).slice(0, 2) ?? []
  if (blocks.length < 2) return { value: 'Unknown', detail: 'Two Block Summaries are required' }
  const [latest, previous] = blocks
  if (latest.height == null || previous.height == null || latest.blockTimeMs == null || previous.blockTimeMs == null || latest.height - previous.height !== 1) {
    return { value: 'Unknown', detail: 'Consecutive Block Summaries unavailable' }
  }
  const elapsed = latest.blockTimeMs - previous.blockTimeMs
  if (elapsed < 0) return { value: 'Unknown', detail: 'Block timestamps are inconsistent' }
  const value = elapsed < 1000 ? elapsed + ' ms' : (elapsed / 1000).toFixed(2) + ' s'
  return { value, detail: latest.height.toLocaleString() + ' → ' + previous.height.toLocaleString() }
}

/** A process value is the retained current Public Projection value; when the
 *  process component is not Current it is explicitly marked as last-good. */
function processStatusDetail(state: string | null | undefined): string | undefined {
  const label = nodeComponentStateLabel(state)
  if (label === 'Current') return undefined
  return 'last-good value retained · collection ' + label
}

function nodeComponentStateLabel(value: string | null | undefined): string {
  const state = typeof value === 'string' ? value.trim().toLowerCase() : undefined
  switch (state) {
    case 'ok': return 'Current'
    case 'starting': return 'Starting'
    case 'error': return 'Error'
    case 'disabled': return 'Disabled'
    case 'unsupported': return 'Unsupported'
    default: return 'Unknown'
  }
}

type NodeHealthPresentation = {
  label: 'Healthy' | 'Unhealthy' | 'Unknown'
  tone: 'ok' | 'warning' | 'error' | 'neutral'
}

function nodeHealthPresentation(value: string | null | undefined): NodeHealthPresentation {
  const health = typeof value === 'string' ? value.trim().toLowerCase() : undefined
  switch (health) {
    case 'healthy': return { label: 'Healthy', tone: 'ok' }
    case 'unhealthy': return { label: 'Unhealthy', tone: 'error' }
    default: return { label: 'Unknown', tone: 'neutral' }
  }
}

function nodeDisplayName(node: Pick<PublicNode, 'displayName' | 'nodeId'>): string {
  const displayName = typeof node.displayName === 'string' ? node.displayName.trim() : ''
  return displayName || node.nodeId
}

function formatNumber(value: number | null | undefined): string {
  return !finiteNonNegative(value) ? 'Unknown' : value.toLocaleString()
}

function formatPercent(value: number | null | undefined): string {
  return !finiteNonNegative(value) ? 'Unknown' : value.toFixed(1) + '%'
}

function peerCount(insight: PublicNode['peers']): string {
  return formatNumber(insight.peerCount)
}

function peerBreakdown(insight: PublicNode['peers']): string {
  const collection = peerInsightCollectionStatus(insight)
  const freshness = peerInsightFreshnessStatus(insight)
  const value = peerInsightValueStatus(insight)
  const hasValue = value !== 'Unknown'
  const directionSummary = formatNumber(insight.inboundCount) + ' inbound · ' + formatNumber(insight.outboundCount) + ' outbound'
  const qualifiers: string[] = []

  if (collection === 'Error') qualifiers.push('Collection failed')
  else if (collection !== 'Current') qualifiers.push('Collection ' + collection.toLowerCase())
  if (freshness === 'Stale') qualifiers.push('Stale')
  if (freshness !== 'Current' && freshness !== 'Stale') qualifiers.push('Freshness unknown')

  if (!hasValue) {
    if (qualifiers.length === 0) qualifiers.push('Value unknown')
    qualifiers.push('No successful Peer snapshot is available')
    qualifiers.push(directionSummary)
    return qualifiers.join('; ')
  }

  if (qualifiers.length > 0) {
    qualifiers.push('Showing last successful snapshot')
    if (value === 'Empty') qualifiers.push('authoritative zero')
    qualifiers.push(directionSummary)
    return qualifiers.join('; ')
  }

  if (value === 'Empty') return 'Empty; authoritative successful zero; ' + directionSummary
  return directionSummary
}
