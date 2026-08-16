import { useEffect, useState } from 'react'
import { Link, useParams } from 'react-router'
import { fetchNetwork, fetchNode, fetchNodeHistory, fetchNodeHistoryExport, usePublicRealtime } from '../api/public'
import type { PublicNetwork, PublicNode } from '../api/generated'
import { PeerInsight } from '../components/PeerInsight'
import { GeoInsight } from '../components/GeoInsight'

export function NetworkPage() {
  const { networkKey = '' } = useParams()
  const [network, setNetwork] = useState<PublicNetwork | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [reload, setReload] = useState(0)
  usePublicRealtime(
    () => setReload((value) => value + 1),
    () => {
      setNetwork(null)
      setError(null)
      setReload((value) => value + 1)
    },
  )
  useEffect(() => { fetchNetwork(networkKey).then(setNetwork).catch((e: Error) => setError(e.message)) }, [networkKey, reload])

  if (error) return <section className="page"><p role="alert" className="form-error">{error}</p><Link to="/">Back to Home</Link></section>
  if (!network) return <section className="page"><p role="status">Loading Network…</p></section>
  return <section className="page"><p><Link to="/">← All Networks</Link></p><h1>{network.displayName}</h1><p className="muted">{network.networkKey}</p><PeerInsight insight={network.peers} /><GeoInsight insight={network.geo} /><div className="node-grid">{network.nodes.map((node) => <NodeCard node={node} key={node.nodeId} />)}</div></section>
}

export function NodePage() {
  const { nodeId = '' } = useParams()
  const [node, setNode] = useState<PublicNode | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [history, setHistory] = useState<Awaited<ReturnType<typeof fetchNodeHistory>>>([])
  const [reload, setReload] = useState(0)
  usePublicRealtime(
    () => setReload((value) => value + 1),
    () => {
      setNode(null)
      setHistory([])
      setError(null)
      setReload((value) => value + 1)
    },
  )
  useEffect(() => { fetchNode(nodeId).then(setNode).catch((e: Error) => setError(e.message)); fetchNodeHistory(nodeId).then(setHistory).catch(() => setHistory([])) }, [nodeId, reload])

  if (error) return <section className="page"><p role="alert" className="form-error">{error}</p><Link to="/">Back to Home</Link></section>
  if (!node) return <section className="page"><p role="status">Loading Node…</p></section>
  return <section className="page"><p><Link to={`/networks/${node.networkKey}`}>← {node.networkKey}</Link></p><h1>{node.displayName ?? 'Node detail'}</h1><p><button type="button" onClick={() => { void fetchNodeHistoryExport(nodeId).then((items) => { const blob = new Blob([JSON.stringify(items, null, 2)], { type: 'application/json' }); const url = URL.createObjectURL(blob); const anchor = document.createElement('a'); anchor.href = url; anchor.download = 'public-history.json'; anchor.click(); URL.revokeObjectURL(url) }).catch(() => setError('Unable to export block history')) }}>Export public history</button></p><PeerInsight insight={node.peers} /><dl className="detail-list"><dt>Node ID</dt><dd>{node.nodeId}</dd><dt>Health</dt><dd><span className={`status status-${node.health}`}>{node.health}</span> — {node.healthReason}</dd><dt>RPC state</dt><dd>{node.rpcState}</dd><dt>Sync state</dt><dd>{node.syncState}</dd><dt>Consensus state</dt><dd>{node.consensusState}</dd><dt>Current head</dt><dd>{node.currentHead ?? 'Unknown'}</dd><dt>Historical high-water mark</dt><dd>{node.historicalHighWatermark ?? 'Unknown'}</dd><dt>Resync state</dt><dd>{node.resyncState}{node.resyncProgress ? ` — ${node.resyncProgress}` : ''}</dd><dt>Network reference</dt><dd>{node.networkReferenceHead ?? 'Unknown'} ({node.networkReferenceConfidence})</dd><dt>Freshness</dt><dd>{node.freshness ?? 'Never observed'}</dd><dt>Host CPU</dt><dd>{node.hostCpuPercent == null ? 'Unknown' : `${node.hostCpuPercent.toFixed(1)}%`}</dd></dl><h2>Block history</h2><div className="history-list">{history.length === 0 ? <p className="muted">No block history observed yet.</p> : history.map((block) => <article className="node-card" key={`${block.height ?? 'gap'}-${block.observedAt ?? ''}`}><strong>{block.height == null ? (block.divergenceKind ? `Chain divergence at height ${block.gapFromHeight ?? '?' } · ${block.divergenceReason ?? 'recent identity mismatch observed'}` : `History gap ${block.gapFromHeight ?? '?'}–${block.gapToHeight ?? '?'}`) : `Height ${block.height}`}</strong><span> · {block.height == null ? 'No sample available; chart disconnected' : `${block.blockTimeMs == null ? 'time unknown' : new Date(block.blockTimeMs).toISOString()} · ${block.transactionCount == null ? 'transactions unknown' : `${block.transactionCount} transactions`}`}</span><p className="muted">{block.height == null ? (block.divergenceKind ? `${block.divergenceReason ?? 'Recent chain divergence observed'}; raw evidence is withheld from Public.` : `${block.gapKind ?? 'gap'}: ${block.gapReason ?? 'bounded recovery did not recover this interval'}`) : `Freshness: ${block.freshness ?? 'unknown'} · Coinbase: ${block.coinbase ?? 'unknown'} · Seal signer: ${block.sealSignerMatch ?? 'unknown'} · Protocol proposer: ${block.protocolProposer ?? 'unknown'}`}</p></article>)}</div></section>
}

function NodeCard({ node }: { node: PublicNode }) {
  return <article className="node-card"><h2><Link to={`/nodes/${node.nodeId}`}>{node.displayName ?? node.nodeId}</Link></h2><p><span className={`status status-${node.health}`}>{node.health}</span> {node.healthReason}</p><p className="muted">RPC: {node.rpcState} · Sync: {node.syncState} · Consensus: {node.consensusState} · Head: {node.currentHead ?? 'unknown'} · History: {node.historicalHighWatermark ?? 'unknown'} · {node.resyncState}</p><PeerInsight insight={node.peers} compact /></article>
}
