import { ArrowLeft, Check, Copy } from 'lucide-react'
import { useId, useState } from 'react'
import type { ReactNode } from 'react'
import { Link, useParams } from 'react-router'
import {
  usePublicNode,
  usePublicNodeHistory,
  usePublicNodeMetrics,
} from '../api/public'
import type { PublicMetricPoint, PublicNode, PublicNodeMetricHistory } from '../api/generated'
import { useHomeRealtimeContext } from '../layouts/HomeLayout'
import { peerInsightCollectionStatus, peerInsightFreshnessStatus, peerInsightValueStatus } from '../components/PeerInsight'
import { formatUtcDateTime, NodeHealthMarker } from '../components/StatusBadge'
import { LinkedValidatorSection } from '../components/LinkedValidator'
import GeoMapBoundary from '../components/GeoMapBoundary'
import GeoWorldMap from '../components/GeoWorldMap'
import { RealtimeNotice } from '../components/RealtimeNotice'
import { formatNodeDataBytes } from '../formatBytes'
import { formatDuration } from '../formatDuration'
import { nodeDataProgress } from '../nodeData'
import { MetricRow } from '../components/MetricRow'
import { ValidatorActivityBadge } from '../components/ValidatorActivityBadge'
import { LastReportAge, networkHeadComparison, validHeight } from '../components/NodeDetailObservations'
import { SummaryMetricCard } from '../components/SummaryMetricCard'
import { OverviewBand } from '../components/OverviewBand'
import { geoMapStatus, nodeGeoOverview } from '../homeGeo'
import { NODE_PEER_COUNTRIES_HEADING } from '../components/geoPresentation'
import { CardX } from '../components/ui/card-x'
import { Alert, AlertDescription } from '../components/ui/alert'
import { Spinner } from '../components/ui/spinner'
import { SURFACE_CARD_STATIC } from '../lib/surface'
import { Disclosure } from '../components/ui/disclosure'
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

  if (resetting) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><p role="status" className="mt-3 text-sm text-muted-foreground">Revalidating Node access…</p></section>
  if (nodeQuery.isPending) return <section className={cn(PAGE, 'relative min-h-32')}><RealtimeNotice realtime={realtime} /><Spinner label="Loading Node"><span className="text-sm text-muted-foreground">Loading Node…</span></Spinner></section>
  if (nodeQuery.error && !nodeQuery.data) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><Alert variant="destructive" className="mt-3 rounded-md border-none bg-red-400/10"><AlertDescription>{nodeQuery.error instanceof Error ? nodeQuery.error.message : 'Unable to load Node'}</AlertDescription></Alert><Link className="mt-3 inline-flex min-h-11 min-w-11 items-center text-sm font-medium text-emerald-600 hover:underline dark:text-emerald-400" to="/">Back to Home</Link></section>
  if (!nodeQuery.data) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><p role="status" className="mt-3 text-sm text-muted-foreground">Node unavailable.</p><Link className="mt-3 inline-flex min-h-11 min-w-11 items-center text-sm font-medium text-emerald-600 hover:underline dark:text-emerald-400" to="/">Back to Home</Link></section>

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
  const nodeDataProgressValue = finiteNonNegative(node.nodeDataDirectorySizeBytes) && finiteNonNegative(node.nodeDataDirectoryCapacityBytes)
    ? nodeDataProgress(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes) : null
  const metricWindowTitle = describeMetricWindow(metricHistory)
  const validatorNode = nodeUsesValidatorComposition(node)

  return <section className={PAGE}>
    <div data-slot="node-detail-breadcrumb" className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs font-medium text-muted-foreground">
        <Link className="inline-flex min-h-11 min-w-11 items-center hover:text-foreground" to="/"><ArrowLeft size={16} aria-hidden="true" />All Networks</Link>
        <span aria-hidden="true">/</span>
        <span>Node detail</span>
      </div>
      <RealtimeNotice realtime={realtime} />
    </div>
    {nodeQuery.isRefetchError && <p role="status" className="mt-2 text-sm text-destructive">Node refresh failed; showing the last successful Node data.</p>}

    <NodeIdentityHeader node={node} />

    {health.tone !== 'ok' && <p className="m-0 mt-2 break-words text-sm text-warning-foreground dark:text-warning">{node.healthReason}</p>}

    {/* Both roles share geometry and DOM order; only performance is conditional.
        The band is Home's own OverviewBand, so card size, the 5:6 column split
        and the map track cannot drift between the two pages. */}
    <OverviewBand
      className="mt-4"
      dataSlot="node-overview"
      mapSlot="node-map"
      metricsLabel="Node key summary"
      metrics={<>
        <SummaryTile label="Head" value={formatNumber(node.currentHead)} detail={headLagDetail(node)} />
        <SummaryTile label="Sync" value={nodeComponentStateLabel(node.syncState)} detail={headLagDetail(node)} />
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
          tone="blue"
          fixedMax={percentChartMax(metricHistory?.processCpuPercent ?? [])}
          series={[{ label: 'Process CPU', points: metricHistory?.processCpuPercent ?? [] }]}
          from={metricHistory?.from}
          to={metricHistory?.to}
          windowSeconds={metricHistory?.windowSeconds}
          axisFormat={formatPercentAxis}
          historyMessage={metricHistoryMessage}
        />
        <NodeMetricCard
          label="Process memory"
          unit="%"
          value={formatPercent(node.processMemoryPercent)}
          detail={processStatusDetail(node.processState)}
          tone="cyan"
          fixedMax={percentChartMax(metricHistory?.processMemoryPercent ?? [])}
          series={[{ label: 'Process memory', points: metricHistory?.processMemoryPercent ?? [] }]}
          from={metricHistory?.from}
          to={metricHistory?.to}
          windowSeconds={metricHistory?.windowSeconds}
          axisFormat={formatPercentAxis}
          historyMessage={metricHistoryMessage}
        />
        <NodeMetricCard
          label="Host network"
          value={formatRate(node.hostNetworkTxBytesPerSec)}
          valueLabel="Upload"
          tone="blue"
          series={[
            { label: 'Upload', points: metricHistory?.networkTxBytesPerSec ?? [] },
            { label: 'Download', points: metricHistory?.networkRxBytesPerSec ?? [], secondary: true },
          ]}
          showLegend
          from={metricHistory?.from}
          to={metricHistory?.to}
          windowSeconds={metricHistory?.windowSeconds}
          axisFormat={formatRate}
          historyMessage={metricHistoryMessage}
        />
        <NodeMetricCard
          label="Peer connections"
          value={peerCount(node.peers)}
          detail={peerBreakdown(node.peers)}
          tone="blue"
          series={[
            { label: 'Inbound', points: metricHistory?.peerInboundCount ?? [] },
            { label: 'Outbound', points: metricHistory?.peerOutboundCount ?? [], secondary: true },
          ]}
          showLegend
          from={metricHistory?.from}
          to={metricHistory?.to}
          windowSeconds={metricHistory?.windowSeconds}
          axisFormat={formatCountAxis}
          historyMessage={metricHistoryMessage}
        />
        <NodeMetricCard
          label="Block interval"
          value={blockInterval.value}
          detail={historyQuery.error ? 'History unavailable' : blockInterval.detail}
          tone="amber"
          series={[{ label: 'Block interval', points: metricHistory?.blockIntervalMs ?? [] }]}
          from={metricHistory?.from}
          to={metricHistory?.to}
          windowSeconds={metricHistory?.windowSeconds}
          axisFormat={formatMillisecondsAxis}
          historyMessage={metricHistoryMessage}
          chartKind="bar"
        />
        <NodeMetricCard
          label="Transactions / block"
          value={formatNumber(node.latestBlockTransactionCount)}
          tone="violet"
          series={[{ label: 'Transactions / block', points: metricHistory?.transactionCount ?? [] }]}
          from={metricHistory?.from}
          to={metricHistory?.to}
          windowSeconds={metricHistory?.windowSeconds}
          axisFormat={formatCountAxis}
          historyMessage={metricHistoryMessage}
          chartKind="bar"
        />
      </div>
    </section>

    <Disclosure title="Identifiers and technical details" description="Node ID, component states, and reference context">
      <dl className="m-0 grid grid-cols-[repeat(auto-fit,minmax(14rem,1fr))] gap-x-4 gap-y-2">
        <TechnicalFact label="Node ID" value={<code className="break-all font-mono">{node.nodeId}</code>} />
        <TechnicalFact label="Network key" value={<code className="break-all font-mono">{node.networkKey}</code>} />
        <TechnicalFact label="RPC state" value={nodeComponentStateLabel(node.rpcState)} />
        <TechnicalFact label="Sync state" value={nodeComponentStateLabel(node.syncState)} />
        <TechnicalFact label="Consensus state" value={nodeComponentStateLabel(node.consensusState)} />
        <TechnicalFact label="Process collection state" value={node.processState || 'Unknown'} />
        <TechnicalFact label="Raw sync state" value={node.syncState || 'Unknown'} />
        <TechnicalFact label="Resync state" value={node.resyncState || 'Unknown'} />
        {node.resyncProgress && <TechnicalFact label="Resync progress" value={node.resyncProgress} />}
        {node.resyncLastProgressAt && <TechnicalFact label="Resync last progress" value={formatUtcDateTime(node.resyncLastProgressAt)} />}
        <TechnicalFact label="Last report" value={formatUtcDateTime(node.lastReportAt)} />
        <TechnicalFact label="Observation receipt" value={formatUtcDateTime(node.freshness)} />
        {node.validatorIdentityState && <TechnicalFact label="Identity discovery" value={node.validatorIdentityState} />}
        {node.validatorIdentityReason && <TechnicalFact label="Identity context" value={node.validatorIdentityReason} />}
        {!validatorNode && node.validator && <>
          <TechnicalFact label="Chain identity" value={<code className="break-all font-mono">{node.validator.validatorNodeId}</code>} />
          <TechnicalFact label="Staking verdict" value={node.validator.currentValidatorStatus ?? 'Unknown'} />
          <TechnicalFact label="Verdict currency" value={node.validator.currentValidatorStatusState ?? 'Unknown'} />
        </>}
        <TechnicalFact label="Historical high watermark" value={formatNumber(node.historicalHighWatermark)} />
        <TechnicalFact label="Observed Network Head" value={formatNumber(node.networkReferenceHead)} />
        <TechnicalFact label="Reference confidence" value={node.networkReferenceConfidence || 'Unknown'} />
      </dl>
    </Disclosure>
  </section>
}

function TechnicalFact({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs font-medium tracking-wider text-muted-foreground">{label}</dt>
      <dd className="m-0 break-words text-sm tabular-nums">{value}</dd>
    </div>
  )
}

/** Node Detail's six key metrics: Home's exact summary card, read-only (no
 *  hover ring) and keeping its own `node-summary-tile` slot. */
function SummaryTile({ label, value, detail }: { label: string; value: ReactNode; detail?: ReactNode }) {
  return <SummaryMetricCard label={label} value={value} caption={detail} hoverable={false} dataSlot="node-summary-tile" />
}

function NodeInfoGroup({ title, label, note, children }: { title: string; label: string; note?: string; children: ReactNode }) {
  return (
    <CardX
      bordered={false}
      role="region"
      aria-label={label}
      data-slot="node-info-group"
      className={CARD}
      contentClassName="flex h-full min-w-0 flex-col gap-2"
    >
      <h2 className="m-0 flex flex-wrap items-baseline gap-x-2 gap-y-1 text-sm font-medium">
        {title}
      </h2>
      <div className="grid grid-cols-1 gap-2">{children}</div>
      {note && <p className="m-0 mt-auto pt-1 text-[11px] text-muted-foreground">{note}</p>}
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

const TONE_OK = 'text-success'
const TONE_WARNING = 'text-warning-foreground dark:text-warning'
const TONE_ERROR = 'text-destructive'
const TONE_MUTED = 'text-muted-foreground'
const HEAD_LAG_WARNING_MAX = 10

type HeadLagPresentation = { label: string; toneClass: string }

/** Never certify a last-good or low-confidence comparison as at network head. */
function headLagPresentation(node: PublicNode): HeadLagPresentation | null {
  if (networkHeadComparison(node).reason || !validHeight(node.currentHead) || !validHeight(node.networkReferenceHead)) return null
  const blocks = node.networkReferenceHead - node.currentHead
  const toneClass = blocks < 0 ? TONE_MUTED : blocks === 0 ? TONE_OK
    : blocks <= HEAD_LAG_WARNING_MAX ? TONE_WARNING : TONE_ERROR
  const label = blocks === 0 ? 'At network head'
    : Math.abs(blocks).toLocaleString() + ' block' + (Math.abs(blocks) === 1 ? '' : 's') + (blocks > 0 ? ' behind' : ' ahead')
  return { label, toneClass }
}

function headLagDetail(node: PublicNode): string | undefined {
  return headLagPresentation(node)?.label ?? networkHeadComparison(node).reason
}

function peerDirectionSummary(insight: PublicNode['peers']): string {
  // Keep collection/freshness warnings even when retained counts are available.
  return peerBreakdown(insight)
}

/** CPU/Memory curves are read against the bucket the window actually reached; a
 *  fixed 0–100% axis flattens a low-utilisation Node onto the baseline. The
 *  samples themselves are never rescaled, only the displayed range. */
function percentChartMax(points: PublicMetricPoint[]): number {
  const values = points.map((point) => point.value).filter(Number.isFinite)
  const max = values.length > 0 ? Math.max(...values) : 0
  for (const bucket of [5, 10, 25, 50]) if (max <= bucket) return bucket
  return 100
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
  return <header className="mt-2 flex min-w-0 flex-col gap-1" aria-labelledby="node-detail-title">
    <div data-slot="node-identity-main" className="flex min-w-0 flex-1 items-start gap-2">
      <NodeHealthMarker health={node.health} />
      <div className="min-w-0">
        <h1 id="node-detail-title" className="m-0 min-w-0 max-w-full break-words text-lg font-semibold leading-tight md:text-2xl">{nodeDisplayName(node)}</h1>
        <div className="mt-1 flex flex-wrap items-center gap-x-1.5 gap-y-1 text-xs text-muted-foreground">
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
  return <p className="m-0 mt-1 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[11px] text-muted-foreground">
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
  return <NodeInfoGroup title="Chain state" label="Node chain state">
    {qc != null && <MetricRow label="QC Head" value={qc.toLocaleString()} />}
    {locked != null && <MetricRow label="Locked Head" value={locked.toLocaleString()} />}
    {committed != null && <MetricRow label="Committed Head" value={committed.toLocaleString()} />}
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
  return <NodeInfoGroup title="Process" label="PlatON process" note="Last collection result; not a live process status">
    <MetricRow label="Collection" value={<span className="inline-flex items-center gap-1.5"><span className={cn('inline-block size-2 shrink-0 rounded-full', processStateDot(node.processState))} aria-hidden="true" />{processCollectionLabel(node.processState)}</span>} />
    {started !== 'Unknown' && <MetricRow label="Started" value={started} />}
    {(bytes != null || nodeDataProgressValue != null) && <div className="grid grid-cols-1 gap-2" role="group" aria-label="Node data directory">
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
  return <NodeInfoGroup title="Host resources" label="Shared Host resources" note="Collected once per Agent; shared by every Node it monitors">
    {resources.map(([label, value]) => finiteNonNegative(value) ? <MetricRow key={label} label={label} value={formatPercent(value)} progress={value} /> : null)}
    {(upload || download) && <MetricRow label="Network" value={<span className="inline-flex flex-wrap items-center justify-end gap-x-2 tabular-nums">{upload && <span>↑ {formatRate(node.hostNetworkTxBytesPerSec)}</span>}{download && <span>↓ {formatRate(node.hostNetworkRxBytesPerSec)}</span>}</span>} />}
    {!resources.some(([, value]) => finiteNonNegative(value)) && !upload && !download && <p className="m-0 text-[11px] text-muted-foreground">Host resources have not been observed yet.</p>}
  </NodeInfoGroup>
}

type MetricSeries = {
  label: string
  points: PublicMetricPoint[]
  secondary?: boolean
}

type MetricChartKind = 'line' | 'bar'

type MetricTone = 'blue' | 'cyan' | 'violet' | 'amber'

const TONE_TEXT: Record<MetricTone, string> = {
  blue: 'text-blue-600 dark:text-blue-400',
  cyan: 'text-cyan-600 dark:text-cyan-400',
  violet: 'text-violet-600 dark:text-violet-400',
  amber: 'text-amber-500 dark:text-amber-400',
}

type MetricChartProps = {
  label: string
  series: MetricSeries[]
  from?: string
  to?: string
  fixedMax?: number
  axisFormat: (value: number) => string
  message?: string
  kind?: MetricChartKind
  windowSeconds?: number
  toneClass: string
  legend?: ReactNode
}

function NodeMetricCard({ label, unit, value, valueLabel, detail, tone, series, showLegend = false, from, to, fixedMax, axisFormat, historyMessage, chartKind = 'line', windowSeconds, className = '' }: {
  label: string
  unit?: string
  value: string
  valueLabel?: string
  detail?: string
  tone: MetricTone
  series: MetricSeries[]
  showLegend?: boolean
  from?: string
  to?: string
  fixedMax?: number
  axisFormat: (value: number) => string
  historyMessage?: string
  chartKind?: MetricChartKind
  windowSeconds?: number
  className?: string
}) {
  // A direction series that has no retained sample must not disappear behind
  // its available partner: name it explicitly while the partner keeps plotting.
  const missingDirections = series.length > 1 && series.some((item) => item.points.length > 0)
    ? series.filter((item) => item.points.length === 0).map((item) => item.label)
    : []
  const toneClass = TONE_TEXT[tone]
  // Multi-direction cards explain their curves under the plot, never under the
  // title, so the header stays title-left / current-value-right on every card.
  const legend = showLegend ? <MetricSeriesLegend label={label} series={series} toneClass={toneClass} /> : undefined
  return (
    <CardX bordered={false} role="article" data-slot="node-metric-card" className={cn('min-w-0', CARD, className)} contentClassName="flex h-full min-w-0 flex-col gap-2">
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
      </div>
      <MetricChart label={label} series={series} from={from} to={to} fixedMax={fixedMax} axisFormat={axisFormat} message={historyMessage} kind={chartKind} windowSeconds={windowSeconds} toneClass={toneClass} legend={legend} />
    </CardX>
  )
}

function MetricSeriesLegend({ label, series, toneClass }: { label: string; series: MetricSeries[]; toneClass: string }) {
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1" aria-label={label + ' chart legend'}>
      {series.map((item) => (
        <span key={item.label} className="inline-flex min-w-0 items-center gap-1.5 text-[10px] text-muted-foreground">
          <i className={cn('inline-block size-2 shrink-0 rounded-full', item.secondary ? 'bg-cyan-500' : cn('bg-current', toneClass))} aria-hidden="true" />
          {item.label}
        </span>
      ))}
    </div>
  )
}

function MetricChart({ label, series, from, to, fixedMax, axisFormat, message, kind = 'line', windowSeconds, toneClass, legend }: MetricChartProps) {
  const seconds = Number.isFinite(windowSeconds) && (windowSeconds ?? 0) > 0 ? Math.round(windowSeconds as number) : 60
  const gradientId = 'node-metric-fill-' + useId().replaceAll(':', '')
  const fromMs = from ? Date.parse(from) : Number.NaN
  const toMs = to ? Date.parse(to) : Number.NaN
  const validWindow = Number.isFinite(fromMs) && Number.isFinite(toMs) && toMs > fromMs
  const values = series.flatMap((item) => item.points.map((point) => point.value)).filter(Number.isFinite)
  const max = fixedMax ?? niceChartMax(values.length > 0 ? Math.max(...values) : 0)
  const plots = validWindow
    ? series.map((item) => ({
        ...item,
        coordinates: kind === 'bar'
          ? chartBarCoordinates(item.points, fromMs, toMs, max)
          : chartCoordinates(item.points, fromMs, toMs, max),
      }))
    : []
  const hasPoints = plots.some((item) => item.coordinates.length > 0)
  const chartMessage = message ?? (hasPoints ? undefined : 'No samples in the last minute')
  const windowLabel = 'over the last ' + seconds + ' seconds'

  return <div className="mt-auto grid min-w-0 grid-cols-[3rem_minmax(0,1fr)] grid-rows-[7.25rem_auto_auto] lg:grid-rows-[6.25rem_auto_auto] gap-x-2">
    <div className="flex flex-col justify-between pr-1 text-right text-[11px] tabular-nums text-muted-foreground" aria-hidden="true">
      <span>{axisFormat(max)}</span>
      <span>{axisFormat(max / 2)}</span>
      <span>{axisFormat(0)}</span>
    </div>
    <svg viewBox="0 0 600 150" preserveAspectRatio="none" role="img" aria-label={label + ' ' + kind + ' chart ' + windowLabel} className={cn('col-start-2 row-start-1 h-[7.25rem] lg:h-[6.25rem] w-full overflow-visible', toneClass)}>
      <title>{label} {kind} chart {windowLabel}</title>
      <desc>{chartMessage ? label + ': ' + chartMessage : series.map((item) => item.label).join(' and ') + ' values from ' + seconds + ' seconds ago to now'}</desc>
      <defs>
        <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="currentColor" stopOpacity="0.34" />
          <stop offset="100%" stopColor="currentColor" stopOpacity="0.02" />
        </linearGradient>
      </defs>
      <g aria-hidden="true">
        <line x1="0" y1="8" x2="600" y2="8" className="stroke-border [vector-effect:non-scaling-stroke]" />
        <line x1="0" y1="75" x2="600" y2="75" className="stroke-border [vector-effect:non-scaling-stroke]" />
        <line x1="0" y1="142" x2="600" y2="142" className="stroke-border [vector-effect:non-scaling-stroke]" />
      </g>
      {!chartMessage && kind === 'line' && plots.map((item, index) => {
        const line = chartLinePath(item.coordinates)
        const area = plots.length === 1 ? chartAreaPath(item.coordinates) : ''
        return <g key={item.label}>
          {area && <path d={area} fill={'url(#' + gradientId + ')'} />}
          {line && <path data-slot="node-metric-chart-line" className={cn('[vector-effect:non-scaling-stroke]', item.secondary ? 'fill-none stroke-cyan-500' : 'fill-none stroke-current')} strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" d={line} />}
          {item.coordinates.length === 1 && <circle className={cn('[vector-effect:non-scaling-stroke]', item.secondary ? 'fill-cyan-500 stroke-background' : 'fill-current stroke-background')} strokeWidth={1.5} cx={item.coordinates[0].x} cy={item.coordinates[0].y} r={index === 0 ? 4 : 3.5} />}
        </g>
      })}
      {!chartMessage && kind === 'bar' && plots.flatMap((item) => {
        const width = chartBarWidth(item.coordinates.length)
        return item.coordinates.map((point, index) => {
          const height = Math.max(1, 142 - point.y)
          const x = Math.max(0, Math.min(600 - width, point.x - width / 2))
          return <rect key={item.label + '-' + index} data-slot="node-metric-chart-bar" className={cn('opacity-75', item.secondary ? 'fill-cyan-500' : 'fill-current')} x={x} y={142 - height} width={width} height={height} rx={Math.min(2.5, width / 3)} />
        })
      })}
      {chartMessage && <text data-slot="node-metric-chart-empty" className="fill-muted-foreground text-[22px]" x="300" y="78" textAnchor="middle">{chartMessage}</text>}
    </svg>
    <div className="col-start-2 row-start-2 flex justify-between pt-1 text-[11px] tabular-nums text-muted-foreground" aria-hidden="true"><span>{seconds}s</span><span>0s</span></div>
    <div data-slot="node-metric-legend" className="col-start-2 row-start-3 min-h-6 pt-1.5">{legend}</div>
  </div>
}

type ChartCoordinate = { x: number; y: number }

function chartCoordinates(points: PublicMetricPoint[], from: number, to: number, max: number): ChartCoordinate[] {
  const coordinates = points
    .map((point) => ({ sampledAt: Date.parse(point.sampledAt), value: point.value }))
    .filter((point) => Number.isFinite(point.sampledAt) && Number.isFinite(point.value))
    .sort((left, right) => left.sampledAt - right.sampledAt)
    .map((point) => ({
      x: Math.max(0, Math.min(600, ((point.sampledAt - from) / (to - from)) * 600)),
      y: 142 - (Math.max(0, Math.min(max, point.value)) / max) * 134,
    }))
  const last = coordinates.at(-1)
  if (last && last.x < 600) coordinates.push({ x: 600, y: last.y })
  return coordinates
}

function chartBarCoordinates(points: PublicMetricPoint[], from: number, to: number, max: number): ChartCoordinate[] {
  return points
    .map((point) => ({ sampledAt: Date.parse(point.sampledAt), value: point.value }))
    .filter((point) => Number.isFinite(point.sampledAt) && Number.isFinite(point.value) && point.sampledAt >= from && point.sampledAt <= to)
    .sort((left, right) => left.sampledAt - right.sampledAt)
    .map((point) => ({
      x: ((point.sampledAt - from) / (to - from)) * 600,
      y: 142 - (Math.max(0, Math.min(max, point.value)) / max) * 134,
    }))
}

function chartBarWidth(count: number): number {
  if (count <= 0) return 0
  return Math.max(2, Math.min(18, (600 / count) * 0.64))
}

function chartLinePath(points: ChartCoordinate[]): string {
  return points.map((point, index) => (index === 0 ? 'M' : 'L') + ' ' + point.x.toFixed(2) + ' ' + point.y.toFixed(2)).join(' ')
}

function chartAreaPath(points: ChartCoordinate[]): string {
  if (points.length < 2) return ''
  const first = points[0]
  const last = points.at(-1)
  if (!last) return ''
  return chartLinePath(points) + ' L ' + last.x.toFixed(2) + ' 142 L ' + first.x.toFixed(2) + ' 142 Z'
}

function niceChartMax(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 1
  const magnitude = 10 ** Math.floor(Math.log10(value))
  const normalized = value / magnitude
  const step = normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10
  return step * magnitude
}

function formatRate(value: number | null | undefined): string {
  if (!finiteNonNegative(value)) return 'Unknown'
  const units = ['B/s', 'KiB/s', 'MiB/s', 'GiB/s']
  let scaled = value
  let unit = 0
  while (scaled >= 1024 && unit < units.length - 1) {
    scaled /= 1024
    unit += 1
  }
  const digits = scaled >= 100 ? 0 : scaled >= 10 ? 1 : 2
  return scaled.toFixed(digits) + ' ' + units[unit]
}

function formatCountAxis(value: number): string {
  return Math.round(value).toLocaleString()
}

function formatMillisecondsAxis(value: number): string {
  if (value >= 1000) return Number((value / 1000).toFixed(value >= 10_000 ? 0 : 1)) + 's'
  return Math.round(value) + 'ms'
}

/** The fixed public metrics response owns the window; the section label reads the
 *  real windowSeconds instead of inventing a range. The retained-sample explanation
 *  that used to sit opposite the label is deliberately gone. */
function describeMetricWindow(history: PublicNodeMetricHistory | undefined): string {
  if (!history) return 'Latest 60 seconds'
  const seconds = Number.isFinite(history.windowSeconds) && history.windowSeconds > 0 ? history.windowSeconds : 60
  return 'Latest ' + seconds + ' seconds'
}

function formatPercentAxis(value: number): string {
  if (value >= 100) return '100%'
  if (value <= 0) return '0%'
  return value.toFixed(value < 10 ? 1 : 0) + '%'
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
