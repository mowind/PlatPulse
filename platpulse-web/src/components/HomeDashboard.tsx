import { useMemo, useState } from 'react'
import { Link } from 'react-router'
import type { PublicNetwork, PublicNode } from '../api/generated'

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
    : !online
      ? 'You are offline'
        : realtimeStatus === 'connected'
        ? 'Current'
        : realtimeStatus === 'connecting'
          ? 'Starting'
          : 'Live updates paused'

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
        ? <div className="dashboard-empty" role="status"><strong>No published Nodes in this view.</strong><span>Private and retired Nodes are not listed on Home.</span></div>
        : <div className="dashboard-node-grid" aria-label="Active Nodes">{visibleRecords.map(({ network, node }) => <HomeNodeCard key={node.nodeId} network={network} node={node} />)}</div>}
    </section>
  )
}

function SummaryCard({ label, value, detail, tone }: { label: string; value: number | null; detail: string; tone: string }) {
  return <article className="dashboard-summary-card"><span className={`dashboard-summary-dot dashboard-summary-dot-${tone}`} aria-hidden="true" /><p>{label}</p><strong>{value === null ? '—' : value.toLocaleString()}</strong><small>{detail}</small></article>
}

function HomeNodeCard({ network, node }: NodeRecord) {
  const tone = toneFor(node.health)
  const metrics = [
    ['RPC', node.rpcState],
    ['Sync', node.syncState],
    ['Consensus', node.consensusState],
    ['Process', node.processState],
    ['Resync', node.resyncState],
    ['Freshness', node.freshness ?? 'Unknown'],
  ]
  return (
    <article className={`dashboard-node-card dashboard-node-card-${tone}`}>
      <header className="dashboard-node-header">
        <div className="dashboard-node-title"><span className={`dashboard-node-status dashboard-node-status-${tone}`} aria-hidden="true" /><div><h2><Link to={`/nodes/${node.nodeId}`}>{nodeLabel(node)}</Link></h2><p><Link className="dashboard-card-network" to={`/networks/${network.networkKey}`}>{network.displayName}</Link> · <span className="breakable">{node.nodeId}</span></p></div></div>
        <span className={`dashboard-health-label dashboard-health-label-${tone}`}>{node.health}</span>
      </header>
      <p className="dashboard-health-reason">{node.healthReason}</p>
      <dl className="dashboard-node-metrics">{metrics.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value || 'Unknown'}</dd></div>)}</dl>
      <dl className="dashboard-observation-row">
        <Observation label="Current Head" value={formatNumber(node.currentHead)} />
        <Observation label="History Boundary" value={formatNumber(node.historicalHighWatermark)} />
        <Observation label="Peer Count" value={formatPeerCount(node)} />
        <Observation label="Peer Observation" value={formatPeerObservation(node)} />
        <Observation label="Host CPU" value={node.hostCpuPercent == null ? 'Unknown' : `${node.hostCpuPercent.toFixed(1)}%`} />
        <Observation label="Network Reference" value={`${formatNumber(node.networkReferenceHead)} · ${node.networkReferenceConfidence}`} />
      </dl>
      <footer className="dashboard-node-footer"><span>{node.resyncProgress ?? node.resyncState}</span><Link to={`/nodes/${node.nodeId}`}>Open Node Detail <span aria-hidden="true">↗</span></Link></footer>
    </article>
  )
}

function Observation({ label, value }: { label: string; value: string }) { return <div><dt>{label}</dt><dd>{value}</dd></div> }
function nodeLabel(node: PublicNode) { return node.displayName ?? node.nodeId }
function formatNumber(value: number | null | undefined) { return value == null ? 'Unknown' : value.toLocaleString() }
function formatPeerCount(node: PublicNode) { return node.peers?.peerCount == null ? 'Unknown' : node.peers.peerCount.toLocaleString() }
function formatPeerObservation(node: PublicNode) {
  const peer = node.peers
  if (!peer) return 'Unknown'
  const freshness = peer.freshness || 'Unknown'
  const staleSince = peer.staleSince ? ` · stale since ${peer.staleSince}` : ''
  return `${peer.state} · ${freshness}${staleSince}`
}
function isHealthy(value: string) { return value.toLowerCase() === 'healthy' }
function healthRank(value: string) { const tone = toneFor(value); return tone === 'bad' ? 0 : tone === 'warn' ? 1 : tone === 'good' ? 2 : 3 }
function toneFor(value: string): 'good' | 'warn' | 'bad' | 'neutral' {
  const normalized = value.toLowerCase()
  if (/(error|failed|unhealthy|offline|unavailable)/.test(normalized)) return 'bad'
  if (normalized === 'live' || /(healthy|current|connected|ready|synced|active|running|ok|fresh)/.test(normalized)) return 'good'
  if (/(starting|unknown|unsupported|disabled|empty|stale|resync|degraded|connecting)/.test(normalized)) return 'warn'
  return 'neutral'
}
