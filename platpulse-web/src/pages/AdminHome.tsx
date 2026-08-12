import { useEffect, useState } from 'react'
import { updateNodeVisibility, fetchAdminDiagnostics } from '../api/admin'
import type { AgentDiagnostic } from '../api/generated'
import { useAuth } from '../auth/AuthContext'

export default function AdminHome() {
  const { status } = useAuth()
  const [nodeId, setNodeId] = useState('')
  const [visibility, setVisibility] = useState<'private' | 'public'>('public')
  const [diagnostics, setDiagnostics] = useState<AgentDiagnostic[]>([])
  const [message, setMessage] = useState<string | null>(null)
  useEffect(() => {
    if (status.state === 'authenticated') fetchAdminDiagnostics().then(setDiagnostics).catch(() => setDiagnostics([]))
  }, [status.state])
  async function submit(event: React.FormEvent) {
    event.preventDefault()
    if (status.state !== 'authenticated') return
    try {
      const result = await updateNodeVisibility(nodeId, visibility, status.csrfToken)
      setMessage(`${result.nodeId} is now ${result.visibility}.`)
    } catch (error) { setMessage(error instanceof Error ? error.message : 'Unable to update visibility') }
  }
  return <section className="page"><h1>Admin diagnostics</h1><p>Owner-only visibility workflow. Endpoint and credential details remain hidden from Home.</p><form onSubmit={submit} className="visibility-form"><div className="field"><label htmlFor="node-id">Node ID</label><input id="node-id" value={nodeId} onChange={(event) => setNodeId(event.target.value)} required /></div><div className="field"><label htmlFor="visibility">Visibility</label><select id="visibility" value={visibility} onChange={(event) => setVisibility(event.target.value as 'private' | 'public')}><option value="public">Public</option><option value="private">Private</option></select></div><button className="primary-action" type="submit">Update visibility</button></form>{message && <p role="status">{message}</p>}<h2>Agents and Nodes</h2>{diagnostics.map((agent) => <section key={agent.agent_id}>
<h3>{agent.agent_id}</h3>
<p className="muted">Liveness: <strong>{agent.liveness}</strong> · Last receipt: {agent.last_received_at ?? 'never'}</p>
<p className="muted">Clock: <strong>{agent.clock_status}</strong>{agent.clock_skew_ms == null ? ' · skew unknown' : ` · skew ${agent.clock_skew_ms} ms`}</p>
{agent.nodes.map((node) => <article className="node-card" key={node.node_id}><h4>{node.display_name ?? node.node_id}</h4><p>{node.health}: {node.health_reason}</p><dl className="detail-list"><dt>RPC</dt><dd>{node.rpc?.state ?? 'unknown'}{node.rpc?.client_version ? ` (${node.rpc.client_version})` : ''}</dd><dt>Sync</dt><dd>{node.sync?.state ?? 'unknown'} — current {node.sync?.current_block ?? 'unknown'} / highest {node.sync?.highest_block ?? 'unknown'}</dd><dt>Consensus</dt><dd>{node.consensus?.state ?? 'unknown'} — epoch {node.consensus?.epoch ?? 'unknown'}, view {node.consensus?.view_number ?? 'unknown'}</dd></dl></article>)}</section>)}</section>
}
