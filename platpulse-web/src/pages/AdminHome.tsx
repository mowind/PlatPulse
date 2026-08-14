import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from 'react'
import {
  updateNodeVisibility,
  useAdminDiagnostics,
  useAdminOverview,
  type RealtimeState,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { useAdminRealtimeContext } from '../layouts/AdminLayout'
import {
  StatusBadge,
  componentStateLabel,
  formatObservedAt,
  freshnessLabel,
  livenessLabel,
} from '../components/StatusBadge'
import type {
  AdminOverview,
  AgentDiagnostic,
  AttentionItem,
  NodeDiagnostic,
} from '../api/generated'

/**
 * PAGE-ADMIN-OVERVIEW (design §8.1): Server-owned attention queue, Node
 * Health Summary, freshness, and next actions. Each panel collects,
 * refreshes, and fails independently; last-good values stay visible with
 * explicit Error and freshness context. The Server computes health,
 * freshness, and attention; the browser only formats them.
 */
export default function AdminHome() {
  const { status, generation } = useAuth()
  const { realtime } = useAdminRealtimeContext()
  const overview = useAdminOverview(generation)
  const diagnostics = useAdminDiagnostics(generation)

  return (
    <section className="page">
      <h1>Overview</h1>
      <p className="muted">
        Server-owned attention queue and Node Health Summary. The Server
        decides health and attention; this page only presents them.
      </p>
      <RealtimeNotice realtime={realtime} />
      <AttentionPanel query={overview} />
      <NodePanel query={diagnostics} />
      <AgentPanel query={diagnostics} />
      <OperationsPanel
        csrfToken={status.state === 'authenticated' ? status.csrfToken : ''}
      />
    </section>
  )
}

function RealtimeNotice({ realtime }: { realtime: RealtimeState }) {
  const label = !realtime.online
    ? 'You are offline'
    : realtime.status === 'connected'
      ? 'Connected'
      : realtime.status === 'connecting'
        ? 'Starting'
        : 'Live updates paused'
  const tone = !realtime.online || realtime.status === 'disconnected' ? 'warning' : 'ok'
  return (
    <p className="realtime-notice" role="status">
      <StatusBadge status={label} tone={tone} />
      <span className="muted"> Server updates arrive as invalidations; REST data stays authoritative.</span>
    </p>
  )
}

type OverviewQuery = ReturnType<typeof useAdminOverview>

function AttentionPanel({ query }: { query: OverviewQuery }) {
  const data = query.data
  const [announcement, setAnnouncement] = useState('')
  const previousCount = useRef<number | null>(null)
  const attentionLength = data?.attention.length

  useEffect(() => {
    if (attentionLength === undefined) return
    if (
      previousCount.current !== null &&
      previousCount.current !== attentionLength
    ) {
      setAnnouncement(
        `Attention queue updated: ${attentionLength} item${attentionLength === 1 ? '' : 's'} need${attentionLength === 1 ? 's' : ''} attention.`,
      )
    }
    previousCount.current = attentionLength
  }, [attentionLength])

  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Attention queue</h2>
        {data && <span className="panel-count">{data.attention.length}</span>}
      </div>
      <p className="sr-only" role="status">
        {announcement}
      </p>
      {data?.summary && <SummaryStrip summary={data.summary} />}
      {!data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Checking the Server for
          attention…
        </p>
      )}
      {!data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load attention'}
          <button type="button" className="text-action" onClick={() => void query.refetch()}>
            Try again
          </button>
        </p>
      )}
      {data && query.isRefetchError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
          successful attention queue.
        </p>
      )}
      {data && data.attention.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No attention items. Nothing needs an
          Owner right now.
        </p>
      )}
      {data && data.attention.length > 0 && (
        <ul className="attention-list">
          {data.attention.map((item) => (
            <AttentionRow key={item.id} item={item} />
          ))}
        </ul>
      )}
    </article>
  )
}

function SummaryStrip({ summary }: { summary: AdminOverview['summary'] }) {
  return (
    <dl className="summary-strip">
      <div>
        <dt>Agents</dt>
        <dd>
          {summary.agents.online} online · {summary.agents.offline} offline ·{' '}
          {summary.agents.unknown} unknown of {summary.agents.total}
        </dd>
      </div>
      <div>
        <dt>Nodes</dt>
        <dd>
          {summary.nodes.healthy} healthy · {summary.nodes.unhealthy} unhealthy ·{' '}
          {summary.nodes.unknown} unknown of {summary.nodes.total}
          {summary.nodes.retired > 0
            ? ` · ${summary.nodes.retired} retired`
            : ''}
        </dd>
      </div>
      <div>
        <dt>Published</dt>
        <dd>
          {summary.nodes.published} of {summary.nodes.total} Nodes are visible on Home
        </dd>
      </div>
    </dl>
  )
}

function AttentionRow({ item }: { item: AttentionItem }) {
  const tone = item.severity === 'critical' ? 'error' : 'warning'
  const severityLabel = item.severity === 'critical' ? 'Critical' : 'Warning'
  return (
    <li className="attention-item">
      <StatusBadge status={severityLabel} tone={tone} />
      <div className="attention-body">
        <p>
          <strong>{item.subject_label}</strong> — {item.message}
        </p>
        <p className="muted">
          {item.kind} · {formatObservedAt(item.observed_at)}
        </p>
      </div>
    </li>
  )
}

type DiagnosticsQuery = ReturnType<typeof useAdminDiagnostics>

function NodePanel({ query }: { query: DiagnosticsQuery }) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const agents = query.data ?? []
  const nodes = agents.flatMap((agent) => agent.nodes)

  const toggle = (nodeId: string) => {
    setExpanded((current) => {
      const next = new Set(current)
      if (next.has(nodeId)) {
        next.delete(nodeId)
      } else {
        next.add(nodeId)
      }
      return next
    })
  }

  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Node health</h2>
        {nodes.length > 0 && <span className="panel-count">{nodes.length}</span>}
      </div>
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Node Health
          Summary…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load Nodes'}
          <button type="button" className="text-action" onClick={() => void query.refetch()}>
            Try again
          </button>
        </p>
      )}
      {query.data && query.isRefetchError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
          successful Node values.
        </p>
      )}
      {query.data && nodes.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No Nodes observed yet.
        </p>
      )}
      {query.data && nodes.length > 0 && (
        <div className="table-wrap">
          <table className="node-table">
            <caption className="sr-only">PlatON Node health, freshness, and sync</caption>
            <thead>
              <tr>
                <th scope="col">Node</th>
                <th scope="col">Network</th>
                <th scope="col">Health</th>
                <th scope="col">Freshness</th>
                <th scope="col">Head / Sync</th>
                <th scope="col">Resync</th>
              </tr>
            </thead>
            <tbody>
              {nodes.map((node) => (
                <NodeRows
                  key={node.node_id}
                  node={node}
                  expanded={expanded.has(node.node_id)}
                  onToggle={() => toggle(node.node_id)}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </article>
  )
}

function NodeRows({
  node,
  expanded,
  onToggle,
}: {
  node: NodeDiagnostic
  expanded: boolean
  onToggle: () => void
}) {
  const collapseOnEscape = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === 'Escape' && expanded) onToggle()
  }
  const detailId = `node-detail-${node.node_id}`
  const healthTone =
    node.health === 'healthy' ? 'ok' : node.health === 'unhealthy' ? 'error' : 'neutral'
  const freshnessTone =
    node.freshness === 'current' ? 'ok' : node.freshness === 'stale' ? 'warning' : 'neutral'
  return (
    <>
      <tr>
        <th scope="row" data-label="Node">
          <button
            type="button"
            className="node-toggle"
            aria-expanded={expanded}
            aria-controls={detailId}
            onClick={onToggle}
            onKeyDown={collapseOnEscape}
          >
            <span aria-hidden="true">{expanded ? '▾' : '▸'}</span> {node.display_name ?? node.node_id}
          </button>
          <small className="muted" title={node.node_id}>
            Node ID · {node.node_id.slice(0, 8)}…
          </small>
        </th>
        <td data-label="Network">{node.network_key}</td>
        <td data-label="Health">
          <StatusBadge status={node.health} tone={healthTone} />
          <span className="health-reason">{node.health_reason}</span>
        </td>
        <td data-label="Freshness">
          <StatusBadge status={freshnessLabel(node.freshness)} tone={freshnessTone} />
          <small className="muted">{formatObservedAt(latestReceivedAt(node))}</small>
        </td>
        <td data-label="Head / Sync">
          {node.current_head ?? 'Unknown'}
          <small className="muted">Sync {componentStateLabel(node.sync?.state)}</small>
        </td>
        <td data-label="Resync">
          {node.resync_state}
          {node.resync_progress ? <small className="muted">{node.resync_progress}</small> : null}
        </td>
      </tr>
      {expanded && (
        <tr className="node-detail-row">
          <td colSpan={6} id={detailId} onKeyDown={collapseOnEscape}>
            <div className="node-detail">
              <button type="button" className="text-action" onClick={onToggle}>
                Collapse details <span aria-hidden="true">▴</span>
              </button>
              <dl className="detail-list">
                <ComponentRow
                  label="RPC"
                  state={node.rpc?.state}
                  errorMessage={node.rpc?.error_message}
                  observedAt={node.rpc?.received_at}
                  detail={
                    node.rpc?.client_version
                      ? `${node.rpc.client_version} · ${node.rpc.namespaces.length} namespaces`
                      : undefined
                  }
                />
                <ComponentRow
                  label="Sync"
                  state={node.sync?.state}
                  errorMessage={node.sync?.error_message}
                  observedAt={node.sync?.received_at}
                  detail={
                    node.sync?.current_block != null
                      ? `last-good head ${node.sync.current_block}${
                          node.sync.highest_block != null
                            ? ` · highest ${node.sync.highest_block}`
                            : ''
                        }`
                      : undefined
                  }
                />
                <ComponentRow
                  label="Consensus"
                  state={node.consensus?.state}
                  errorMessage={node.consensus?.error_message}
                  observedAt={node.consensus?.received_at}
                  detail={
                    node.consensus?.highest_commit_block != null
                      ? `last-good commit ${node.consensus.highest_commit_block}`
                      : undefined
                  }
                />
                <ComponentRow
                  label="Process"
                  state={node.process?.state}
                  errorMessage={node.process?.error_message}
                  observedAt={node.process?.received_at}
                  detail={
                    node.process?.pid != null ? `pid ${node.process.pid}` : undefined
                  }
                />
              </dl>
            </div>
          </td>
        </tr>
      )}
    </>
  )
}

function ComponentRow({
  label,
  state,
  errorMessage,
  observedAt,
  detail,
}: {
  label: string
  state: string | null | undefined
  errorMessage?: string | null
  observedAt?: string | null
  detail?: string
}) {
  const tone =
    state === 'error' ? 'error' : state === 'ok' ? 'ok' : state === 'starting' ? 'neutral' : 'neutral'
  return (
    <div className="component-row">
      <dt>{label}</dt>
      <dd>
        <StatusBadge status={componentStateLabel(state)} tone={tone} />
        {state === 'error' && errorMessage && (
          <span className="component-error"> {errorMessage}</span>
        )}
        {detail && <span className="muted"> {detail}</span>}
        {observedAt && <small className="muted"> · {formatObservedAt(observedAt)}</small>}
      </dd>
    </div>
  )
}

function latestReceivedAt(node: NodeDiagnostic): string | null | undefined {
  const candidates = [node.rpc?.received_at, node.sync?.received_at, node.consensus?.received_at]
    .filter((value): value is string => typeof value === 'string')
    .sort()
  return candidates.at(-1)
}

function AgentPanel({ query }: { query: DiagnosticsQuery }) {
  const agents = query.data ?? []
  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Agents</h2>
        {agents.length > 0 && <span className="panel-count">{agents.length}</span>}
      </div>
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading Agent state…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load Agents'}
          <button type="button" className="text-action" onClick={() => void query.refetch()}>
            Try again
          </button>
        </p>
      )}
      {query.data && query.isRefetchError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
          successful Agent values.
        </p>
      )}
      {query.data && agents.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No Agents enrolled yet.
        </p>
      )}
      {query.data && agents.length > 0 && (
        <div className="agent-grid">
          {agents.map((agent) => (
            <AgentCard key={agent.agent_id} agent={agent} />
          ))}
        </div>
      )}
    </article>
  )
}

function AgentCard({ agent }: { agent: AgentDiagnostic }) {
  const liveness = livenessLabel(agent.liveness)
  const livenessTone =
    agent.liveness === 'online' ? 'ok' : agent.liveness === 'offline' ? 'error' : 'neutral'
  const spoolFatal = agent.host?.spool_store_fatal === true
  return (
    <article className="agent-card">
      <h3>{agent.agent_id}</h3>
      <p>
        <StatusBadge status={liveness} tone={livenessTone} />
        {spoolFatal && <StatusBadge status="Error" tone="error" />}
      </p>
      <dl className="detail-list">
        <div>
          <dt>Last report</dt>
          <dd>
            {agent.last_report_sequence == null
              ? 'None yet'
              : `#${agent.last_report_sequence} · ${formatObservedAt(agent.last_received_at)}`}
          </dd>
        </div>
        <div>
          <dt>Boot</dt>
          <dd>
            {agent.boot_status} {agent.active_boot_id ? `· ${agent.active_boot_id.slice(0, 8)}…` : ''}
          </dd>
        </div>
        <div>
          <dt>Shutdown</dt>
          <dd>{agent.shutdown_state}</dd>
        </div>
        <div>
          <dt>Diagnostics</dt>
          <dd>
            {agent.sequence_gap_count} sequence gap{agent.sequence_gap_count === 1 ? '' : 's'} ·{' '}
            {agent.security_event_count} security event
            {agent.security_event_count === 1 ? '' : 's'}
            {spoolFatal ? ' · spool store fatal' : ''}
          </dd>
        </div>
      </dl>
    </article>
  )
}

function OperationsPanel({ csrfToken }: { csrfToken: string }) {
  const [nodeId, setNodeId] = useState('')
  const [visibility, setVisibility] = useState<'private' | 'public'>('public')
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setMessage(null)
    setError(null)
    try {
      const result = await updateNodeVisibility(nodeId, visibility, csrfToken)
      setMessage(`${result.nodeId} is now ${result.visibility}.`)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to update visibility')
    }
  }

  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Operations</h2>
      </div>
      <p className="muted">
        Publish or retract a Node from the Home projection. Endpoint and
        credential details remain hidden from Home.
      </p>
      <form onSubmit={submit} className="visibility-form">
        <div className="field">
          <label htmlFor="node-id">Node ID</label>
          <input
            id="node-id"
            value={nodeId}
            onChange={(event) => setNodeId(event.target.value)}
            required
          />
        </div>
        <div className="field">
          <label htmlFor="visibility">Visibility</label>
          <select
            id="visibility"
            value={visibility}
            onChange={(event) => setVisibility(event.target.value as 'private' | 'public')}
          >
            <option value="public">Public</option>
            <option value="private">Private</option>
          </select>
        </div>
        <button className="primary-action" type="submit">
          Update visibility
        </button>
      </form>
      {message && (
        <p className="form-success" role="status">
          {message}
        </p>
      )}
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
    </article>
  )
}
