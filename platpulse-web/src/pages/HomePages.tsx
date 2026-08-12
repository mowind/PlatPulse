import { useEffect, useState } from 'react'
import { Link, useParams } from 'react-router'
import { fetchNetwork, fetchNode, fetchNodeHistory, usePublicRealtime } from '../api/public'
import type { PublicNetwork, PublicNode } from '../api/generated'

export function NetworkPage() {
  const { networkKey = '' } = useParams()
  const [network, setNetwork] = useState<PublicNetwork | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [reload, setReload] = useState(0)
  usePublicRealtime(() => setReload((value) => value + 1))
  useEffect(() => { fetchNetwork(networkKey).then(setNetwork).catch((e: Error) => setError(e.message)) }, [networkKey, reload])

  if (error) return <section className="page"><p role="alert" className="form-error">{error}</p><Link to="/">Back to Home</Link></section>
  if (!network) return <section className="page"><p role="status">Loading Network…</p></section>
  return <section className="page"><p><Link to="/">← All Networks</Link></p><h1>{network.displayName}</h1><p className="muted">{network.networkKey}</p><div className="node-grid">{network.nodes.map((node) => <NodeCard node={node} key={node.nodeId} />)}</div></section>
}

export function NodePage() {
  const { nodeId = '' } = useParams()
  const [node, setNode] = useState<PublicNode | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [reload, setReload] = useState(0)
  usePublicRealtime(() => setReload((value) => value + 1))
  const [history, setHistory] = useState<Awaited<ReturnType<typeof fetchNodeHistory>>>([])
  useEffect(() => { fetchNode(nodeId).then(setNode).catch((e: Error) => setError(e.message)); fetchNodeHistory(nodeId).then(setHistory).catch(() => setHistory([])) }, [nodeId, reload])

  if (error) return <section className="page"><p role="alert" className="form-error">{error}</p><Link to="/">Back to Home</Link></section>
  if (!node) return <section className="page"><p role="status">Loading Node…</p></section>
  return <section className="page"><p><Link to={`/networks/${node.networkKey}`}>← {node.networkKey}</Link></p><h1>{node.displayName ?? 'Node detail'}</h1><dl className="detail-list"><dt>Node ID</dt><dd>{node.nodeId}</dd><dt>Health</dt><dd><span className={`status status-${node.health}`}>{node.health}</span> — {node.healthReason}</dd><dt>RPC state</dt><dd>{node.rpcState}</dd><dt>Sync state</dt><dd>{node.syncState}</dd><dt>Consensus state</dt><dd>{node.consensusState}</dd><dt>Freshness</dt><dd>{node.freshness ?? 'Never observed'}</dd><dt>Host CPU</dt><dd>{node.hostCpuPercent == null ? 'Unknown' : `${node.hostCpuPercent.toFixed(1)}%`}</dd></dl><h2>Block history</h2><div className="history-list">{history.length === 0 ? <p className="muted">No block history observed yet.</p> : history.map((block) => <article className="node-card" key={`${block.height ?? 'gap'}-${block.observedAt ?? ''}`}><strong>{block.height == null ? `History gap ${block.gapFromHeight ?? '?'}–${block.gapToHeight ?? '?'}` : `Height ${block.height}`}</strong><span> · {block.height == null ? 'No sample available; chart disconnected' : `${block.blockTimeMs == null ? 'time unknown' : new Date(block.blockTimeMs).toISOString()} · ${block.transactionCount == null ? 'transactions unknown' : `${block.transactionCount} transactions`}`}</span><p className="muted">{block.height == null ? `${block.gapKind ?? 'gap'}: ${block.gapReason ?? 'bounded recovery did not recover this interval'}` : `Freshness: ${block.freshness ?? 'unknown'} · Coinbase: ${block.coinbase ?? 'unknown'} · Seal signer: ${block.sealSignerMatch ?? 'unknown'} · Protocol proposer: ${block.protocolProposer ?? 'unknown'}`}</p></article>)}</div></section>
}

function NodeCard({ node }: { node: PublicNode }) {
  return <article className="node-card"><h2><Link to={`/nodes/${node.nodeId}`}>{node.displayName ?? node.nodeId}</Link></h2><p><span className={`status status-${node.health}`}>{node.health}</span> {node.healthReason}</p><p className="muted">RPC: {node.rpcState} · Sync: {node.syncState} · Consensus: {node.consensusState}</p></article>
}
