import { useState } from 'react'
import { Link, useParams } from 'react-router'
import {
  fetchNodeHistoryExport,
  usePublicNetwork,
  usePublicNode,
  usePublicNodeHistory,
  usePublicNodePeerHistory,
  usePublicValidatorAnalytics,
  usePublicValidatorHistory,
} from '../api/public'
import type { PublicNode, PublicValidatorInsight } from '../api/generated'
import { useHomeRealtimeContext } from '../layouts/HomeLayout'
import { PeerInsight } from '../components/PeerInsight'
import { PeerHistoryInsight, normalizePublicPeerHistory } from '../components/PeerHistoryInsight'
import { GeoInsight } from '../components/GeoInsight'
import { ValidatorInsight } from '../components/ValidatorInsight'
import { ValidatorAnalytics } from '../components/ValidatorAnalytics'
import { formatObservedAt, StatusBadge } from '../components/StatusBadge'
import { RealtimeNotice } from '../components/RealtimeNotice'

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
  const peerHistoryQuery = usePublicNodePeerHistory(nodeId, generation)
  const [activeTab, setActiveTab] = useState<'details' | 'network'>('details')
  const [exportError, setExportError] = useState<string | null>(null)

  if (resetting) return <section className="page"><p role="status">Revalidating Node access…</p></section>
  if (nodeQuery.isPending) return <section className="page"><p role="status">Loading Node…</p></section>
  if (nodeQuery.error && !nodeQuery.data) return <section className="page"><p role="alert" className="form-error">{nodeQuery.error instanceof Error ? nodeQuery.error.message : 'Unable to load Node'}</p><Link to="/">Back to Home</Link></section>
  if (!nodeQuery.data) return <section className="page"><p role="status">Node unavailable.</p><Link to="/">Back to Home</Link></section>

  const node = nodeQuery.data
  const exportHistory = async () => {
    setExportError(null)
    try {
      const items = await fetchNodeHistoryExport(nodeId, undefined, generation)
      const blob = new Blob([JSON.stringify(items, null, 2)], { type: 'application/json' })
      const url = URL.createObjectURL(blob)
      const anchor = document.createElement('a')
      anchor.href = url
      anchor.download = 'public-history.json'
      anchor.click()
      URL.revokeObjectURL(url)
    } catch {
      setExportError('Unable to export block history')
    }
  }

  return <section className="page node-detail-page">
    <RealtimeNotice realtime={realtime} />
    {nodeQuery.isRefetchError && <p role="status" className="form-error">Node refresh failed; showing the last successful Node data.</p>}
    <div className="node-detail-breadcrumb">
      <Link to={`/networks/${node.networkKey}`}>← {node.networkKey}</Link>
      <span aria-hidden="true">/</span>
      <span>Public Node Detail</span>
    </div>
    <header className="node-detail-heading">
      <div>
        <p className="dashboard-kicker">PLATPULSE / NODE DETAIL</p>
        <h1>{node.displayName ?? 'Node detail'}</h1>
        <p className="node-id-line">Node ID <code>{node.nodeId}</code></p>
        <p className="node-id-line">Network key <code>{node.networkKey}</code></p>
      </div>
      <div className="node-detail-actions">
        <StatusBadge status={nodeHealthLabel(node.health)} tone={nodeHealthTone(node.health)} />
        <button className="secondary-action" type="button" onClick={() => void exportHistory()}>Export public history</button>
      </div>
    </header>
    {exportError && <p role="alert" className="form-error">{exportError}</p>}

    <section className="node-overview-panel panel" aria-labelledby="node-overview-title">
      <div className="node-overview-heading">
        <div>
          <h2 id="node-overview-title">Node Health Summary</h2>
          <p className="health-reason">{node.healthReason}</p>
        </div>
        <div className="node-overview-freshness">
          <StatusBadge status={freshnessLabel(node.freshness)} tone={statusTone(freshnessLabel(node.freshness))} />
          <span>{freshnessDetail(node.freshness)}</span>
        </div>
      </div>
      <div className="node-overview-grid">
        <div className="node-headline-metric"><span>Current Head</span><strong>{formatNumber(node.currentHead)}</strong><small>Latest accepted Node observation</small></div>
        <div className="node-headline-metric"><span>Peers</span><strong>{peerCount(node.peers)}</strong><small>{peerBreakdown(node.peers)}</small></div>
        <div className="node-overview-statuses" aria-label="Node component status">
          <NodeOverviewStatus label="RPC" value={nodeComponentStateLabel(node.rpcState)} />
          <NodeOverviewStatus label="Sync" value={nodeComponentStateLabel(node.syncState)} />
          <NodeOverviewStatus label="Consensus" value={nodeComponentStateLabel(node.consensusState)} />
        </div>
      </div>
    </section>

    <div className="node-tabs" role="tablist" aria-label="Node detail views">
      <TabButton id="node-details-tab" panelId="node-details-panel" selected={activeTab === 'details'} onSelect={() => setActiveTab('details')} onNavigate={(tab) => { setActiveTab(tab); document.getElementById(`node-${tab}-tab`)?.focus() }}>Details</TabButton>
      <TabButton id="node-network-tab" panelId="node-network-panel" selected={activeTab === 'network'} onSelect={() => setActiveTab('network')} onNavigate={(tab) => { setActiveTab(tab); document.getElementById(`node-${tab}-tab`)?.focus() }}>Network</TabButton>
    </div>

    <section id="node-details-panel" className="node-tabpanel" role="tabpanel" aria-labelledby="node-details-tab" aria-label="Details" hidden={activeTab !== 'details'}>
      <h2 className="sr-only">Details</h2>
      <section className="panel node-history-panel" aria-labelledby="node-history-title">
        <div className="panel-heading">
          <div>
            <h2 id="node-history-title">Bounded Block History</h2>
            <p className="muted">Server-configured history window; absent blocks are not zero. The exact bound is not part of the Public Projection.</p>
          </div>
          <span className="history-window-label">Best effort</span>
        </div>
        {historyQuery.error && <p role="status" className="form-error">Block History is Error; retained history is unavailable. <Link to={`/networks/${node.networkKey}`}>Return to Network</Link> and try again.</p>}
        {historyQuery.isPending && <p role="status">Block History is Starting; loading the Server window…</p>}
        {!historyQuery.isPending && !historyQuery.error && (historyQuery.data?.filter((block) => block.height != null).length ?? 0) === 0 && (historyQuery.data?.filter((block) => block.gapFromHeight != null || block.gapToHeight != null).length ?? 0) === 0 && <p className="panel-state">Block History is Empty; no retained Block Summary is available in the Server window.</p>}
        {!historyQuery.isPending && !historyQuery.error && (historyQuery.data?.filter((block) => block.gapFromHeight != null || block.gapToHeight != null).length ?? 0) > 0 && <p className="panel-state">No block summaries are available in this window; the Server recorded history gaps instead.</p>}
        <div className="history-list">
          {historyQuery.data?.filter((block) => block.height != null).map((block) => <HistoryCard block={block} key={`${block.height}-${block.observedAt ?? ''}`} />)}
          {historyQuery.data?.filter((block) => block.gapFromHeight != null || block.gapToHeight != null).slice(0, 3).map((gap) => <HistoryGapCard gap={gap} key={`${gap.gapFromHeight}-${gap.gapToHeight}-${gap.observedAt ?? ''}`} />)}
        </div>
      </section>
      <section className="panel node-chain-context" aria-labelledby="node-chain-context-title">
        <div className="panel-heading">
          <div>
            <h2 id="node-chain-context-title">Chain context</h2>
            <p className="muted">Supporting observations explain the current position without repeating the headline status.</p>
          </div>
        </div>
        <dl className="node-observation-grid" aria-label="Supporting Node observations">
          <Observation label="History Boundary" value={formatNumber(node.historicalHighWatermark)} />
          <Observation label="Network Reference" value={formatNumber(node.networkReferenceHead)} />
          <Observation label="Reference Confidence" value={confidenceLabel(node.networkReferenceConfidence)} />
          <Observation label="Host CPU" value={formatPercent(node.hostCpuPercent)} detail="Sanitized Host observation" />
          <Observation label="Process" value={nodeComponentStateLabel(node.processState)} detail="Node Process observation" />
          <Observation label="Resync" value={nodeComponentStateLabel(node.resyncState)} detail={resyncDetail(node)} />
        </dl>
      </section>
      {node.validator && <ValidatorDetails validator={node.validator} generation={generation} />}
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

function Observation({ label, value, detail }: { label: string; value: string; detail?: string }) {
  return <div><dt>{label}</dt><dd>{value}</dd>{detail && <small>{detail}</small>}</div>
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

function statusTone(value: string): 'ok' | 'warning' | 'error' | 'neutral' {
  return stateTone(value)
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

function confidenceLabel(value: string | null | undefined): string {
  if (!value || value === 'unknown') return 'Unknown'
  return value.charAt(0).toUpperCase() + value.slice(1)
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

function ValidatorDetails({ validator, generation }: { validator: PublicValidatorInsight; generation: number }) {
  const analytics = usePublicValidatorAnalytics(validator.validatorId, 31, generation)
  return <>
    <ValidatorInsight insight={validator} />
    {analytics.data && <ValidatorAnalytics analytics={analytics.data} compact />}
    {analytics.error && <p role="status" className="muted">Validator analytics unavailable; the core Node view remains available.</p>}
  </>
}

function HistoryCard({ block }: { block: NonNullable<ReturnType<typeof usePublicNodeHistory>['data']>[number] }) {
  if (block.height == null) return null
  const detail = `Freshness: ${block.freshness ?? 'unknown'} · Coinbase: ${block.coinbase ?? 'unknown'} · Seal signer: ${block.sealSignerMatch ?? 'unknown'} · Protocol proposer: ${block.protocolProposer ?? 'unknown'}`
  return <article className="node-card"><strong>Height {block.height}</strong><span> · {block.blockTimeMs == null ? 'time unknown' : new Date(block.blockTimeMs).toISOString()} · {block.transactionCount == null ? 'transactions unknown' : `${block.transactionCount} transactions`}</span><p className="muted">{detail}</p></article>
}

function resyncDetail(node: PublicNode): string {
  return nodeComponentStateLabel(node.resyncState) === 'Current' ? 'No active resync' : node.resyncProgress ?? 'Progress unavailable'
}

function HistoryGapCard({ gap }: { gap: NonNullable<ReturnType<typeof usePublicNodeHistory>['data']>[number] }) {
  const from = gap.gapFromHeight == null ? 'Unknown' : gap.gapFromHeight.toLocaleString()
  const to = gap.gapToHeight == null ? 'Unknown' : gap.gapToHeight.toLocaleString()
  return <article className="node-card history-gap-card"><strong>History gap · {from}–{to}</strong><span>{gap.gapKind ?? 'Unclassified gap'} · {formatObservedAt(gap.observedAt)}</span><p className="muted">{gap.gapReason ?? 'The interval could not be recovered.'}</p></article>
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
