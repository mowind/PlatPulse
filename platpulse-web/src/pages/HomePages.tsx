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
import { PeerInsight } from '../components/PeerInsight'
import { PeerHistoryInsight, normalizePublicPeerHistory } from '../components/PeerHistoryInsight'
import { GeoInsight } from '../components/GeoInsight'
import { ValidatorInsight } from '../components/ValidatorInsight'
import { ValidatorAnalytics } from '../components/ValidatorAnalytics'
import { formatObservedAt, StatusBadge } from '../components/StatusBadge'
import { RealtimeNotice } from '../components/RealtimeNotice'
import { formatBytes } from '../formatBytes'

export function NetworkPage() {
  const { networkKey = '' } = useParams()
  const { generation, resetting, realtime } = useHomeRealtimeContext()
  const query = usePublicNetwork(networkKey, generation)

  if (resetting) return <section className="page public-network-page"><p role="status">Revalidating Home access…</p></section>
  if (query.isPending) return <section className="page public-network-page"><p role="status">Network is Starting; loading public data…</p></section>
  if (query.error && !query.data) return <section className="page public-network-page"><p role="alert" className="form-error">Network is Error; {query.error instanceof Error ? query.error.message : 'public Network data is unavailable.'}</p><Link to="/">Back to Home</Link></section>
  if (!query.data) return <section className="page public-network-page"><p role="status">Network is Unknown; public data is unavailable.</p><Link to="/">Back to Home</Link></section>

  const network = query.data
  return <section className="page public-network-page" aria-labelledby="network-page-title">
    <header className="public-page-heading">
      <div>
        <p className="dashboard-kicker">PLATPULSE / NETWORK OVERVIEW</p>
        <h1 id="network-page-title">{network.displayName}</h1>
        <p className="public-page-subtitle">Active PlatON Nodes and network-level public observations.</p>
      </div>
      <RealtimeNotice realtime={realtime} />
    </header>
    {query.isRefetchError && <p role="status" className="form-error">Network refresh failed; showing the last successful Network data.</p>}
    <div className="public-network-context">
      <Link to="/">← All Networks</Link>
      <span>Network key <code>{network.networkKey}</code></span>
    </div>
    <div className="public-network-insights">
      <PeerInsight insight={network.peers} />
      <GeoInsight insight={network.geo} />
    </div>
    {network.validators.length > 0 && <>
      <h2 className="public-section-heading">Validators</h2>
      <div className="node-grid">{network.validators.map((validator) => <ValidatorCard key={validator.validatorId} validator={validator} generation={generation} />)}</div>
    </>}
    <h2 className="public-section-heading">PlatON Nodes</h2>
    {network.nodes.length === 0
      ? <p className="public-empty-state" role="status">Empty: no Active Nodes are published for this Network.</p>
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

  if (resetting) return <section className="page"><p role="status">Revalidating Node access…</p></section>
  if (nodeQuery.isPending) return <section className="page"><p role="status">Loading Node…</p></section>
  if (nodeQuery.error && !nodeQuery.data) return <section className="page"><p role="alert" className="form-error">{nodeQuery.error instanceof Error ? nodeQuery.error.message : 'Unable to load Node'}</p><Link to="/">Back to Home</Link></section>
  if (!nodeQuery.data) return <section className="page"><p role="status">Node unavailable.</p><Link to="/">Back to Home</Link></section>

  const node = nodeQuery.data
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
          <p className="dashboard-kicker">PLATPULSE / {node.networkKey}</p>
          <h1 id="node-detail-title">{node.displayName ?? 'Node detail'}</h1>
          <p className="node-id-line">Node ID <code>{node.nodeId}</code></p>
        </div>
        <div className="node-hero-badges" aria-label="Node status">
          <div><span>Health</span><StatusBadge status={nodeHealthLabel(node.health)} tone={nodeHealthTone(node.health)} /></div>
          <div><span>Node status</span><StatusBadge status={activity.label} tone={activity.tone} /></div>
          <div className="node-uptime"><span>Process uptime</span><strong>{formatDuration(node.processUptimeMs)}</strong></div>
        </div>
      </header>

      {nodeHealthTone(node.health) !== 'ok' && <p className="node-hero-reason">{node.healthReason}</p>}
      <div className="node-hero-resources" aria-label="Node process and storage resources">
        <HeroResourceMetric label="CPU" value={formatPercent(node.processCpuPercent)} progress={node.processCpuPercent} />
        <HeroResourceMetric label="MEMORY" value={formatPercent(node.processMemoryPercent)} progress={node.processMemoryPercent} />
        <HeroResourceMetric
          label="NODE DATA"
          value={formatBytes(node.nodeDataDirectorySizeBytes)}
          detail={diskDetail(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)}
          progress={diskProgress(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)}
        />
      </div>
      <div className="node-consensus-runway" aria-label="Node chain and consensus progress">
        <HeroMetric label="HEAD" value={formatNumber(node.currentHead)} />
        <HeroMetric label="QC" value={formatConsensusValue(node.consensus?.highestQcBlock, node.consensus)} />
        <HeroMetric label="LOCKED" value={formatConsensusValue(node.consensus?.highestLockBlock, node.consensus)} />
        <HeroMetric label="COMMITTED" value={formatConsensusValue(node.consensus?.highestCommitBlock, node.consensus)} />
        <HeroMetric label="VALIDATOR" value={formatValidatorMembership(node)} />
      </div>
      <footer className="node-hero-footer">
        <div><span>Started</span><strong>{formatDateTime(node.processStartedAt)}</strong></div>
        <div><span>Last report</span><strong>{formatDateTime(node.lastReportAt)}</strong></div>
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

function NodeMetricCard({ label, value, detail, tone, series, from, to, fixedMax, axisFormat, historyMessage, chartKind = 'line', className = '' }: {
  label: string
  value: string
  detail?: string
  tone: 'blue' | 'violet' | 'amber'
  series: MetricSeries[]
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
    <MetricChart label={label} series={series} from={from} to={to} fixedMax={fixedMax} axisFormat={axisFormat} message={historyMessage} kind={chartKind} />
  </article>
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

function formatDateTime(value: string | null | undefined): string {
  if (!value) return 'Unknown'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return 'Unknown'
  return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'medium' }).format(date)
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
  return <div className="node-overview-status"><span>{label}</span><StatusBadge status={value} tone={stateTone(value)} /></div>
}

function nodeComponentStateLabel(value: string | null | undefined): string {
  switch (value) {
    case 'ok':
    case 'healthy':
    case 'normal':
    case 'current': return 'Current'
    case 'stale': return 'Stale'
    case 'error':
    case 'unhealthy': return 'Error'
    case 'stopped':
      return 'Error'
    case 'resyncing':
      return 'Starting'
    case 'disabled': return 'Disabled'
    case 'unsupported': return 'Unsupported'
    case 'empty': return 'Empty'
    case 'Current': return 'Current'
    case 'Error': return 'Error'
    case 'Stale': return 'Stale'
    case 'Unsupported': return 'Unsupported'
    case 'Stopped': return 'Error'
    case 'Resyncing': return 'Starting'
    default: return 'Unknown'
  }
}

function nodeHealthLabel(value: string): string {
  if (value === 'healthy') return 'Healthy'
  if (value === 'unhealthy') return 'Unhealthy'
  return 'Unknown'
}

function nodeHealthTone(value: string): 'ok' | 'warning' | 'error' | 'neutral' {
  if (value === 'healthy') return 'ok'
  if (value === 'unhealthy') return 'error'
  return 'neutral'
}

function stateTone(value: string | null | undefined): 'ok' | 'warning' | 'error' | 'neutral' {
  const label = nodeComponentStateLabel(value)
  if (label === 'Current') return 'ok'
  if (label === 'Error') return 'error'
  if (label === 'Stale' || label === 'Starting' || label === 'Unsupported') return 'warning'
  return 'neutral'
}

function freshnessLabel(value: string | null | undefined): string {
  if (value === 'current') return 'Current'
  if (value === 'stale') return 'Stale'
  if (value && value !== 'unknown') return 'Current'
  return 'Unknown'
}

function freshnessDetail(value: string | null | undefined): string {
  if (!value || value === 'unknown') return 'Freshness Unknown; no Server timestamp is available.'
  if (value === 'current' || value === 'stale') return `Freshness ${freshnessLabel(value)}; Server-provided state.`
  return `Last observed ${formatObservedAt(value)}.`
}

function formatNumber(value: number | null | undefined): string {
  return value == null ? 'Unknown' : value.toLocaleString()
}

function formatPercent(value: number | null | undefined): string {
  return value == null ? 'Unknown' : `${value.toFixed(1)}%`
}

function peerCount(insight: PublicNode['peers']): string {
  if (insight.peerCount == null) return 'Unknown'
  if (insight.peerCount === 0 && ['starting', 'disabled', 'unsupported'].includes(insight.state)) return 'Unknown'
  return insight.peerCount.toLocaleString()
}

function peerBreakdown(insight: PublicNode['peers']): string {
  if (insight.state === 'error') return insight.peerCount == null ? 'Peer observation unavailable' : 'Error; showing last-good value'
  if (insight.freshness === 'stale') return 'Stale; showing last-good value'
  if (insight.peerCount === 0) return 'Empty; authoritative successful zero'
  if (insight.inboundCount == null || insight.outboundCount == null) return 'Current observation'
  return `${insight.inboundCount} inbound · ${insight.outboundCount} outbound`
}

function NodeCard({ node }: { node: PublicNode }) {
  return <article className="node-card network-node-card">
    <header className="network-node-card-header">
      <div>
        <p className="node-card-eyebrow">PlatON Node</p>
        <h2><Link to={`/nodes/${node.nodeId}`}>{node.displayName ?? node.nodeId}</Link></h2>
      </div>
      <StatusBadge status={nodeHealthLabel(node.health)} tone={nodeHealthTone(node.health)} />
    </header>
    <p className="health-reason">{node.healthReason}</p>
    <div className="network-node-highlights">
      <div><span>Current Head</span><strong>{formatNumber(node.currentHead)}</strong></div>
      <div><span>Peers</span><strong>{peerCount(node.peers)}</strong><small>{peerBreakdown(node.peers)}</small></div>
      <div><span>Last observed</span><strong>{freshnessLabel(node.freshness)}</strong><small>{freshnessDetail(node.freshness)}</small></div>
    </div>
    <div className="network-node-statuses" aria-label="Node component status">
      <NodeOverviewStatus label="RPC" value={nodeComponentStateLabel(node.rpcState)} />
      <NodeOverviewStatus label="Sync" value={nodeComponentStateLabel(node.syncState)} />
      <NodeOverviewStatus label="Consensus" value={nodeComponentStateLabel(node.consensusState)} />
    </div>
    <Link className="network-node-detail-link" to={`/nodes/${node.nodeId}`}>View Node Details <span aria-hidden="true">↗</span></Link>
  </article>
}
