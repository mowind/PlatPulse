import { useEffect, useMemo, useState } from 'react'
import { Link } from 'react-router'
import type { PublicNetwork, PublicNode } from '../api/generated'
import { ServerStatusNotice } from './ServerStatusNotice'

type HomeDashboardProps = {
  networks: PublicNetwork[]
  realtimeStatus: string
  error: string | null
}

type NodeRecord = {
  network: PublicNetwork
  node: PublicNode
}

const SORT_OPTIONS = [
  { value: 'health', label: 'Health' },
  { value: 'name', label: 'Name' },
  { value: 'head', label: 'Current head' },
] as const

/** Home dashboard inspired by the supplied reference, using PlatPulse data only. */
export default function HomeDashboard({ networks, realtimeStatus, error }: HomeDashboardProps) {
  const [selectedNetwork, setSelectedNetwork] = useState('all')
  const [sortBy, setSortBy] = useState<(typeof SORT_OPTIONS)[number]['value']>('health')
  const [now, setNow] = useState(() => new Date())

  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), 1000)
    return () => window.clearInterval(timer)
  }, [])

  const nodeRecords = useMemo<NodeRecord[]>(
    () => networks.flatMap((network) => network.nodes.map((node) => ({ network, node }))),
    [networks],
  )
  const filteredNodes = useMemo(() => {
    const visible = selectedNetwork === 'all'
      ? nodeRecords
      : nodeRecords.filter(({ network }) => network.networkKey === selectedNetwork)
    return [...visible].sort((left, right) => {
      if (sortBy === 'name') return (left.node.displayName ?? left.node.nodeId).localeCompare(right.node.displayName ?? right.node.nodeId)
      if (sortBy === 'head') return (right.node.currentHead ?? -1) - (left.node.currentHead ?? -1)
      return healthRank(left.node.health) - healthRank(right.node.health)
    })
  }, [nodeRecords, selectedNetwork, sortBy])

  const healthy = nodeRecords.filter(({ node }) => statusTone(node.health) === 'good').length
  const attention = nodeRecords.length - healthy
  const liveLabel = realtimeStatus === 'connected' ? 'Live' : realtimeStatus === 'connecting' ? 'Connecting' : 'Paused'

  return <section className="page home-dashboard" aria-labelledby="home-dashboard-title">
    <ServerStatusNotice />
    <header className="dashboard-heading">
      <div>
        <p className="dashboard-kicker">PLATPULSE / OVERVIEW</p>
        <h1 id="home-dashboard-title">Home</h1>
        <p className="dashboard-clock">Current time <strong>{formatTime(now)}</strong></p>
      </div>
      <div className={'dashboard-live dashboard-live-' + statusTone(liveLabel)} role="status">
        <span aria-hidden="true" /> {liveLabel}
      </div>
    </header>

    {error && <p role="status" aria-live="polite" className="dashboard-error">{error}</p>}

    <div className="dashboard-summary-grid" aria-label="Home summary">
      <SummaryCard label="Published Nodes" value={nodeRecords.length} tone="indigo" detail="Visible on Home" />
      <SummaryCard label="Healthy Nodes" value={healthy} tone="green" detail="Server-owned health" />
      <SummaryCard label="Attention" value={attention} tone={attention > 0 ? 'red' : 'green'} detail={attention > 0 ? 'Review Node detail' : 'No unhealthy Nodes'} />
      <SummaryCard label="Networks" value={networks.length} tone="violet" detail="Published Network groups" />
    </div>

    <div className="dashboard-toolbar" aria-label="Node filters and sorting">
      <div className="dashboard-view-switcher" aria-label="Dashboard view">
        <button className="dashboard-view-button dashboard-view-button-active" type="button" aria-label="Card view" aria-pressed="true">▦</button>
        <button className="dashboard-view-button" type="button" aria-label="List view" aria-pressed="false" disabled>☷</button>
        <button className="dashboard-view-button" type="button" aria-label="Map view" aria-pressed="false" disabled>⌖</button>
      </div>
      <div className="dashboard-filter-pills" role="group" aria-label="Network filter">
        <button className={selectedNetwork === 'all' ? 'active' : ''} type="button" onClick={() => setSelectedNetwork('all')}>All</button>
        {networks.map((network) => <button className={selectedNetwork === network.networkKey ? 'active' : ''} type="button" key={network.networkKey} onClick={() => setSelectedNetwork(network.networkKey)}>{network.displayName}</button>)}
      </div>
      <label className="dashboard-sort">Sort
        <select value={sortBy} onChange={(event) => setSortBy(event.target.value as (typeof SORT_OPTIONS)[number]['value'])}>
          {SORT_OPTIONS.map((option) => <option value={option.value} key={option.value}>{option.label}</option>)}
        </select>
      </label>
    </div>

    {filteredNodes.length === 0
      ? <div className="dashboard-empty"><strong>No published Nodes in this view.</strong><span>Private and retired Nodes are not listed on Home.</span></div>
      : <div className="dashboard-node-grid">{filteredNodes.map(({ network, node }) => <HomeNodeCard key={node.nodeId} network={network} node={node} />)}</div>}
  </section>
}

function SummaryCard({ label, value, tone, detail }: { label: string; value: number; tone: string; detail: string }) {
  return <article className="dashboard-summary-card">
    <div className={'dashboard-summary-dot dashboard-summary-dot-' + tone} aria-hidden="true" />
    <p>{label}</p>
    <strong>{value.toLocaleString()}</strong>
    <small>{detail}</small>
  </article>
}

function HomeNodeCard({ network, node }: NodeRecord) {
  const tone = statusTone(node.health)
  const metrics = [
    { label: 'RPC', value: node.rpcState ?? 'Unknown' },
    { label: 'Sync', value: node.syncState ?? 'Unknown' },
    { label: 'Consensus', value: node.consensusState ?? 'Unknown' },
    { label: 'Host CPU', value: node.hostCpuPercent == null ? 'Unknown' : node.hostCpuPercent.toFixed(1) + '%' },
  ]
  return <article className={'dashboard-node-card dashboard-node-card-' + tone}>
    <header className="dashboard-node-header">
      <div className="dashboard-node-title"><span className={'dashboard-node-status dashboard-node-status-' + tone} aria-hidden="true" /><div><h2><Link to={'/nodes/' + node.nodeId}>{node.displayName ?? node.nodeId}</Link></h2><p><Link className="dashboard-card-network" to={'/networks/' + network.networkKey}>{network.displayName}</Link> <span aria-hidden="true">·</span> {node.nodeId}</p></div></div>
      <span className={'dashboard-health-label dashboard-health-label-' + tone}>{node.health}</span>
    </header>
    <div className="dashboard-node-rule" />
    <dl className="dashboard-node-metrics">{metrics.map((metric) => <div key={metric.label}><dt>{metric.label}</dt><dd>{metric.value}</dd><span className={'dashboard-metric-bar dashboard-metric-bar-' + statusTone(metric.value)}><i /></span></div>)}</dl>
    <div className="dashboard-observation-row"><Observation label="Current head" value={formatNumber(node.currentHead)} /><Observation label="History" value={formatNumber(node.historicalHighWatermark)} /><Observation label="Peer count" value={node.peers?.peerCount == null ? 'Unknown' : String(node.peers.peerCount)} /><Observation label="Freshness" value={formatFreshness(node.freshness)} /></div>
    <div className="dashboard-quality" aria-label="Node observation quality"><span className="dashboard-quality-label">Observation quality</span><StatusSegments node={node} /></div>
    <footer className="dashboard-node-footer"><span>{node.resyncState}</span><span>{node.networkReferenceConfidence ?? 'Reference unknown'}</span><Link to={'/nodes/' + node.nodeId}>Open detail <span aria-hidden="true">↗</span></Link></footer>
  </article>
}

function Observation({ label, value }: { label: string; value: string }) {
  return <div><dt>{label}</dt><dd>{value}</dd></div>
}

function StatusSegments({ node }: { node: PublicNode }) {
  const values = [node.health ?? 'unknown', node.rpcState ?? 'unknown', node.syncState ?? 'unknown', node.consensusState ?? 'unknown', node.processState ?? 'unknown', node.peers?.state ?? 'unknown']
  return <div className="dashboard-quality-segments">{Array.from({ length: 12 }, (_, index) => <i key={index} className={'dashboard-quality-segment dashboard-quality-segment-' + statusTone(values[index % values.length] ?? 'unknown')} />)}</div>
}

function statusTone(value: string): 'good' | 'warn' | 'bad' | 'neutral' {
  const normalized = value.toLowerCase()
  if (/(healthy|current|connected|ready|synced|active|running|ok|fresh|live)/.test(normalized)) return 'good'
  if (/(stale|starting|unknown|unsupported|disabled|empty|resync|degraded|paused|attention)/.test(normalized)) return 'warn'
  if (/(error|failed|unhealthy|offline|retired|unavailable)/.test(normalized)) return 'bad'
  return 'neutral'
}

function healthRank(value: string) {
  const tone = statusTone(value)
  return tone === 'bad' ? 0 : tone === 'warn' ? 1 : tone === 'good' ? 2 : 3
}

function formatNumber(value: number | null | undefined) {
  return value == null ? 'Unknown' : value.toLocaleString()
}

function formatFreshness(value: string | null | undefined) {
  if (!value) return 'Unknown'
  const date = new Date(value)
  if (Number.isNaN(date.valueOf())) return value
  const minutes = Math.max(0, Math.round((Date.now() - date.valueOf()) / 60000))
  return minutes < 1 ? 'Just now' : minutes + 'm ago'
}

function formatTime(value: Date) {
  return value.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
}
