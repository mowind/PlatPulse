import { useId, useState } from 'react'
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
import type { PublicMetricPoint, PublicNode, PublicValidatorInsight } from '../api/generated'
import { useHomeRealtimeContext } from '../layouts/HomeLayout'
import { PeerInsight, peerInsightCollectionStatus, peerInsightFreshnessStatus, peerInsightValueStatus } from '../components/PeerInsight'
import { PeerHistoryInsight, normalizePublicPeerHistory } from '../components/PeerHistoryInsight'
import { GeoInsight } from '../components/GeoInsight'
import { ValidatorInsight } from '../components/ValidatorInsight'
import { ValidatorAnalytics } from '../components/ValidatorAnalytics'
import { formatRelativeTime, formatUtcDateTime, StatusBadge } from '../components/StatusBadge'
import { RealtimeNotice } from '../components/RealtimeNotice'
import { formatBytes } from '../formatBytes'

export function NetworkPage() {
  const { networkKey = '' } = useParams()
  const { generation, resetting, realtime } = useHomeRealtimeContext()
  const query = usePublicNetwork(networkKey, generation)

  if (resetting) return <section className="page public-network-page"><RealtimeNotice realtime={realtime} /><p role="status">Revalidating Home access…</p></section>
  if (query.isPending) return <section className="page public-network-page"><RealtimeNotice realtime={realtime} /><p role="status">Network is Starting; loading public data…</p></section>
  if (query.error && !query.data) return <section className="page public-network-page"><RealtimeNotice realtime={realtime} /><p role="alert" className="form-error">Network is Error; {query.error instanceof Error ? query.error.message : 'public Network data is unavailable.'}</p><Link to="/">Back to Home</Link></section>
  if (!query.data) return <section className="page public-network-page"><RealtimeNotice realtime={realtime} /><p role="status">Network is Unknown; public data is unavailable.</p><Link to="/">Back to Home</Link></section>

  const network = query.data
  return <section className="page public-network-page" aria-labelledby="network-page-title">
    <nav className="public-page-breadcrumb" aria-label="Breadcrumb">
      <Link to="/">← All Networks</Link>
      <span aria-hidden="true">/</span>
      <span>Network overview</span>
    </nav>
    <header className="public-page-heading">
      <div className="public-page-title">
        <h1 id="network-page-title">{network.displayName}</h1>
        <p className="public-page-subtitle">Active PlatON Nodes and network-level public observations.</p>
        <div className="public-page-meta" aria-label="Network identity and live updates">
          <span className="public-page-key">Network key <code>{network.networkKey}</code></span>
          <RealtimeNotice realtime={realtime} />
        </div>
      </div>
    </header>
    {query.isRefetchError && <p role="status" className="form-error">Network refresh failed; showing the last successful Network data.</p>}
    <div className={"public-network-insights" + (network.geo.state === 'disabled' ? " public-network-insights-geo-disabled" : "")}>
      <PeerInsight insight={network.peers} />
      <GeoInsight insight={network.geo} peerState={network.peers.state} />
    </div>
    {network.validators.length > 0 && <>
      <h2 className="public-section-heading">Validators</h2>
      <div className="node-grid">{network.validators.map((validator) => <ValidatorCard key={validator.validatorId} validator={validator} generation={generation} />)}</div>
    </>}
    <h2 className="public-section-heading">PlatON Nodes</h2>
    {network.nodes.length === 0
      ? <p className="public-empty-state" role="status">Empty: this Network has no Active Nodes.</p>
      : <div className="node-grid">{network.nodes.map((node) => <NodeCard node={node} key={node.nodeId} />)}</div>}
  </section>
}

function ValidatorCard({ validator, generation }: { validator: PublicValidatorInsight; generation: number }) {
  const history = usePublicValidatorHistory(validator.validatorId, 20, generation)
  const analytics = usePublicValidatorAnalytics(validator.validatorId, 31, generation)
  return <article className="node-card">
    <ValidatorInsight insight={validator} history={history.data?.entries} />
    {analytics.data && <ValidatorAnalytics analytics={analytics.data} compact />}
    {history.error && <p role="status" className="muted">Validator history unavailable.</p>}
  </article>
}

export function NodePage() {
  const { nodeId = '' } = useParams()
  const { generation, resetting, realtime } = useHomeRealtimeContext()
  const nodeQuery = usePublicNode(nodeId, generation)
  const historyQuery = usePublicNodeHistory(nodeId, generation)
  const metricsQuery = usePublicNodeMetrics(nodeId, generation)
  const peerHistoryQuery = usePublicNodePeerHistory(nodeId, generation)
  const [activeTab, setActiveTab] = useState<'details' | 'network'>('details')

  if (resetting) return <section className="page"><RealtimeNotice realtime={realtime} /><p role="status">Revalidating Node access…</p></section>
  if (nodeQuery.isPending) return <section className="page"><RealtimeNotice realtime={realtime} /><p role="status">Loading Node…</p></section>
  if (nodeQuery.error && !nodeQuery.data) return <section className="page"><RealtimeNotice realtime={realtime} /><p role="alert" className="form-error">{nodeQuery.error instanceof Error ? nodeQuery.error.message : 'Unable to load Node'}</p><Link to="/">Back to Home</Link></section>
  if (!nodeQuery.data) return <section className="page"><RealtimeNotice realtime={realtime} /><p role="status">Node unavailable.</p><Link to="/">Back to Home</Link></section>

  const node = nodeQuery.data
  const health = nodeHealthPresentation(node.health)
  const activity = nodeActivity(node)
  const blockInterval = latestBlockInterval(historyQuery.data)
  const metricHistory = metricsQuery.data
  const metricHistoryMessage = metricHistory
    ? undefined
    : metricsQuery.isPending
      ? 'Loading metric history…'
      : metricsQuery.error
        ? 'Metric history unavailable'
        : undefined

  return <section className="page node-detail-page">
    <div className="node-detail-topline">
      <div className="node-detail-breadcrumb">
        <Link to={`/networks/${node.networkKey}`}>← {node.networkKey}</Link>
        <span aria-hidden="true">/</span>
        <span>Node detail</span>
      </div>
      <RealtimeNotice realtime={realtime} />
    </div>
    {nodeQuery.isRefetchError && <p role="status" className="form-error">Node refresh failed; showing the last successful Node data.</p>}

    <section className="node-hero-card" aria-labelledby="node-detail-title">
      <header className="node-hero-header">
        <div>
          <h1 id="node-detail-title">{nodeDisplayName(node)}</h1>
          <p className="node-id-line">Node ID <code>{node.nodeId}</code></p>
        </div>
        <div className="node-hero-facts" aria-label="Node status">
          <div className="node-hero-fact"><span>Health</span><StatusBadge status={health.label} tone={health.tone} /></div>
          <div className="node-hero-fact"><span>Node status</span><StatusBadge status={activity.label} tone={activity.tone} /></div>
          <div className="node-hero-fact node-uptime"><span>Process uptime</span><strong>{formatDuration(node.processUptimeMs)}</strong></div>
        </div>
      </header>

      {health.tone !== 'ok' && <p className="node-hero-reason">{node.healthReason}</p>}
      <div className="node-hero-resources" aria-label="Node process and storage resources">
        <HeroResourceMetric label="CPU" value={formatPercent(node.processCpuPercent)} progress={node.processCpuPercent} />
        <HeroResourceMetric label="Memory" value={formatPercent(node.processMemoryPercent)} progress={node.processMemoryPercent} />
        <HeroResourceMetric
          label="Node data"
          value={formatBytes(node.nodeDataDirectorySizeBytes)}
          detail={diskDetail(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)}
          progress={diskProgress(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)}
        />
      </div>
      <div className="node-consensus-summary" aria-label="Node chain and consensus progress">
        <div className="node-height-metrics" aria-label="Node height metrics">
          <HeroMetric label="Head" value={formatNumber(node.currentHead)} />
          <HeroMetric label="QC" value={formatConsensusValue(node.consensus?.highestQcBlock, node.consensus)} />
          <HeroMetric label="Locked" value={formatConsensusValue(node.consensus?.highestLockBlock, node.consensus)} />
          <HeroMetric label="Committed" value={formatConsensusValue(node.consensus?.highestCommitBlock, node.consensus)} />
        </div>
        <div className="node-validator-role" aria-label="Validator role">
          <HeroMetric label="Validator" value={formatValidatorMembership(node)} />
        </div>
      </div>
      <footer className="node-hero-footer">
        <div><span>Started</span><strong>{formatUtcDateTime(node.processStartedAt)}</strong></div>
        <div><span>Last report</span><strong>{formatUtcDateTime(node.lastReportAt)}</strong></div>
      </footer>
    </section>

    <div className="node-tabs" role="tablist" aria-label="Node detail views">
      <TabButton id="node-details-tab" panelId="node-details-panel" selected={activeTab === 'details'} onSelect={() => setActiveTab('details')} onNavigate={(tab) => { setActiveTab(tab); document.getElementById(`node-${tab}-tab`)?.focus() }}>Details</TabButton>
      <TabButton id="node-network-tab" panelId="node-network-panel" selected={activeTab === 'network'} onSelect={() => setActiveTab('network')} onNavigate={(tab) => { setActiveTab(tab); document.getElementById(`node-${tab}-tab`)?.focus() }}>Network</TabButton>
    </div>

    <section id="node-details-panel" className="node-tabpanel" role="tabpanel" aria-labelledby="node-details-tab" aria-label="Details" hidden={activeTab !== 'details'}>
      <h2 className="sr-only">Details</h2>
      <div className="node-metric-grid">
        <article className="node-metric-card node-metric-network">
          <MetricCardHeading label="Network" hint="Host observation · live 1m" />
          <div className="node-network-rates">
            <div><span><i className="node-chart-key node-chart-key-primary" />↑ Upload</span><strong>{formatRate(node.hostNetworkTxBytesPerSec)}</strong></div>
            <div><span><i className="node-chart-key node-chart-key-secondary" />↓ Download</span><strong>{formatRate(node.hostNetworkRxBytesPerSec)}</strong></div>
          </div>
          <MetricChart
            label="Network"
            series={[
              { label: 'Upload', points: metricHistory?.networkTxBytesPerSec ?? [] },
              { label: 'Download', points: metricHistory?.networkRxBytesPerSec ?? [], secondary: true },
            ]}
            from={metricHistory?.from}
            to={metricHistory?.to}
            axisFormat={formatRate}
            message={metricHistoryMessage}
          />
        </article>
        <NodeMetricCard
          label="Connections"
          value={peerCount(node.peers)}
          detail={`${peerBreakdown(node.peers)} · live 1m`}
          tone="blue"
          series={[
            { label: 'Inbound', points: metricHistory?.peerInboundCount ?? [] },
            { label: 'Outbound', points: metricHistory?.peerOutboundCount ?? [], secondary: true },
          ]}
          showLegend
          from={metricHistory?.from}
          to={metricHistory?.to}
          axisFormat={formatCountAxis}
          historyMessage={metricHistoryMessage}
        />
        <NodeMetricCard
          label="Block time"
          value={blockInterval.value}
          detail={`${historyQuery.error ? 'History unavailable' : blockInterval.detail} · live 1m`}
          tone="amber"
          series={[{ label: 'Block time', points: metricHistory?.blockIntervalMs ?? [] }]}
          from={metricHistory?.from}
          to={metricHistory?.to}
          axisFormat={formatMillisecondsAxis}
          historyMessage={metricHistoryMessage}
          chartKind="bar"
        />
        <NodeMetricCard
          label="Transactions"
          value={formatNumber(node.latestBlockTransactionCount)}
          detail="Block Summary transaction count · live 1m"
          tone="violet"
          series={[{ label: 'Transactions', points: metricHistory?.transactionCount ?? [] }]}
          from={metricHistory?.from}
          to={metricHistory?.to}
          axisFormat={formatCountAxis}
          historyMessage={metricHistoryMessage}
          chartKind="bar"
        />
      </div>
    </section>

    <section id="node-network-panel" className="node-tabpanel" role="tabpanel" aria-labelledby="node-network-tab" aria-label="Network" hidden={activeTab !== 'network'}>
      <h2 className="sr-only">Network</h2>
      <PeerInsight insight={node.peers} />
      <PeerHistoryInsight
        history={peerHistoryQuery.data ? normalizePublicPeerHistory(peerHistoryQuery.data) : undefined}
        error={Boolean(peerHistoryQuery.error)}
        loading={peerHistoryQuery.isPending}
      />
      <p className="redaction-note">Network insight is public and redacted: peer addresses and identity lists are never displayed.</p>
    </section>
  </section>
}

function HeroMetric({ label, value }: { label: string; value: string }) {
  return <div><span>{label}</span><strong>{value}</strong></div>
}

function HeroResourceMetric({ label, value, detail, progress }: { label: string; value: string; detail?: string; progress?: number | null }) {
  const boundedProgress = progress == null ? null : Math.max(0, Math.min(100, progress))
  return <div className="node-hero-resource">
    <div><span>{label}</span><strong>{value}</strong></div>
    {boundedProgress != null && <i className="node-hero-resource-progress" aria-hidden="true"><span style={{ width: `${boundedProgress}%` }} /></i>}
    {detail && <small>{detail}</small>}
  </div>
}

function MetricCardHeading({ label, hint }: { label: string; hint?: string }) {
  return <header className="node-metric-card-heading"><h3>{label}</h3>{hint && <span>{hint}</span>}</header>
}

type MetricSeries = {
  label: string
  points: PublicMetricPoint[]
  secondary?: boolean
}

type MetricChartKind = 'line' | 'bar'

type MetricChartProps = {
  label: string
  series: MetricSeries[]
  from?: string
  to?: string
  fixedMax?: number
  axisFormat: (value: number) => string
  message?: string
  kind?: MetricChartKind
}

function NodeMetricCard({ label, value, detail, tone, series, showLegend = false, from, to, fixedMax, axisFormat, historyMessage, chartKind = 'line', className = '' }: {
  label: string
  value: string
  detail?: string
  tone: 'blue' | 'violet' | 'amber'
  series: MetricSeries[]
  showLegend?: boolean
  from?: string
  to?: string
  fixedMax?: number
  axisFormat: (value: number) => string
  historyMessage?: string
  chartKind?: MetricChartKind
  className?: string
}) {
  return <article className={`node-metric-card node-metric-${tone} ${className}`.trim()}>
    <div className="node-metric-card-summary">
      <MetricCardHeading label={label} />
      <strong className="node-metric-value">{value}</strong>
    </div>
    {detail && <p>{detail}</p>}
    {showLegend && <MetricSeriesLegend label={label} series={series} />}
    <MetricChart label={label} series={series} from={from} to={to} fixedMax={fixedMax} axisFormat={axisFormat} message={historyMessage} kind={chartKind} />
  </article>
}

function MetricSeriesLegend({ label, series }: { label: string; series: MetricSeries[] }) {
  return <div className="node-metric-legend" aria-label={`${label} chart legend`}>
    {series.map((item) => <span key={item.label}><i className={item.secondary ? 'node-chart-key node-chart-key-secondary' : 'node-chart-key'} aria-hidden="true" />{item.label}</span>)}
  </div>
}

function MetricChart({ label, series, from, to, fixedMax, axisFormat, message, kind = 'line' }: MetricChartProps) {
  const gradientId = `node-metric-fill-${useId().replaceAll(':', '')}`
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

  return <div className="node-metric-chart">
    <div className="node-metric-y-axis" aria-hidden="true">
      <span>{axisFormat(max)}</span>
      <span>{axisFormat(max / 2)}</span>
      <span>{axisFormat(0)}</span>
    </div>
    <svg viewBox="0 0 600 150" preserveAspectRatio="none" role="img" aria-label={`${label} ${kind} chart over the last minute`}>
      <title>{label} {kind} chart over the last minute</title>
      <desc>{chartMessage ? `${label}: ${chartMessage}` : `${series.map((item) => item.label).join(' and ')} values from one minute ago to now`}</desc>
      <defs>
        <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="var(--node-metric-accent)" stopOpacity="0.34" />
          <stop offset="100%" stopColor="var(--node-metric-accent)" stopOpacity="0.02" />
        </linearGradient>
      </defs>
      <g className="node-metric-grid-lines" aria-hidden="true">
        <line x1="0" y1="8" x2="600" y2="8" />
        <line x1="0" y1="75" x2="600" y2="75" />
        <line x1="0" y1="142" x2="600" y2="142" />
      </g>
      {!chartMessage && kind === 'line' && plots.map((item, index) => {
        const line = chartLinePath(item.coordinates)
        const area = plots.length === 1 ? chartAreaPath(item.coordinates) : ''
        return <g key={item.label}>
          {area && <path className="node-metric-chart-area" d={area} fill={`url(#${gradientId})`} />}
          {line && <path className={item.secondary ? 'node-metric-chart-line node-metric-chart-line-secondary' : 'node-metric-chart-line'} d={line} />}
          {item.coordinates.length === 1 && <circle className={item.secondary ? 'node-metric-chart-dot node-metric-chart-dot-secondary' : 'node-metric-chart-dot'} cx={item.coordinates[0].x} cy={item.coordinates[0].y} r={index === 0 ? 4 : 3.5} />}
        </g>
      })}
      {!chartMessage && kind === 'bar' && plots.flatMap((item) => {
        const width = chartBarWidth(item.coordinates.length)
        return item.coordinates.map((point, index) => {
          const height = Math.max(1, 142 - point.y)
          const x = Math.max(0, Math.min(600 - width, point.x - width / 2))
          return <rect key={`${item.label}-${index}`} className="node-metric-chart-bar" x={x} y={142 - height} width={width} height={height} rx={Math.min(2.5, width / 3)} />
        })
      })}
      {chartMessage && <text className="node-metric-chart-empty" x="300" y="78" textAnchor="middle">{chartMessage}</text>}
    </svg>
    <div className="node-metric-x-axis" aria-hidden="true"><span>1m</span><span>0s</span></div>
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
  return points.map((point, index) => `${index === 0 ? 'M' : 'L'} ${point.x.toFixed(2)} ${point.y.toFixed(2)}`).join(' ')
}

function chartAreaPath(points: ChartCoordinate[]): string {
  if (points.length < 2) return ''
  const first = points[0]
  const last = points.at(-1)
  if (!last) return ''
  return `${chartLinePath(points)} L ${last.x.toFixed(2)} 142 L ${first.x.toFixed(2)} 142 Z`
}

function niceChartMax(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 1
  const magnitude = 10 ** Math.floor(Math.log10(value))
  const normalized = value / magnitude
  const step = normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10
  return step * magnitude
}

function nodeActivity(node: PublicNode): { label: string; tone: 'ok' | 'warning' | 'error' | 'neutral' } {
  const value = node.validator?.activity
  if (!value || value === 'unknown' || value === 'observing') return { label: 'Observing', tone: 'neutral' }
  const label = value.charAt(0).toUpperCase() + value.slice(1)
  if (node.validator?.activityState === 'stale') return { label: `${label} (Stale)`, tone: 'warning' }
  return { label, tone: ['exiting', 'exited', 'verifying', 'locked'].includes(value) ? 'warning' : 'ok' }
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

function formatDuration(value: number | null | undefined): string {
  if (value == null || value < 0) return 'Unknown'
  const totalSeconds = Math.floor(value / 1000)
  const days = Math.floor(totalSeconds / 86_400)
  const hours = Math.floor((totalSeconds % 86_400) / 3_600)
  const minutes = Math.floor((totalSeconds % 3_600) / 60)
  if (days > 0) return `${days}d ${hours}h`
  if (hours > 0) return `${hours}h ${minutes}m`
  if (minutes > 0) return `${minutes}m`
  return `${totalSeconds}s`
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
  return `${scaled.toFixed(digits)} ${units[unit]}`
}

function formatCountAxis(value: number): string {
  return Math.round(value).toLocaleString()
}

function formatMillisecondsAxis(value: number): string {
  if (value >= 1000) return `${Number((value / 1000).toFixed(value >= 10_000 ? 0 : 1))}s`
  return `${Math.round(value)}ms`
}

function diskProgress(size: number | null | undefined, capacity: number | null | undefined): number | null {
  if (size == null || capacity == null || capacity <= 0) return null
  return (size / capacity) * 100
}

function diskDetail(size: number | null | undefined, capacity: number | null | undefined): string {
  const progress = diskProgress(size, capacity)
  if (progress == null) return 'Node data directory'
  return `${formatBytes(capacity)} total · ${formatPercent(progress)}`
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
  const value = elapsed < 1000 ? `${elapsed} ms` : `${(elapsed / 1000).toFixed(2)} s`
  return { value, detail: `Block ${latest.height.toLocaleString()} − ${previous.height.toLocaleString()}` }
}

function TabButton({ id, panelId, selected, onSelect, onNavigate, children }: { id: string; panelId: string; selected: boolean; onSelect: () => void; onNavigate: (tab: 'details' | 'network') => void; children: string }) {
  return <button
    id={id}
    className="node-tab"
    type="button"
    role="tab"
    aria-controls={panelId}
    aria-selected={selected}
    tabIndex={selected ? 0 : -1}
    onClick={onSelect}
    onKeyDown={(event) => {
      if (event.key === 'Enter' || event.key === ' ') {
        event.preventDefault()
        onSelect()
      } else if (event.key === 'ArrowRight' || event.key === 'ArrowDown' || event.key === 'ArrowLeft' || event.key === 'ArrowUp') {
        event.preventDefault()
        onNavigate(id === 'node-details-tab' ? 'network' : 'details')
      }
    }}
  >{children}</button>
}

function NodeOverviewStatus({ label, value }: { label: string; value: string }) {
  const state = nodeComponentStateLabel(value)
  return (
    <div className="node-overview-status">
      <span>{label}</span>
      <span className={`node-overview-state node-overview-state-${state.toLowerCase()}`}>
        <span className="node-overview-state-dot" aria-hidden="true" />
        {state}
      </span>
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
      <div className="network-node-time">
        <span>Oldest component update</span>
        <strong>Unknown</strong>
        <small>RPC, Sync, and Consensus receipt time is unavailable.</small>
      </div>
    )
  }

  const relative = formatRelativeTime(update.date)
  const absolute = formatUtcDateTime(update.date)
  return (
    <div className="network-node-time">
      <span>Oldest component update</span>
      <strong><time dateTime={update.timestamp} title={absolute} aria-label={relative + '; ' + absolute}>{relative}</time></strong>
      <small><time dateTime={update.timestamp}>{absolute}</time></small>
      <small className="network-node-time-note">Earliest Server receipt across RPC, Sync, and Consensus</small>
    </div>
  )
}

function formatNumber(value: number | null | undefined): string {
  return value == null ? 'Unknown' : value.toLocaleString()
}

function formatPercent(value: number | null | undefined): string {
  return value == null ? 'Unknown' : `${value.toFixed(1)}%`
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
  return <article className="node-card network-node-card" aria-labelledby={titleId}>
    <header className="network-node-card-header" role="group" aria-label="Node identity and health">
      <div>
        <h2 id={titleId}><Link to={'/nodes/' + node.nodeId}>{displayName}</Link></h2>
      </div>
      <StatusBadge status={health.label} tone={health.tone} />
    </header>
    {showHealthReason && <p className="health-reason">{node.healthReason || 'Server health reason unavailable.'}</p>}
    <div className="network-node-highlights" role="group" aria-label="Node summary facts">
      <div><span>Head</span><strong>{formatNumber(node.currentHead)}</strong></div>
      <div><span>Peers</span><strong>{peerCount(node.peers)}</strong><small>{peerBreakdown(node.peers)}</small></div>
      <NodeUpdateTime value={node.freshness} />
    </div>
    <div className="network-node-statuses" role="group" aria-label="Node component status">
      <NodeOverviewStatus label="RPC" value={node.rpcState} />
      <NodeOverviewStatus label="Sync" value={node.syncState} />
      <NodeOverviewStatus label="Consensus" value={node.consensusState} />
    </div>
    <Link className="network-node-detail-link" to={'/nodes/' + node.nodeId}>View details <span aria-hidden="true">→</span></Link>
  </article>
}
