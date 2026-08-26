import { useMemo, useState } from 'react'
import { Link } from 'react-router'
import type { PublicConsensusInsight, PublicNetwork, PublicNode } from '../api/generated'
import { realtimeStreamLabel } from './RealtimeNotice'
import { StatusBadge } from './StatusBadge'

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
        <SummaryCard label="Published Nodes" value={hasProjection ? records.length : null} tone="indigo" />
        <SummaryCard label="Healthy Nodes" value={healthyCount} tone="green" />
        <SummaryCard label="Attention" value={healthyCount === null ? null : records.length - healthyCount} tone={healthyCount !== null && records.length === healthyCount ? 'green' : 'red'} />
        <SummaryCard label="Networks" value={hasProjection ? networks.length : null} tone="violet" />
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

function SummaryCard({ label, value, tone }: { label: string; value: number | null; tone: string }) {
  return <article className="dashboard-summary-card"><span className={`dashboard-summary-dot dashboard-summary-dot-${tone}`} aria-hidden="true" /><p>{label}</p><strong>{value === null ? '—' : value.toLocaleString()}</strong></article>
}

/**
 * Compact Home card: ONE whole-card semantic link to Node Detail. Healthy
 * Nodes carry no routine prose (health reason, Last Observed, component
 * status rows, "no active resync"); only an exceptional Node keeps a single
 * short diagnostic line (issue #97).
 */
function HomeNodeCard({ network, node }: NodeRecord) {
  const tone = toneFor(node.health)
  const activity = activityBadge(node)
  const diagnostic = exceptionalDiagnostic(node)
  return (
    <article className={`dashboard-node-card dashboard-node-card-${tone}`}>
      <Link className="dashboard-node-card-link" to={`/nodes/${node.nodeId}`}>
        <header className="dashboard-node-header">
          <div className="dashboard-node-title">
            <span className={`dashboard-node-status dashboard-node-status-${tone}`} aria-hidden="true" />
            <div>
              <h2>{nodeLabel(node)}</h2>
              <p className="dashboard-node-network">{network.displayName}</p>
            </div>
          </div>
          <div className="dashboard-node-badges">
            <StatusBadge status={activity.status} tone={activity.tone} />
            <StatusBadge status={healthLabel(node.health)} tone={statusTone(tone)} />
          </div>
        </header>
        <ResourceRow node={node} />
        <div className="dashboard-node-primary" aria-label="Node highlights">
          <Metric label="HEAD" value={formatNumber(node.currentHead)} />
          <Metric label="TXS" value={formatNumber(node.latestBlockTransactionCount)} />
          <Metric label="PEERS" value={formatPeerCount(node)} detail={formatPeerObservation(node)} />
        </div>
        <ConsensusRow consensus={node.consensus} />
        {diagnostic && <p className="dashboard-node-diagnostic">{diagnostic}</p>}
      </Link>
    </article>
  )
}

function ResourceRow({ node }: { node: PublicNode }) {
  return (
    <div className="dashboard-node-resources" aria-label="Host resources">
      <Metric label="CPU" value={formatPercent(node.hostCpuPercent)} />
      <Metric label="MEMORY" value={formatPercent(node.hostMemoryPercent)} />
      <Metric label="STORAGE" value={formatPercent(node.hostStoragePercent)} />
      <Metric label="↑ UP" value={formatRate(node.hostNetworkTxBytesPerSec)} />
      <Metric label="↓ DOWN" value={formatRate(node.hostNetworkRxBytesPerSec)} />
    </div>
  )
}

function formatPercent(value: number | null | undefined) {
  return value == null ? '—' : `${value.toFixed(1)}%`
}

function formatRate(value: number | null | undefined) {
  if (value == null) return '—'
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

function Metric({ label, value, detail }: { label: string; value: string; detail?: string }) {
  return <div className="dashboard-node-primary-metric"><span>{label}</span><strong>{value}</strong>{detail && <small>{detail}</small>}</div>
}

/**
 * The final compact metric row: QC, LOCKED, COMMITTED, and VALIDATOR come
 * from the Node-scoped last-good consensus observation (issue #99). Missing,
 * unsupported, disabled, and never-observed values render Unknown, never
 * zero or False; failed or stale collections keep the last-good values and
 * visibly mark them Stale.
 */
function ConsensusRow({ consensus }: { consensus: PublicConsensusInsight | undefined }) {
  const status = consensusValueStatus(consensus)
  const detail = status === 'stale' ? 'Stale' : undefined
  return (
    <div className="dashboard-node-consensus" aria-label="Consensus progress">
      <Metric label="QC" value={formatConsensusBlock(consensus?.highestQcBlock, status)} detail={detail} />
      <Metric label="LOCKED" value={formatConsensusBlock(consensus?.highestLockBlock, status)} detail={detail} />
      <Metric label="COMMITTED" value={formatConsensusBlock(consensus?.highestCommitBlock, status)} detail={detail} />
      <Metric label="VALIDATOR" value={formatConsensusValidator(consensus, status)} detail={detail} />
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
  const peer = node.peers
  if (peer?.peerCount == null) return 'Unknown'
  // Starting/Disabled/Unsupported do not provide a usable value; only a
  // successful observation may show an authoritative zero (webui.md §5.3).
  if (peer.peerCount === 0 && ['starting', 'disabled', 'unsupported'].includes(peer.state)) return 'Unknown'
  return peer.peerCount.toLocaleString()
}
function formatPeerObservation(node: PublicNode) {
  const peer = node.peers
  if (!peer) return 'Unknown observation'
  if (peer.state === 'error') return peer.peerCount == null ? 'Error; no last-good value' : 'Error; showing last-good value'
  if (peer.state === 'starting') return 'Starting; no usable snapshot yet'
  if (peer.state === 'disabled') return 'Disabled; Peer observation is not configured'
  if (peer.state === 'unsupported') return 'Unsupported; no supported Peer snapshot'
  if (peer.peerCount == null || peer.state === 'unknown') return 'Unknown observation'
  if (peer.freshness === 'stale') return 'Stale; showing last-good value'
  if (peer.peerCount === 0) return 'Empty; authoritative zero'
  return 'Current observation'
}
function statusTone(tone: ReturnType<typeof toneFor>): 'ok' | 'warning' | 'error' | 'neutral' {
  return tone === 'good' ? 'ok' : tone === 'bad' ? 'error' : tone === 'warn' ? 'warning' : 'neutral'
}

/**
 * Link-aware Validator Activity (issue #100). A Public Node exposes Activity
 * only through its effective explicit Node Validator Link. A Node without
 * an observed activity value renders Observing in the compact Home badge.
 * Provider Activity never affects the separate Node Health badge.
 */
function activityBadge(node: PublicNode): { status: string; tone: 'ok' | 'warning' | 'error' | 'neutral' } {
  const validator = node.validator
  const value = validator?.activity
  if (!value || value === 'unknown') return { status: 'Observing', tone: 'neutral' }
  if (value === 'observing') return { status: 'Observing', tone: 'neutral' }
  const label = value.charAt(0).toUpperCase() + value.slice(1)
  const stale = validator?.activityState === 'stale'
  const tone = stale || ['exiting', 'exited', 'verifying', 'locked'].includes(value) ? 'warning' : 'ok'
  return { status: stale ? `${label} (Stale)` : label, tone }
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
