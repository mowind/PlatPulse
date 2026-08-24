import { useMemo, useState } from 'react'
import { Link } from 'react-router'
import type { PublicNetwork, PublicNode } from '../api/generated'
import { realtimeStreamLabel } from './RealtimeNotice'
import { formatObservedAt, StatusBadge } from './StatusBadge'

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

export default function HomeDashboard({
  networks,
  realtimeStatus,
  online,
  resetting,
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
  const liveMessage = resetting
    ? 'Revalidating Home access…'
    : realtimeStreamLabel(realtimeStatus)

  return (
    <section className="page home-dashboard" aria-labelledby="home-dashboard-title">
      <header className="dashboard-heading">
        <div>
          <p className="dashboard-kicker">PLATPULSE / OVERVIEW</p>
          <h1 id="home-dashboard-title">Home</h1>
          <p className="dashboard-subtitle">Published Active Nodes from the Server-owned Public Projection.</p>
        </div>
        <p className={`dashboard-live dashboard-live-${toneFor(liveMessage)}`} role="status" aria-live="polite">
          <span aria-hidden="true" /> {liveMessage}
        </p>
        {!online && <p className="dashboard-live dashboard-live-warning" role="status" aria-live="polite"><span aria-hidden="true" /> You are offline</p>}
      </header>

      {error && <p className="dashboard-error" role="alert">{error}</p>}
      {loading && <p role="status">Starting Home…</p>}

      <div className="dashboard-summary-grid" aria-label="Home summary">
        <SummaryCard label="Published Nodes" value={hasProjection ? records.length : null} detail="Active Nodes on Home" tone="indigo" />
        <SummaryCard label="Healthy Nodes" value={healthyCount} detail="Server-owned health" tone="green" />
        <SummaryCard label="Attention" value={healthyCount === null ? null : records.length - healthyCount} detail="Unknown and degraded included" tone={healthyCount !== null && records.length === healthyCount ? 'green' : 'red'} />
        <SummaryCard label="Networks" value={hasProjection ? networks.length : null} detail="Published Network groups" tone="violet" />
      </div>

      <div className="dashboard-toolbar" aria-label="Node filters and sorting">
        <div className="dashboard-filter-pills" role="group" aria-label="Network filter">
          <button type="button" className={networkFilter === 'all' ? 'active' : ''} aria-pressed={networkFilter === 'all'} onClick={() => setNetworkFilter('all')}>All Networks</button>
          {networks.map((network) => (
            <button type="button" className={networkFilter === network.networkKey ? 'active' : ''} aria-pressed={networkFilter === network.networkKey} key={network.networkKey} onClick={() => setNetworkFilter(network.networkKey)}>
              {network.displayName}
            </button>
          ))}
        </div>
        <label className="dashboard-sort">Sort
          <select value={sortBy} onChange={(event) => setSortBy(event.target.value as SortKey)}>
            {sortOptions.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
          </select>
        </label>
      </div>

      {loading || (error && !hasLastGood)
        ? null
        : visibleRecords.length === 0
        ? <div className="dashboard-empty" role="status" aria-label="Empty: no published Nodes in this view."><strong>No published Nodes in this view.</strong><span>Private and retired Nodes are not listed on Home.</span></div>
        : <div className="dashboard-node-grid" aria-label="Active Nodes">{visibleRecords.map(({ network, node }) => <HomeNodeCard key={node.nodeId} network={network} node={node} />)}</div>}
    </section>
  )
}

function SummaryCard({ label, value, detail, tone }: { label: string; value: number | null; detail: string; tone: string }) {
  return <article className="dashboard-summary-card"><span className={`dashboard-summary-dot dashboard-summary-dot-${tone}`} aria-hidden="true" /><p>{label}</p><strong>{value === null ? '—' : value.toLocaleString()}</strong><small>{detail}</small></article>
}

function HomeNodeCard({ network, node }: NodeRecord) {
  const tone = toneFor(node.health)
  return (
    <article className={`dashboard-node-card dashboard-node-card-${tone}`}>
      <header className="dashboard-node-header">
        <div className="dashboard-node-title"><span className={`dashboard-node-status dashboard-node-status-${tone}`} aria-hidden="true" /><div><h2><Link to={`/nodes/${node.nodeId}`}>{nodeLabel(node)}</Link></h2><p><Link className="dashboard-card-network" to={`/networks/${network.networkKey}`}>{network.displayName}</Link></p></div></div>
        <StatusBadge status={healthLabel(node.health)} tone={statusTone(tone)} />
      </header>
      <p className="dashboard-health-reason">{node.healthReason}</p>
      <div className="dashboard-node-primary" aria-label="Node highlights">
        <Metric label="Current Head" value={formatNumber(node.currentHead)} />
        <Metric label="Peers" value={formatPeerCount(node)} detail={formatPeerObservation(node)} />
        <Metric label="Last Observed" value={freshnessLabel(node.freshness)} detail={lastObservedDetail(node.freshness)} />
      </div>
      <div className="dashboard-node-statuses" aria-label="Node component status">
        <ComponentStatus label="RPC" value={observationLabel(node.rpcState)} />
        <ComponentStatus label="Sync" value={observationLabel(node.syncState)} />
        <ComponentStatus label="Consensus" value={observationLabel(node.consensusState)} />
        <ComponentStatus label="Process" value={observationLabel(node.processState)} />
      </div>
      <footer className="dashboard-node-footer"><span>{resyncSummary(node)}</span><Link to={`/nodes/${node.nodeId}`}>View Node Details <span aria-hidden="true">↗</span></Link></footer>
    </article>
  )
}

function Metric({ label, value, detail }: { label: string; value: string; detail?: string }) {
  return <div className="dashboard-node-primary-metric"><span>{label}</span><strong>{value}</strong>{detail && <small>{detail}</small>}</div>
}

function ComponentStatus({ label, value }: { label: string; value: string }) {
  return <div className="dashboard-node-status-item"><span>{label}</span><StatusBadge status={value} tone={statusTone(toneFor(value))} /></div>
}

function resyncSummary(node: PublicNode): string {
  const state = observationLabel(node.resyncState)
  return state === 'Current' ? 'No active resync' : node.resyncProgress ?? `Resync ${state}`
}

function lastObservedDetail(value: string | null | undefined): string {
  if (!value || value === 'unknown' || value === 'current' || value === 'stale') return 'Server timestamp unavailable'
  return formatObservedAt(value)
}

function formatPeerCount(node: PublicNode) { return node.peers?.peerCount == null ? 'Unknown' : node.peers.peerCount.toLocaleString() }
function formatPeerObservation(node: PublicNode) {
  const peer = node.peers
  if (!peer) return 'Unknown observation'
  if (peer.state === 'error') return peer.peerCount == null ? 'Error; no last-good value' : 'Error; showing last-good value'
  if (peer.freshness === 'stale') return 'Stale; showing last-good value'
  if (peer.peerCount === 0) return 'Empty; authoritative zero'
  return 'Current observation'
}
function statusTone(tone: ReturnType<typeof toneFor>): 'ok' | 'warning' | 'error' | 'neutral' {
  return tone === 'good' ? 'ok' : tone === 'bad' ? 'error' : tone === 'warn' ? 'warning' : 'neutral'
}

function healthLabel(value: string): string {
  if (value === 'healthy') return 'Healthy'
  if (value === 'unhealthy') return 'Unhealthy'
  return 'Unknown'
}

function nodeLabel(node: PublicNode) { return node.displayName ?? node.nodeId }
function observationLabel(value: string | null | undefined): string {
  switch (value) {
    case 'ok':
    case 'connected':
    case 'synced':
    case 'ready':
    case 'running':
    case 'idle':
    case 'normal':
    case 'current':
    case 'fresh':
    case 'active':
      return 'Current'
    case 'error':
    case 'failed':
    case 'unhealthy':
    case 'offline':
      return 'Error'
    case 'stale':
      return 'Stale'
    case 'starting':
    case 'connecting':
      return 'Starting'
    case 'disabled':
      return 'Disabled'
    case 'unsupported':
      return 'Unsupported'
    case 'empty':
      return 'Empty'
    default:
      return 'Unknown'
  }
}
function freshnessLabel(value: string | null | undefined): string {
  return value === 'stale' ? 'Stale' : value && value !== 'unknown' ? 'Current' : 'Unknown'
}
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
