import { useEffect, useState } from 'react'
import { updateNodeVisibility, fetchAdminDiagnostics, fetchAdminNodeHistory, useAdminRealtime } from '../api/admin'
import type { AgentDiagnostic } from '../api/generated'
import { useAuth } from '../auth/AuthContext'

export default function AdminHome() {
  const { status } = useAuth()
  const [nodeId, setNodeId] = useState('')
  const [visibility, setVisibility] = useState<'private' | 'public'>('public')
  const [diagnostics, setDiagnostics] = useState<AgentDiagnostic[]>([])
  const [history, setHistory] = useState<Record<string, Awaited<ReturnType<typeof fetchAdminNodeHistory>>>>({})
  const [message, setMessage] = useState<string | null>(null)
  const [reload, setReload] = useState(0)
  const realtime = useAdminRealtime(() => setReload((value) => value + 1))
  useEffect(() => {
    if (status.state !== 'authenticated') return
    fetchAdminDiagnostics().then(async (items) => {
      setDiagnostics(items)
      const entries = await Promise.all(items.flatMap((agent) => agent.nodes.map(async (node) => [node.node_id, await fetchAdminNodeHistory(node.node_id).catch(() => ({ items: [], availability: null, aggregateSupported: false, rawRetentionDays: 7 }))] as const)))
      setHistory(Object.fromEntries(entries))
    }).catch(() => setDiagnostics([]))
  }, [status.state, reload])
  async function submit(event: React.FormEvent) {
    event.preventDefault()
    if (status.state !== 'authenticated') return
    try {
      const result = await updateNodeVisibility(nodeId, visibility, status.csrfToken)
      setMessage(`${result.nodeId} is now ${result.visibility}.`)
    } catch (error) { setMessage(error instanceof Error ? error.message : 'Unable to update visibility') }
  }
  return <section className="page"><h1>Admin diagnostics</h1><p>Owner-only visibility workflow. Endpoint and credential details remain hidden from Home.</p><p>Realtime updates: {realtime}</p><form onSubmit={submit} className="visibility-form"><div className="field"><label htmlFor="node-id">Node ID</label><input id="node-id" value={nodeId} onChange={(event) => setNodeId(event.target.value)} required /></div><div className="field"><label htmlFor="visibility">Visibility</label><select id="visibility" value={visibility} onChange={(event) => setVisibility(event.target.value as 'private' | 'public')}><option value="public">Public</option><option value="private">Private</option></select></div><button className="primary-action" type="submit">Update visibility</button></form>{message && <p role="status">{message}</p>}<h2>Agents and Nodes</h2>{diagnostics.map((agent) => <section key={agent.agent_id}><h3>{agent.agent_id}</h3><p className="muted">Boot: {agent.boot_status} {agent.active_boot_id ?? 'unknown'}{agent.previous_boot_id ? ` · previous ${agent.previous_boot_id}` : ''} · last sequence {agent.last_report_sequence ?? 'unknown'} · report gaps {agent.sequence_gap_count} · security events {agent.security_event_count}</p><p className="muted">Shutdown: {agent.shutdown_state} · started {agent.shutdown_started_at ?? 'unknown'} · deadline {agent.shutdown_deadline_at ?? 'unknown'}{agent.shutdown_unresolved_range ? ` · unresolved ${agent.shutdown_unresolved_range[0]}–${agent.shutdown_unresolved_range[1]}` : ''}{agent.shutdown_last_error ? ` · failure: ${agent.shutdown_last_error}` : ''}</p>{agent.nodes.map((node) => <article className="node-card" key={node.node_id}><h4>{node.display_name ?? node.node_id}</h4><p>{node.health}: {node.health_reason}</p><dl className="detail-list"><dt>RPC</dt><dd>{node.rpc?.state ?? 'unknown'}</dd><dt>Sync</dt><dd>{node.sync?.state ?? 'unknown'} — current {node.sync?.current_block ?? 'unknown'}</dd><dt>Current head</dt><dd>{node.current_head ?? 'unknown'}</dd><dt>Historical high-water mark</dt><dd>{node.historical_high_watermark ?? 'unknown'}</dd><dt>Resync</dt><dd>{node.resync_state}{node.resync_progress ? ` — ${node.resync_progress}` : ''}</dd><dt>Network reference</dt><dd>{node.network_reference_head ?? 'unknown'} ({node.network_reference_confidence})</dd></dl><h5>Block history</h5>{history[node.node_id]?.availability === 'unavailable' && <p className="muted">Older raw history is unavailable; Phase 1 has no aggregate fallback.</p>}{(history[node.node_id]?.items ?? []).slice(0, 10).map((block) => <p className="muted" key={`${block.height}-${block.observedAt}`}>{block.divergenceKind ? `Chain divergence · height ${block.gapFromHeight ?? '?'} · retained ${block.divergenceRetainedHash ?? 'unknown'} vs observed ${block.divergenceObservedHash ?? 'unknown'} at ${block.divergenceObservedAt ?? 'unknown'} · ${block.divergenceReason ?? 'bounded identity mismatch'}` : block.height == null ? `History gap ${block.gapFromHeight ?? '?'}–${block.gapToHeight ?? '?'} · ${block.gapKind ?? 'gap'}: ${block.gapReason ?? 'bounded recovery did not recover this interval'}` : `Height ${block.height} · ${block.transactionCount == null ? 'transactions unknown' : `${block.transactionCount} transactions`} · ${block.freshness ?? 'freshness unknown'}`}</p>)}</article>)}</section>)}</section>
}
