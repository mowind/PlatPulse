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

    <section className="node-summary-panel panel" aria-labelledby="node-summary-title">
      <div className="panel-heading">
        <div>
          <h2 id="node-summary-title">Node Health Summary</h2>
          <p className="health-reason">{node.healthReason}</p>
        </div>
        <StatusBadge status={freshnessLabel(node.freshness)} tone={statusTone(freshnessLabel(node.freshness))} />
        <p className="muted node-freshness-detail">{freshnessDetail(node.freshness)}</p>
      </div>
      <div className="node-fact-grid" aria-label="Node independent facts">
        <NodeFact label="Health" value={nodeHealthLabel(node.health)} tone={nodeHealthTone(node.health)} />
        <NodeFact label="RPC" value={nodeComponentStateLabel(node.rpcState)} tone={stateTone(node.rpcState)} />
        <NodeFact label="Sync" value={nodeComponentStateLabel(node.syncState)} tone={stateTone(node.syncState)} />
        <NodeFact label="Consensus" value={nodeComponentStateLabel(node.consensusState)} tone={stateTone(node.consensusState)} />
        <NodeFact label="Process" value={nodeComponentStateLabel(node.processState)} tone={stateTone(node.processState)} />
        <NodeFact label="Resync" value={nodeComponentStateLabel(node.resyncState)} tone={stateTone(node.resyncState)} detail={node.resyncProgress ?? undefined} />
      </div>
      <dl className="node-observation-grid" aria-label="Node independent observations">
        <Observation label="Current Head" value={formatNumber(node.currentHead)} />
        <Observation label="History Boundary" value={formatNumber(node.historicalHighWatermark)} />
        <Observation label="Network Reference" value={formatNumber(node.networkReferenceHead)} />
        <Observation label="Reference Confidence" value={confidenceLabel(node.networkReferenceConfidence)} />
        <Observation label="Host CPU" value={formatPercent(node.hostCpuPercent)} detail="Sanitized Host observation" />
        <Observation label="Peer Count" value={peerCount(node.peers)} detail={peerCountDetail(node.peers)} />
      </dl>
    </section>

    <div className="node-tabs" role="tablist" aria-label="Node detail views">
      <TabButton id="node-details-tab" panelId="node-details-panel" selected={activeTab === 'details'} onSelect={() => setActiveTab('details')} onNavigate={(tab) => { setActiveTab(tab); document.getElementById(`node-${tab}-tab`)?.focus() }}>Details</TabButton>
      <TabButton id="node-network-tab" panelId="node-network-panel" selected={activeTab === 'network'} onSelect={() => setActiveTab('network')} onNavigate={(tab) => { setActiveTab(tab); document.getElementById(`node-${tab}-tab`)?.focus() }}>Network</TabButton>
    </div>

    <section id="node-details-panel" className="node-tabpanel" role="tabpanel" aria-labelledby="node-details-tab" aria-label="Details" hidden={activeTab !== 'details'}>
      <h2 className="sr-only">Details</h2>
      <div className="node-signal-grid" aria-label="Current observation signals">
        <SignalCard label="Host CPU" value={formatPercent(node.hostCpuPercent)} detail="Shared Host observation" />
        <SignalCard label="Current Head" value={formatNumber(node.currentHead)} detail="Latest accepted Node observation" />
        <SignalCard label="History Boundary" value={formatNumber(node.historicalHighWatermark)} detail="Historical high-water mark" />
        <SignalCard label="Peers" value={peerCount(node.peers)} detail={peerCountDetail(node.peers)} />
        <SignalCard label="RPC" value={nodeComponentStateLabel(node.rpcState)} detail="Independent RPC observation" status={nodeComponentStateLabel(node.rpcState)} />
        <SignalCard label="Consensus" value={nodeComponentStateLabel(node.consensusState)} detail="Independent consensus observation" status={nodeComponentStateLabel(node.consensusState)} />
      </div>
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
        {!historyQuery.isPending && !historyQuery.error && (historyQuery.data?.filter((block) => block.height != null).length ?? 0) === 0 && <p className="panel-state">Block History is Empty; no retained Block Summary is available in the Server window.</p>}
        <div className="history-list">
          {historyQuery.data?.filter((block) => block.height != null).map((block) => <HistoryCard block={block} key={`${block.height}-${block.observedAt ?? ''}`} />)}
        </div>
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

function NodeFact({ label, value, tone, detail }: { label: string; value: string; tone: 'ok' | 'warning' | 'error' | 'neutral'; detail?: string }) {
  return <article className="node-fact"><span>{label}</span><StatusBadge status={value} tone={tone} />{detail && <small>{detail}</small>}</article>
}

function Observation({ label, value, detail }: { label: string; value: string; detail?: string }) {
  return <div><dt>{label}</dt><dd>{value}</dd>{detail && <small>{detail}</small>}</div>
}

function SignalCard({ label, value, detail, status }: { label: string; value: string; detail: string; status?: string }) {
  return <article className="node-signal-card"><span className="node-signal-label">{label}</span><strong>{status ? <StatusBadge status={status} tone={stateTone(status)} /> : value}</strong><small>{detail}</small></article>
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

function peerCountDetail(insight: PublicNode['peers']): string {
  if (insight.state === 'error') return insight.peerCount == null ? 'Error; no last-good value' : `Error; last-good value received ${formatObservedAt(insight.receivedAt)}`
  if (insight.freshness === 'stale') return `Stale; last-good value received ${formatObservedAt(insight.receivedAt)}`
  if (insight.peerCount === 0) return 'Empty; authoritative successful zero'
  if (insight.peerCount == null) return 'Unknown; never observed'
  return `Current; observed ${formatObservedAt(insight.observedAt)}`
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

function NodeCard({ node }: { node: PublicNode }) {
  return <article className="node-card"><h2><Link to={`/nodes/${node.nodeId}`}>{node.displayName ?? node.nodeId}</Link></h2><p><StatusBadge status={nodeHealthLabel(node.health)} tone={nodeHealthTone(node.health)} /> {node.healthReason}</p><p className="muted">RPC: {nodeComponentStateLabel(node.rpcState)} · Sync: {nodeComponentStateLabel(node.syncState)} · Consensus: {nodeComponentStateLabel(node.consensusState)} · Head: {node.currentHead == null ? 'Unknown' : node.currentHead.toLocaleString()} · History: {node.historicalHighWatermark == null ? 'Unknown' : node.historicalHighWatermark.toLocaleString()} · {nodeComponentStateLabel(node.resyncState)}</p>{node.validator && <ValidatorInsight insight={node.validator} compact />}<PeerInsight insight={node.peers} compact /></article>
}
