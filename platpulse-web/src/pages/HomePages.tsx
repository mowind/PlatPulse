import { useEffect, useState } from 'react'
import { Link, useParams } from 'react-router'
import { fetchNetwork, fetchNode, usePublicRealtime } from '../api/public'
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
  useEffect(() => { fetchNode(nodeId).then(setNode).catch((e: Error) => setError(e.message)) }, [nodeId, reload])

  if (error) return <section className="page"><p role="alert" className="form-error">{error}</p><Link to="/">Back to Home</Link></section>
  if (!node) return <section className="page"><p role="status">Loading Node…</p></section>
  return <section className="page"><p><Link to={`/networks/${node.networkKey}`}>← {node.networkKey}</Link></p><h1>{node.displayName ?? 'Node detail'}</h1><dl className="detail-list"><dt>Node ID</dt><dd>{node.nodeId}</dd><dt>Health</dt><dd><span className={`status status-${node.health}`}>{node.health}</span> — {node.healthReason}</dd><dt>RPC state</dt><dd>{node.rpcState}</dd><dt>Sync state</dt><dd>{node.syncState}</dd><dt>Consensus state</dt><dd>{node.consensusState}</dd><dt>Freshness</dt><dd>{node.freshness ?? 'Never observed'}</dd><dt>Host CPU</dt><dd>{node.hostCpuPercent == null ? 'Unknown' : `${node.hostCpuPercent.toFixed(1)}%`}</dd></dl></section>
}

function NodeCard({ node }: { node: PublicNode }) {
  return <article className="node-card"><h2><Link to={`/nodes/${node.nodeId}`}>{node.displayName ?? node.nodeId}</Link></h2><p><span className={`status status-${node.health}`}>{node.health}</span> {node.healthReason}</p><p className="muted">RPC: {node.rpcState} · Sync: {node.syncState} · Consensus: {node.consensusState}</p></article>
}
