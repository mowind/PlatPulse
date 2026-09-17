import { ArrowLeft, ArrowRight, ChevronRight } from 'lucide-react'
import { useId } from 'react'
import type { ReactNode } from 'react'
import { Link, useParams } from 'react-router'
import {
  usePublicNetwork,
  usePublicNode,
  usePublicNodeHistory,
  usePublicNodeMetrics,
  usePublicNodePeerHistory,
  usePublicValidatorAnalytics,
  usePublicValidatorHistory,
} from '../api/public'
import type { PublicMetricPoint, PublicNode, PublicNodeMetricHistory, PublicValidatorInsight } from '../api/generated'
import { useHomeRealtimeContext } from '../layouts/HomeLayout'
import { PeerInsight, peerInsightCollectionStatus, peerInsightFreshnessStatus, peerInsightValueStatus } from '../components/PeerInsight'
import { PeerHistoryInsight, normalizePublicPeerHistory } from '../components/PeerHistoryInsight'
import { GeoInsight } from '../components/GeoInsight'
import { ValidatorInsight } from '../components/ValidatorInsight'
import { ValidatorAnalytics } from '../components/ValidatorAnalytics'
import { formatRelativeTime, formatUtcDateTime, NodeHealthMarker, StatusBadge } from '../components/StatusBadge'
import { LinkedValidatorSection } from '../components/LinkedValidator'
import { RealtimeNotice } from '../components/RealtimeNotice'
import { formatNodeDataBytes } from '../formatBytes'
import { formatDuration } from '../formatDuration'
import { nodeDataProgress } from '../nodeData'
import { MetricRow } from '../components/MetricRow'
import { CardX } from '../components/ui/card-x'
import { Alert, AlertDescription } from '../components/ui/alert'
import { Empty } from '../components/ui/empty'
import { Spinner } from '../components/ui/spinner'
import { SURFACE_CARD } from '../lib/surface'
import { cn } from '../lib/utils'

const PAGE = 'min-w-0 p-4'
const CARD = cn('min-w-0 rounded-md border-none', SURFACE_CARD)
const NODE_CARD_GRID = 'grid grid-cols-1 gap-3 sm:grid-cols-[repeat(auto-fill,minmax(300px,1fr))]'

export function NetworkPage() {
  const { networkKey = '' } = useParams()
  const { generation, resetting, realtime } = useHomeRealtimeContext()
  const query = usePublicNetwork(networkKey, generation)

  if (resetting) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><p role="status" className="mt-3 text-sm text-muted-foreground">Revalidating Home access…</p></section>
  if (query.isPending) return <section className={cn(PAGE, 'relative min-h-32')}><RealtimeNotice realtime={realtime} /><Spinner label="Loading public Network data"><span className="text-sm text-muted-foreground">Network is Starting; loading public data…</span></Spinner></section>
  if (query.error && !query.data) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><Alert variant="destructive" className="mt-3 rounded-md border-none bg-red-400/10"><AlertDescription>Network is Error; {query.error instanceof Error ? query.error.message : 'public Network data is unavailable.'}</AlertDescription></Alert><Link className="mt-3 inline-flex min-h-11 min-w-11 items-center text-sm font-medium text-emerald-600 hover:underline dark:text-emerald-400" to="/">Back to Home</Link></section>
  if (!query.data) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><p role="status" className="mt-3 text-sm text-muted-foreground">Network is Unknown; public data is unavailable.</p><Link className="mt-3 inline-flex min-h-11 min-w-11 items-center text-sm font-medium text-emerald-600 hover:underline dark:text-emerald-400" to="/">Back to Home</Link></section>

  const network = query.data
  return <section className={PAGE} aria-labelledby="network-page-title">
    <nav className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs font-medium" aria-label="Breadcrumb">
      <Link className="inline-flex min-h-11 min-w-11 items-center text-muted-foreground hover:text-foreground" to="/"><ArrowLeft size={16} aria-hidden="true" /> All Networks</Link>
      <span aria-hidden="true" className="text-muted-foreground">/</span>
      <span className="text-muted-foreground">Network overview</span>
    </nav>
    <header className="mt-1 flex flex-col gap-2">
      <div className="min-w-0">
        <h1 id="network-page-title" className="m-0 break-words text-lg font-semibold md:text-2xl">{network.displayName}</h1>
        <p className="m-0 mt-1 text-sm text-muted-foreground">Active PlatON Nodes and network-level public observations.</p>
        <div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-2" aria-label="Network identity and live updates">
          <span className="min-w-0 break-all text-xs text-muted-foreground">Network key <code className="font-mono font-semibold text-foreground">{network.networkKey}</code></span>
          <RealtimeNotice realtime={realtime} />
        </div>
      </div>
    </header>
    {query.isRefetchError && <p role="status" className="mt-3 text-sm text-destructive">Network refresh failed; showing the last successful Network data.</p>}
    <div className="mt-4 grid grid-cols-1 gap-3 lg:grid-cols-2">
      <PeerInsight insight={network.peers} />
      <GeoInsight insight={network.geo} peerState={network.peers.state} />
    </div>
    {network.validators.length > 0 && <>
      <h2 className="mb-2 mt-4 text-lg font-semibold">Validators</h2>
      <div className={NODE_CARD_GRID}>{network.validators.map((validator) => <ValidatorCard key={validator.validatorId} validator={validator} generation={generation} />)}</div>
    </>}
    <CardX bordered={false} data-slot="network-nodes-panel" className={cn('mt-4', CARD)}>
      <section className="min-w-0" aria-labelledby="network-nodes-title">
        <h2 id="network-nodes-title" className="m-0 mb-2 text-lg font-semibold">PlatON Nodes</h2>
        {network.nodes.length === 0
          ? <Empty description={<span role="status">Empty: this Network has no Active Nodes.</span>} />
          : <div className={NODE_CARD_GRID}>{network.nodes.map((node) => <NodeCard node={node} key={node.nodeId} />)}</div>}
      </section>
    </CardX>
  </section>
}

function ValidatorCard({ validator, generation }: { validator: PublicValidatorInsight; generation: number }) {
  const history = usePublicValidatorHistory(validator.validatorId, 20, generation)
  const analytics = usePublicValidatorAnalytics(validator.validatorId, 31, generation)
  return <CardX bordered={false} data-slot="validator-card" className={CARD} contentClassName="flex min-w-0 flex-col gap-2">
    <ValidatorInsight insight={validator} history={history.data?.entries} />
    {analytics.data && <ValidatorAnalytics analytics={analytics.data} compact />}
    {history.error && <p role="status" className="m-0 text-xs text-muted-foreground">Validator history unavailable.</p>}
  </CardX>
}

export function NodePage() {
  const { nodeId = '' } = useParams()
  const { generation, resetting, realtime } = useHomeRealtimeContext()
  const nodeQuery = usePublicNode(nodeId, generation)
  const historyQuery = usePublicNodeHistory(nodeId, generation)
  const metricsQuery = usePublicNodeMetrics(nodeId, generation)
  const peerHistoryQuery = usePublicNodePeerHistory(nodeId, generation)

  if (resetting) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><p role="status" className="mt-3 text-sm text-muted-foreground">Revalidating Node access…</p></section>
  if (nodeQuery.isPending) return <section className={cn(PAGE, 'relative min-h-32')}><RealtimeNotice realtime={realtime} /><Spinner label="Loading Node"><span className="text-sm text-muted-foreground">Loading Node…</span></Spinner></section>
  if (nodeQuery.error && !nodeQuery.data) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><Alert variant="destructive" className="mt-3 rounded-md border-none bg-red-400/10"><AlertDescription>{nodeQuery.error instanceof Error ? nodeQuery.error.message : 'Unable to load Node'}</AlertDescription></Alert><Link className="mt-3 inline-flex min-h-11 min-w-11 items-center text-sm font-medium text-emerald-600 hover:underline dark:text-emerald-400" to="/">Back to Home</Link></section>
  if (!nodeQuery.data) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><p role="status" className="mt-3 text-sm text-muted-foreground">Node unavailable.</p><Link className="mt-3 inline-flex min-h-11 min-w-11 items-center text-sm font-medium text-emerald-600 hover:underline dark:text-emerald-400" to="/">Back to Home</Link></section>

  const node = nodeQuery.data
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
  const nodeDataProgressValue = nodeDataProgress(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)
  const metricWindow = describeMetricWindow(metricHistory)

  return <section className={PAGE}>
    <div data-slot="node-detail-breadcrumb" className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs font-medium text-muted-foreground">
        <Link className="inline-flex min-h-11 min-w-11 items-center hover:text-foreground" to={'/networks/' + node.networkKey}><ArrowLeft size={16} aria-hidden="true" />{node.networkKey}</Link>
        <span aria-hidden="true">/</span>
        <span>Node detail</span>
      </div>
      <RealtimeNotice realtime={realtime} />
    </div>
    {nodeQuery.isRefetchError && <p role="status" className="mt-2 text-sm text-destructive">Node refresh failed; showing the last successful Node data.</p>}

    <header className="mt-2 flex min-w-0 flex-col gap-3 md:flex-row md:items-start md:justify-between" aria-labelledby="node-detail-title">
      <div data-slot="node-identity-main" className="flex min-w-0 flex-1 items-start gap-2">
        <NodeHealthMarker health={node.health} />
        <div className="min-w-0">
          <h1 id="node-detail-title" className="m-0 break-words text-lg font-semibold leading-tight md:text-2xl">{nodeDisplayName(node)}</h1>
          <p className="m-0 mt-1 break-words text-[11px] text-muted-foreground">Node ID <code className="font-mono font-semibold text-foreground">{node.nodeId}</code></p>
        </div>
      </div>
      <dl className="m-0 flex flex-wrap gap-x-5 gap-y-2" aria-label="Node identity facts">
        <div className="min-w-0">
          <dt className="text-xs font-medium tracking-wider text-muted-foreground">Last report</dt>
          <dd className="m-0 text-sm tabular-nums">{formatUtcDateTime(node.lastReportAt)}</dd>
        </div>
      </dl>
    </header>

    {health.tone !== 'ok' && <p className="m-0 mt-2 break-words text-sm text-warning-foreground dark:text-warning">{node.healthReason}</p>}

    <div className="mt-4 grid grid-cols-2 gap-3 lg:grid-cols-4" role="group" aria-label="Node key summary">
      <SummaryTile label="Head" value={formatNumber(node.currentHead)} />
      <SummaryTile label="Sync" value={nodeComponentStateLabel(node.syncState)} detail={formatSyncDetail(node)} />
      <SummaryTile label="Peers" value={peerCount(node.peers)} detail={peerBreakdown(node.peers)} />
      <SummaryTile label="Process uptime" value={formatDuration(node.processUptimeMs)} />
    </div>

    <div className="mt-4 grid grid-cols-1 gap-3 lg:grid-cols-3">
      <NodeInfoGroup title="Chain & consensus" label="Node chain and consensus observations">
        <MetricRow label="QC" value={formatConsensusValue(node.consensus?.highestQcBlock, node.consensus)} />
        <MetricRow label="Locked" value={formatConsensusValue(node.consensus?.highestLockBlock, node.consensus)} />
        <MetricRow label="Committed" value={formatConsensusValue(node.consensus?.highestCommitBlock, node.consensus)} />
        <MetricRow label="Validator" value={formatValidatorMembership(node)} />
        <MetricRow label="Resync" value={nodeComponentStateLabel(node.resyncState)} detail={formatResyncDetail(node)} />
        <MetricRow label="Network reference" value={formatReferenceHead(node)} detail={formatReferenceDetail(node)} />
      </NodeInfoGroup>

      <NodeInfoGroup title="PlatON process & Node Data" label="PlatON process resources">
        <MetricRow label="CPU" value={formatPercent(node.processCpuPercent)} progress={node.processCpuPercent} />
        <MetricRow label="Memory" value={formatPercent(node.processMemoryPercent)} progress={node.processMemoryPercent} />
        <MetricRow label="Started" value={formatUtcDateTime(node.processStartedAt)} />
        <MetricRow label="Process state" value={nodeComponentStateLabel(node.processState)} />
        <div className="mt-1 grid grid-cols-1 gap-2 border-t border-dashed border-border/60 pt-2" role="group" aria-label="Node data directory">
          <MetricRow
            label="Directory usage"
            value={formatPercent(nodeDataProgressValue)}
            detail={(formatNodeDataBytes(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes) ?? 'Unknown') + ' · directory size against the hosting filesystem capacity, not whole-Host disk usage'}
            progress={nodeDataProgressValue}
          />
        </div>
      </NodeInfoGroup>

      <NodeInfoGroup title="Host resources" label="Shared Host resources" note="Collected once per Agent; shared by every Node it monitors">
        <MetricRow label="Host CPU" value={formatPercent(node.hostCpuPercent)} progress={node.hostCpuPercent} />
        <MetricRow label="Host memory" value={formatPercent(node.hostMemoryPercent)} progress={node.hostMemoryPercent} />
        <MetricRow label="Host storage" value={formatPercent(node.hostStoragePercent)} progress={node.hostStoragePercent} />
        <MetricRow label="Host upload" value={formatRate(node.hostNetworkTxBytesPerSec)} />
        <MetricRow label="Host download" value={formatRate(node.hostNetworkRxBytesPerSec)} />
      </NodeInfoGroup>
    </div>

    <LinkedValidatorSection node={node} variant="detail" />

    <section data-slot="node-metrics-section" className="mt-4" aria-labelledby="node-metrics-title">
      <header className="mb-2 flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
        <h2 id="node-metrics-title" className="m-0 text-lg font-semibold">{metricWindow.title}</h2>
        <p className="m-0 text-[11px] text-muted-foreground">{metricWindow.detail}</p>
      </header>
      <div className="grid auto-rows-fr grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
        <NodeMetricCard
          label="Process CPU"
          unit="%"
          value={formatPercent(node.processCpuPercent)}
          detail={processStatusDetail(node.processState)}
          tone="blue"
          fixedMax={100}
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
          fixedMax={100}
          series={[{ label: 'Process memory', points: metricHistory?.processMemoryPercent ?? [] }]}
          from={metricHistory?.from}
          to={metricHistory?.to}
          windowSeconds={metricHistory?.windowSeconds}
          axisFormat={formatPercentAxis}
          historyMessage={metricHistoryMessage}
        />
        <NodeMetricCard
          label="Host network"
          unit="bytes/s"
          value={formatRate(node.hostNetworkTxBytesPerSec)}
          detail={'Host upload · download ' + formatRate(node.hostNetworkRxBytesPerSec)}
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
          unit="count"
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
          unit="ms"
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
          label="Transactions per block"
          unit="tx/block"
          value={formatNumber(node.latestBlockTransactionCount)}
          detail="Transactions in each Block Summary"
          tone="violet"
          series={[{ label: 'Transactions per block', points: metricHistory?.transactionCount ?? [] }]}
          from={metricHistory?.from}
          to={metricHistory?.to}
          windowSeconds={metricHistory?.windowSeconds}
          axisFormat={formatCountAxis}
          historyMessage={metricHistoryMessage}
          chartKind="bar"
        />
      </div>
    </section>

    <NodeDisclosure title="Peer diagnostics" summaryDetail="Peer insight and retained aggregate Peer history">
      <PeerInsight insight={node.peers} />
      <PeerHistoryInsight
        history={peerHistoryQuery.data ? normalizePublicPeerHistory(peerHistoryQuery.data) : undefined}
        error={Boolean(peerHistoryQuery.error)}
        loading={peerHistoryQuery.isPending}
      />
      <p className="m-0 rounded-md border border-border/60 bg-background/40 px-3 py-2 text-xs text-muted-foreground">Network insight is public and redacted: peer addresses and identity lists are never displayed.</p>
    </NodeDisclosure>

    <NodeDisclosure title="Identifiers and technical details" summaryDetail="Node ID, component states, and reference context">
      <dl className="m-0 grid grid-cols-[repeat(auto-fit,minmax(14rem,1fr))] gap-x-4 gap-y-2">
        <TechnicalFact label="Node ID" value={<code className="break-all font-mono">{node.nodeId}</code>} />
        <TechnicalFact label="Network key" value={<code className="break-all font-mono">{node.networkKey}</code>} />
        <TechnicalFact label="RPC state" value={nodeComponentStateLabel(node.rpcState)} />
        <TechnicalFact label="Sync state" value={nodeComponentStateLabel(node.syncState)} />
        <TechnicalFact label="Consensus state" value={nodeComponentStateLabel(node.consensusState)} />
        <TechnicalFact label="Process state" value={nodeComponentStateLabel(node.processState)} />
        <TechnicalFact label="Historical high watermark" value={formatNumber(node.historicalHighWatermark)} />
        <TechnicalFact label="Observed Network Head" value={formatNumber(node.networkReferenceHead)} />
        <TechnicalFact label="Reference confidence" value={node.networkReferenceConfidence || 'Unknown'} />
      </dl>
    </NodeDisclosure>
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

function SummaryTile({ label, value, detail }: { label: string; value: ReactNode; detail?: ReactNode }) {
  return (
    <CardX bordered={false} data-slot="node-summary-tile" className={cn('group min-w-0', CARD)} contentClassName="flex h-full flex-col gap-1">
      <span className="break-words text-xs font-medium tracking-wider text-muted-foreground">{label}</span>
      <strong className="min-w-0 break-words text-lg font-bold leading-none tracking-tight tabular-nums md:text-2xl">{value}</strong>
      {detail != null && detail !== '' && <small className="break-words text-[11px] text-muted-foreground">{detail}</small>}
    </CardX>
  )
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
        {note && <span className="text-[11px] font-normal text-muted-foreground">{note}</span>}
      </h2>
      <div className="grid grid-cols-1 gap-2">{children}</div>
    </CardX>
  )
}

function NodeDisclosure({ title, summaryDetail, children }: { title: string; summaryDetail?: string; children: ReactNode }) {
  return (
    <details data-slot="node-disclosure" className={cn('group mt-4 overflow-hidden', CARD)}>
      <summary className="flex min-h-11 cursor-pointer list-none flex-wrap items-baseline gap-x-3 gap-y-1 px-4 py-2.5 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring [&::-webkit-details-marker]:hidden">
        <ChevronRight data-slot="node-disclosure-marker" size={16} aria-hidden="true" className="text-muted-foreground transition-transform group-open:rotate-90" />
        <span className="font-semibold">{title}</span>
        {summaryDetail && <span className="text-[11px] text-muted-foreground">{summaryDetail}</span>}
      </summary>
      <div className="grid min-w-0 gap-3 px-4 pb-4">{children}</div>
    </details>
  )
}

function MetricCardHeading({ label, hint }: { label: string; hint?: string }) {
  return <header className="min-w-0"><h3 className="m-0 text-sm font-medium">{label}</h3>{hint && <span className="text-[11px] text-muted-foreground">{hint}</span>}</header>
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
}

function NodeMetricCard({ label, unit, value, detail, tone, series, showLegend = false, from, to, fixedMax, axisFormat, historyMessage, chartKind = 'line', windowSeconds, className = '' }: {
  label: string
  unit?: string
  value: string
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
  return (
    <CardX bordered={false} role="article" data-slot="node-metric-card" className={cn('min-w-0', CARD, className)} contentClassName="flex h-full min-w-0 flex-col gap-2">
      <div className="flex min-w-0 items-start justify-between gap-3">
        <MetricCardHeading label={label} hint={unit} />
        <strong data-slot="node-metric-value" className="min-w-0 break-words text-right text-lg font-bold leading-none tracking-tight tabular-nums">{value}</strong>
      </div>
      {detail && <p className="m-0 min-h-4 break-words text-[11px] text-muted-foreground">{detail}</p>}
      {showLegend && <MetricSeriesLegend label={label} series={series} toneClass={toneClass} />}
      {missingDirections.length > 0 && <p className="m-0 text-[11px] italic text-muted-foreground">{missingDirections.join(' and ')} unavailable in this window</p>}
      <MetricChart label={label} series={series} from={from} to={to} fixedMax={fixedMax} axisFormat={axisFormat} message={historyMessage} kind={chartKind} windowSeconds={windowSeconds} toneClass={toneClass} />
    </CardX>
  )
}

function MetricSeriesLegend({ label, series, toneClass }: { label: string; series: MetricSeries[]; toneClass: string }) {
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-3" aria-label={label + ' chart legend'}>
      {series.map((item) => (
        <span key={item.label} className="inline-flex min-w-0 items-center gap-1 text-[11px] text-muted-foreground">
          <i className={cn('inline-block size-2 shrink-0 rounded-full', item.secondary ? 'bg-cyan-500' : cn('bg-current', toneClass))} aria-hidden="true" />
          {item.label}
        </span>
      ))}
    </div>
  )
}

function MetricChart({ label, series, from, to, fixedMax, axisFormat, message, kind = 'line', windowSeconds, toneClass }: MetricChartProps) {
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

  return <div className="mt-auto grid min-w-0 grid-cols-[3rem_minmax(0,1fr)] grid-rows-[7.25rem_auto] gap-x-2">
    <div className="flex flex-col justify-between pr-1 text-right text-[11px] tabular-nums text-muted-foreground" aria-hidden="true">
      <span>{axisFormat(max)}</span>
      <span>{axisFormat(max / 2)}</span>
      <span>{axisFormat(0)}</span>
    </div>
    <svg viewBox="0 0 600 150" preserveAspectRatio="none" role="img" aria-label={label + ' ' + kind + ' chart ' + windowLabel} className={cn('col-start-2 row-start-1 h-[7.25rem] w-full overflow-visible', toneClass)}>
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

function formatConsensusValue(value: number | null | undefined, consensus: PublicNode['consensus'] | undefined): string {
  if (!consensus || value == null || consensus.freshness === 'unknown' || ['starting', 'disabled', 'unsupported'].includes(consensus.state)) return 'Unknown'
  return value.toLocaleString()
}

function formatValidatorMembership(node: PublicNode): string {
  const consensus = node.consensus
  if (!consensus || consensus.validator == null || consensus.freshness === 'unknown' || ['starting', 'disabled', 'unsupported'].includes(consensus.state)) return 'Unknown'
  return consensus.validator ? 'True' : 'False'
}

function formatRate(value: number | null | undefined): string {
  if (value == null) return 'Unknown'
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

/** The fixed public metrics response owns the window; the section label and
 *  every chart read the real from/to/windowSeconds instead of inventing a range. */
function describeMetricWindow(history: PublicNodeMetricHistory | undefined): { title: string; detail: string } {
  if (!history) {
    return { title: 'Latest 60 seconds', detail: 'Real retained samples from the fixed public metric-history window.' }
  }
  const seconds = Number.isFinite(history.windowSeconds) && history.windowSeconds > 0 ? history.windowSeconds : 60
  const from = formatUtcDateTime(history.from)
  const to = formatUtcDateTime(history.to)
  return {
    title: 'Latest ' + seconds + ' seconds',
    detail: 'Real retained samples from ' + from + ' to ' + to + ' UTC; missing intervals stay empty.',
  }
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
  return { value, detail: 'Block ' + latest.height.toLocaleString() + ' − ' + previous.height.toLocaleString() }
}

/** A process value is the retained current Public Projection value; when the
 *  process component is not Current it is explicitly marked as last-good. */
function processStatusDetail(state: string | null | undefined): string {
  const label = nodeComponentStateLabel(state)
  return label === 'Current'
    ? 'PlatON process · current observation'
    : 'PlatON process · last-good value retained; collection ' + label
}

/** Sync progress is only asserted from the Server Observed Network Head when
 *  its confidence is high; a low-confidence reference is shown as context, not
 *  as a progress claim. */
function formatSyncDetail(node: PublicNode): string {
  if (node.currentHead == null || node.networkReferenceHead == null) return 'Reference unavailable; sync progress is Unknown'
  if (node.networkReferenceConfidence !== 'high') {
    return 'Reference confidence ' + (node.networkReferenceConfidence || 'unknown') + '; progress is not asserted'
  }
  const behind = node.networkReferenceHead - node.currentHead
  if (behind <= 0) return 'At the Server Observed Network Head'
  return behind.toLocaleString() + ' blocks behind the Server Observed Network Head'
}

function formatResyncDetail(node: PublicNode): string {
  if (node.resyncProgress) return node.resyncProgress
  return node.resyncState === 'normal' ? 'No resync in progress' : 'Resync progress is Unknown'
}

function formatReferenceHead(node: PublicNode): string {
  return node.networkReferenceHead == null ? 'Unknown' : node.networkReferenceHead.toLocaleString()
}

function formatReferenceDetail(node: PublicNode): string {
  if (node.networkReferenceHead == null) return 'Server Observed Network Head unavailable'
  return 'Server Observed Network Head · ' + (node.networkReferenceConfidence || 'unknown') + ' confidence'
}

function nodeOverviewTone(state: string): 'ok' | 'warning' | 'error' | 'neutral' {
  switch (state) {
    case 'Current': return 'ok'
    case 'Starting': return 'warning'
    case 'Error': return 'error'
    case 'Unsupported': return 'warning'
    default: return 'neutral'
  }
}

function NodeOverviewStatus({ label, value }: { label: string; value: string }) {
  const state = nodeComponentStateLabel(value)
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <span className="text-xs font-medium tracking-wider text-muted-foreground">{label}</span>
      <StatusBadge status={state} tone={nodeOverviewTone(state)} />
    </div>
  )
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

type ComponentUpdateTime = { timestamp: string; date: Date }

const COMPONENT_TIMESTAMP_PATTERN = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(Z|[+-]\d{2}:\d{2})$/

/** PublicNode.freshness is the Server-computed earliest receipt timestamp. */
function parseComponentUpdateTime(value: string | null | undefined): ComponentUpdateTime | null {
  const timestamp = typeof value === 'string' ? value.trim() : undefined
  if (!timestamp) return null

  const match = COMPONENT_TIMESTAMP_PATTERN.exec(timestamp)
  if (!match) return null

  const year = Number(match[1])
  const month = Number(match[2])
  const day = Number(match[3])
  const hour = Number(match[4])
  const minute = Number(match[5])
  const second = Number(match[6])
  const timezone = match[7]
  const offsetHour = timezone === 'Z' ? 0 : Number(timezone.slice(1, 3))
  const offsetMinute = timezone === 'Z' ? 0 : Number(timezone.slice(4, 6))
  const daysInMonth = month === 2
    ? year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0) ? 29 : 28
    : [4, 6, 9, 11].includes(month) ? 30 : 31

  if (
    month < 1 || month > 12 || day < 1 || day > daysInMonth
    || hour > 23 || minute > 59 || second > 59
    || offsetHour > 23 || offsetMinute > 59
  ) return null

  const date = new Date(timestamp)
  return Number.isNaN(date.getTime()) ? null : { timestamp, date }
}

function NodeUpdateTime({ value }: { value: PublicNode['freshness'] }) {
  const update = parseComponentUpdateTime(value)
  if (!update) {
    return (
      <MetricRow
        label="Oldest component update"
        value="Unknown"
        detail="RPC, Sync, and Consensus receipt time is unavailable."
      />
    )
  }

  const relative = formatRelativeTime(update.date)
  const absolute = formatUtcDateTime(update.date)
  return (
    <MetricRow
      label="Oldest component update"
      value={<time dateTime={update.timestamp} title={absolute} aria-label={relative + '; ' + absolute}>{relative}</time>}
      detail={<><time dateTime={update.timestamp}>{absolute}</time><span className="block">Earliest Server receipt across RPC, Sync, and Consensus</span></>}
    />
  )
}

function formatNumber(value: number | null | undefined): string {
  return value == null ? 'Unknown' : value.toLocaleString()
}

function formatPercent(value: number | null | undefined): string {
  return value == null ? 'Unknown' : value.toFixed(1) + '%'
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

function NodeCard({ node }: { node: PublicNode }) {
  const health = nodeHealthPresentation(node.health)
  const showHealthReason = health.tone !== 'ok'
  const displayName = nodeDisplayName(node)
  const titleId = 'network-node-card-title-' + node.nodeId
  return (
    <CardX bordered={false} data-slot="node-card" data-tone={health.tone} className={cn('group/node-card min-w-0 transition-all duration-200', CARD, 'hover:-translate-y-0.5 hover:shadow-[0_0_20px,0_0_0_1px] hover:shadow-emerald-600/10')} contentClassName="flex h-full min-w-0 flex-col gap-3">
      <article className="flex min-w-0 flex-col gap-3" aria-labelledby={titleId}>
        <header className="flex min-w-0 items-start gap-2" role="group" aria-label="Node identity and health">
          <NodeHealthMarker health={node.health} />
          <h2 id={titleId} className="m-0 min-w-0 text-base font-semibold leading-snug">
            <Link className="inline-flex min-h-11 min-w-11 items-center break-words hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring" to={'/nodes/' + node.nodeId}>{displayName}</Link>
          </h2>
        </header>
        {showHealthReason && <p className="m-0 break-words text-[11px] text-warning-foreground dark:text-warning">{node.healthReason || 'Server health reason unavailable.'}</p>}
        <div className="grid grid-cols-1 gap-2 border-t border-dashed border-border/60 pt-2" role="group" aria-label="Node summary facts">
          <MetricRow label="Head" value={formatNumber(node.currentHead)} />
          <MetricRow label="Peers" value={peerCount(node.peers)} detail={peerBreakdown(node.peers)} />
          <NodeUpdateTime value={node.freshness} />
        </div>
        <div className="grid grid-cols-1 gap-2 min-[42rem]:grid-cols-3" role="group" aria-label="Node component status">
          <NodeOverviewStatus label="RPC" value={node.rpcState} />
          <NodeOverviewStatus label="Sync" value={node.syncState} />
          <NodeOverviewStatus label="Consensus" value={node.consensusState} />
        </div>
        <Link className="mt-auto inline-flex min-h-11 min-w-11 items-center justify-end gap-1 text-sm font-semibold text-emerald-600 hover:underline dark:text-emerald-400" to={'/nodes/' + node.nodeId}>View details <ArrowRight size={16} aria-hidden="true" /></Link>
      </article>
    </CardX>
  )
}
