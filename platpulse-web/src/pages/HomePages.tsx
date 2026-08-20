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

export function NetworkPage() {
  const { networkKey = '' } = useParams()
  const { generation, resetting } = useHomeRealtimeContext()
  const query = usePublicNetwork(networkKey, generation)

  if (resetting) return <section className="page"><p role="status">Revalidating Home access…</p></section>
  if (query.isPending) return <section className="page"><p role="status">Loading Network…</p></section>
  if (query.error) return <section className="page"><p role="alert" className="form-error">{query.error instanceof Error ? query.error.message : 'Unable to load Network'}</p><Link to="/">Back to Home</Link></section>
  if (!query.data) return <section className="page"><p role="status">Network unavailable.</p><Link to="/">Back to Home</Link></section>

  const network = query.data
  return <section className="page">
    <p><Link to="/">← All Networks</Link></p>
    <h1>{network.displayName}</h1>
    <p className="muted">{network.networkKey}</p>
    <PeerInsight insight={network.peers} />
    <GeoInsight insight={network.geo} />
    {network.validators.length > 0 && <>
      <h2>Validators</h2>
      <div className="node-grid">{network.validators.map((validator) => <ValidatorCard key={validator.validatorId} validator={validator} generation={generation} />)}</div>
    </>}
    <div className="node-grid">{network.nodes.map((node) => <NodeCard node={node} key={node.nodeId} />)}</div>
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
  const { generation, resetting } = useHomeRealtimeContext()
  const nodeQuery = usePublicNode(nodeId, generation)
  const historyQuery = usePublicNodeHistory(nodeId, generation)
  const peerHistoryQuery = usePublicNodePeerHistory(nodeId, generation)
  const [exportError, setExportError] = useState<string | null>(null)

  if (resetting) return <section className="page"><p role="status">Revalidating Node access…</p></section>
  if (nodeQuery.isPending) return <section className="page"><p role="status">Loading Node…</p></section>
  if (nodeQuery.error) return <section className="page"><p role="alert" className="form-error">{nodeQuery.error instanceof Error ? nodeQuery.error.message : 'Unable to load Node'}</p><Link to="/">Back to Home</Link></section>
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

  return <section className="page">
    <p><Link to={`/networks/${node.networkKey}`}>← {node.networkKey}</Link></p>
    <h1>{node.displayName ?? 'Node detail'}</h1>
    <p><button type="button" onClick={() => void exportHistory()}>Export public history</button></p>
    {exportError && <p role="alert" className="form-error">{exportError}</p>}
    <PeerInsight insight={node.peers} />
    {node.validator && <ValidatorDetails validator={node.validator} generation={generation} />}
    <PeerHistoryInsight
      history={peerHistoryQuery.data ? normalizePublicPeerHistory(peerHistoryQuery.data) : undefined}
      error={Boolean(peerHistoryQuery.error)}
      loading={peerHistoryQuery.isPending}
    />
    <dl className="detail-list">
      <dt>Node ID</dt><dd>{node.nodeId}</dd>
      <dt>Health</dt><dd><Status value={node.health} /> — {node.healthReason}</dd>
      <dt>RPC state</dt><dd>{node.rpcState}</dd>
      <dt>Sync state</dt><dd>{node.syncState}</dd>
      <dt>Consensus state</dt><dd>{node.consensusState}</dd>
      <dt>Current head</dt><dd>{node.currentHead ?? 'Unknown'}</dd>
      <dt>Historical high-water mark</dt><dd>{node.historicalHighWatermark ?? 'Unknown'}</dd>
      <dt>Resync state</dt><dd>{node.resyncState}{node.resyncProgress ? ` — ${node.resyncProgress}` : ''}</dd>
      <dt>Network reference</dt><dd>{node.networkReferenceHead ?? 'Unknown'} ({node.networkReferenceConfidence})</dd>
      <dt>Freshness</dt><dd>{node.freshness ?? 'Never observed'}</dd>
      <dt>Host CPU</dt><dd>{node.hostCpuPercent == null ? 'Unknown' : `${node.hostCpuPercent.toFixed(1)}%`}</dd>
    </dl>
    <h2>Block history</h2>
    {historyQuery.error && <p role="status" className="form-error">Block history unavailable.</p>}
    {historyQuery.isPending && <p role="status">Loading block history…</p>}
    <div className="history-list">
      {historyQuery.data?.length === 0 && <p className="muted">No block history observed yet.</p>}
      {historyQuery.data?.map((block) => <HistoryCard block={block} key={`${block.height ?? 'gap'}-${block.observedAt ?? ''}`} />)}
    </div>
  </section>
}

function ValidatorDetails({ validator, generation }: { validator: PublicValidatorInsight; generation: number }) {
  const analytics = usePublicValidatorAnalytics(validator.validatorId, 31, generation)
  return <>
    <ValidatorInsight insight={validator} />
    {analytics.data && <ValidatorAnalytics analytics={analytics.data} compact />}
  </>
}

function HistoryCard({ block }: { block: NonNullable<ReturnType<typeof usePublicNodeHistory>['data']>[number] }) {
  const title = block.height == null
    ? block.divergenceKind
      ? `Chain divergence at height ${block.gapFromHeight ?? '?'} · ${block.divergenceReason ?? 'recent identity mismatch observed'}`
      : `History gap ${block.gapFromHeight ?? '?'}–${block.gapToHeight ?? '?'}`
    : `Height ${block.height}`
  const detail = block.height == null
    ? block.divergenceKind
      ? `${block.divergenceReason ?? 'Recent chain divergence observed'}; raw evidence is withheld from Public.`
      : `${block.gapKind ?? 'gap'}: ${block.gapReason ?? 'bounded recovery did not recover this interval'}`
    : `Freshness: ${block.freshness ?? 'unknown'} · Coinbase: ${block.coinbase ?? 'unknown'} · Seal signer: ${block.sealSignerMatch ?? 'unknown'} · Protocol proposer: ${block.protocolProposer ?? 'unknown'}`
  return <article className="node-card"><strong>{title}</strong><span> · {block.height == null ? 'No sample available; chart disconnected' : `${block.blockTimeMs == null ? 'time unknown' : new Date(block.blockTimeMs).toISOString()} · ${block.transactionCount == null ? 'transactions unknown' : `${block.transactionCount} transactions`}`}</span><p className="muted">{detail}</p></article>
}

function Status({ value }: { value: string }) {
  return <span className={`status status-${value}`}>{value}</span>
}

function NodeCard({ node }: { node: PublicNode }) {
  return <article className="node-card"><h2><Link to={`/nodes/${node.nodeId}`}>{node.displayName ?? node.nodeId}</Link></h2><p><Status value={node.health} /> {node.healthReason}</p><p className="muted">RPC: {node.rpcState} · Sync: {node.syncState} · Consensus: {node.consensusState} · Head: {node.currentHead ?? 'unknown'} · History: {node.historicalHighWatermark ?? 'unknown'} · {node.resyncState}</p>{node.validator && <ValidatorInsight insight={node.validator} compact />}<PeerInsight insight={node.peers} compact /></article>
}
