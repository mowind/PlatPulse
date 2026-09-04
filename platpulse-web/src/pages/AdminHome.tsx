import { useEffect, useRef, useState, type KeyboardEvent } from 'react'
import { Link } from 'react-router'
import './AdminOverviewPrototype.css'
import {
  useAdminDiagnostics,
  useAdminNodes,
  useAdminOverview,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { formatBytes } from '../formatBytes'
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
  AdminNodeListItem,
  NodeDiagnostic,
} from '../api/generated'

/**
 * PAGE-ADMIN-OVERVIEW (webui.md §8.4): Server-owned attention queue, Node
 * Health Summary/freshness, and Agent inventory/diagnostics as applicable.
 * Each panel collects, refreshes, and fails independently; last-good
 * values stay visible with explicit Error and freshness context. The
 * Server computes health, freshness, and attention; the browser only
 * formats them. This page carries no per-Node visibility/publication or
 * Geo database content: Site Access Mode in Settings remains the single
 * site-level Public/Private authority (issues #93 and #111).
 */
export default function AdminHome() {
  const { generation } = useAuth()
  const overview = useAdminOverview(generation)
  const diagnostics = useAdminDiagnostics(generation)
  const nodes = useAdminNodes(generation)
  const snapshot = overview.data

  return (
    <section className="page admin-overview prototype-a">
      <OverviewHeader snapshot={snapshot} query={overview} />
      <AttentionPanel query={overview} />
      {snapshot && <SummaryCards summary={snapshot.summary} />}
      <NodePanel nodeQuery={nodes} diagnosticsQuery={diagnostics} />
      <AgentPanel query={diagnostics} />
    </section>
  )
}

type OverviewQuery = ReturnType<typeof useAdminOverview>

function OverviewHeader({
  snapshot,
  query,
}: {
  snapshot: AdminOverview | undefined
  query: OverviewQuery
}) {
  return (
    <header className="prototype-header">
      <div>
        <span className="eyebrow">Owner triage</span>
        <h1>Overview</h1>
        <p>A compact read on what needs intervention across your PlatON estate.</p>
      </div>
      <div className="header-status">
        <span className="live-dot" aria-hidden="true" />
        {snapshot ? (
          <>
            Last good snapshot · <SnapshotTime timestamp={snapshot.generated_at} />
            <button
              type="button"
              className="refresh-button"
              onClick={() => void query.refetch()}
              disabled={query.isFetching}
            >
              {query.isFetching ? 'Refreshing…' : 'Refresh'}
            </button>
          </>
        ) : query.isError ? (
          'Snapshot unavailable'
        ) : (
          'Snapshot loading'
        )}
      </div>
    </header>
  )
}

function SnapshotTime({ timestamp }: { timestamp: string }) {
  const date = new Date(timestamp)
  if (Number.isNaN(date.getTime())) return <span>Unknown time</span>
  const relative = formatRelativeTime(date, new Date())
  const absolute = new Intl.DateTimeFormat('en-GB', {
    dateStyle: 'medium',
    timeStyle: 'long',
    timeZone: 'UTC',
  }).format(date)
  return (
    <time dateTime={timestamp} title={absolute} aria-label={`${relative}; ${absolute}`}>
      {relative}
    </time>
  )
}

function formatRelativeTime(value: Date, now: Date): string {
  const seconds = Math.round((value.getTime() - now.getTime()) / 1000)
  const absoluteSeconds = Math.abs(seconds)
  const formatter = new Intl.RelativeTimeFormat('en', { numeric: 'auto' })
  if (absoluteSeconds < 60) return formatter.format(seconds, 'second')
  if (absoluteSeconds < 3_600) return formatter.format(Math.round(seconds / 60), 'minute')
  if (absoluteSeconds < 86_400) return formatter.format(Math.round(seconds / 3_600), 'hour')
  return formatter.format(Math.round(seconds / 86_400), 'day')
}

function SummaryCards({ summary }: { summary: AdminOverview['summary'] }) {
  const cards = [
    {
      label: 'Agents',
      value: summary.agents.total,
      legend: `${summary.agents.online} online · ${summary.agents.offline} offline · ${summary.agents.unknown} unknown`,
      href: '/admin/agents',
      accent: 'violet',
    },
    {
      label: 'Active Nodes',
      value: summary.nodes.active,
      legend: `${summary.nodes.healthy} healthy · ${summary.nodes.unhealthy} unhealthy · ${summary.nodes.unknown} unknown`,
      href: '/admin/nodes?lifecycle=active',
      accent: 'green',
    },
    {
      label: 'Retired Nodes',
      value: summary.nodes.retired,
      legend: 'Excluded from live health buckets',
      href: '/admin/nodes?lifecycle=retired',
      accent: 'slate',
    },
    {
      label: 'Networks',
      value: summary.networks.total,
      legend: `${summary.networks.with_identity_mismatch} with Network Identity Mismatch`,
      href: '/admin/networks',
      accent: 'red',
    },
  ]
  return (
    <nav className="summary-cards" aria-label="Overview summaries">
      {cards.map((card) => (
        <Link className={`summary-card accent-${card.accent}`} to={card.href} key={card.label}>
          <span className="eyebrow">{card.label}</span>
          <strong>{card.value}</strong>
          <span>{card.legend}</span>
          <span className="card-arrow" aria-hidden="true">↗</span>
        </Link>
      ))}
    </nav>
  )
}

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
        `Attention queue updated: ${attentionLength} item${attentionLength === 1 ? '' : 's'} need${attentionLength === 1 ? '' : 's'} attention.`,
      )
    }
    previousCount.current = attentionLength
  }, [attentionLength])

  return (
    <article className="panel overview-panel a-attention">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">01 · Attention</span>
          <h2>Attention queue</h2>
        </div>
        {data && <span className="panel-count">{data.attention.length}</span>}
      </div>
      <p className="sr-only" role="status">
        {announcement}
      </p>
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
type NodesQuery = ReturnType<typeof useAdminNodes>

function NodePanel({
  nodeQuery,
  diagnosticsQuery,
}: {
  nodeQuery: NodesQuery
  diagnosticsQuery: DiagnosticsQuery
}) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const nodes = nodeQuery.data ?? []
  const diagnosticsByNode = new Map(
    (diagnosticsQuery.data ?? [])
      .flatMap((agent) => agent.nodes)
      .map((node) => [node.node_id, node] as const),
  )

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
        <h2>Node Health Summary</h2>
        {nodes.length > 0 && <span className="panel-count">{nodes.length}</span>}
      </div>
      {!nodeQuery.data && nodeQuery.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Node Health
          Summary…
        </p>
      )}
      {!nodeQuery.data && nodeQuery.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {nodeQuery.error instanceof Error ? nodeQuery.error.message : 'Unable to load Nodes'}
          <button type="button" className="text-action" onClick={() => void nodeQuery.refetch()}>
            Try again
          </button>
        </p>
      )}
      {nodeQuery.data && nodeQuery.isRefetchError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
          successful Node values.
        </p>
      )}
      {nodeQuery.data && nodes.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No Nodes observed yet.
        </p>
      )}
      {nodeQuery.data && nodes.length > 0 && (
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
                  diagnostic={diagnosticsByNode.get(node.node_id)}
                  diagnosticsQuery={diagnosticsQuery}
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
  diagnostic,
  diagnosticsQuery,
  expanded,
  onToggle,
}: {
  node: AdminNodeListItem
  diagnostic: NodeDiagnostic | undefined
  diagnosticsQuery: DiagnosticsQuery
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
          <small className="muted">Server-owned freshness</small>
        </td>
        <td data-label="Head / Sync">
          {node.current_head ?? 'Unknown'}
          <small className="muted">
            Sync {componentStateLabel(diagnostic?.sync?.state)}
          </small>
        </td>
        <td data-label="Resync">
          {node.resync_state}
          {diagnostic?.resync_progress ? (
            <small className="muted">{diagnostic.resync_progress}</small>
          ) : null}
        </td>
      </tr>
      {expanded && (
        <tr className="node-detail-row">
          <td colSpan={6} id={detailId} onKeyDown={collapseOnEscape}>
            <div className="node-detail">
              <button type="button" className="text-action" onClick={onToggle}>
                Collapse details <span aria-hidden="true">▴</span>
              </button>
              {!diagnostic && diagnosticsQuery.isPending && (
                <p className="panel-state" role="status">
                  <StatusBadge status="Starting" tone="neutral" /> Loading Node diagnostics…
                </p>
              )}
              {!diagnostic && diagnosticsQuery.isError && (
                <p className="panel-state" role="alert">
                  <StatusBadge status="Error" tone="error" /> Node diagnostics are unavailable;
                  the summary above remains available.
                  <button type="button" className="text-action" onClick={() => void diagnosticsQuery.refetch()}>
                    Try again
                  </button>
                </p>
              )}
              {diagnostic && (
                <dl className="detail-list">
                  <ComponentRow
                    label="RPC"
                    state={diagnostic.rpc?.state}
                    errorMessage={diagnostic.rpc?.error_message}
                    observedAt={diagnostic.rpc?.received_at}
                    detail={
                      diagnostic.rpc?.client_version
                        ? `${diagnostic.rpc.client_version} · ${diagnostic.rpc.namespaces.length} namespaces`
                        : undefined
                    }
                  />
                  <ComponentRow
                    label="Sync"
                    state={diagnostic.sync?.state}
                    errorMessage={diagnostic.sync?.error_message}
                    observedAt={diagnostic.sync?.received_at}
                    detail={
                      diagnostic.sync?.current_block != null
                        ? `last-good head ${diagnostic.sync.current_block}${
                            diagnostic.sync.highest_block != null
                              ? ` · highest ${diagnostic.sync.highest_block}`
                              : ''
                          }`
                        : undefined
                    }
                  />
                  <ComponentRow
                    label="Consensus"
                    state={diagnostic.consensus?.state}
                    errorMessage={diagnostic.consensus?.error_message}
                    observedAt={diagnostic.consensus?.received_at}
                    detail={
                      diagnostic.consensus?.highest_commit_block != null
                        ? `last-good commit ${diagnostic.consensus.highest_commit_block}`
                        : undefined
                    }
                  />
                  <ComponentRow
                    label="Peers"
                    state={diagnostic.peers?.state}
                    errorMessage={diagnostic.peers?.error_message}
                    observedAt={diagnostic.peers?.received_at}
                    detail={
                      diagnostic.peers?.peer_count != null
                        ? `${diagnostic.peers.peer_count} peers · ${diagnostic.peers.freshness}`
                        : undefined
                    }
                  />
                  <ComponentRow
                    label="Process"
                    state={diagnostic.process?.state}
                    errorMessage={diagnostic.process?.error_message}
                    observedAt={diagnostic.process?.received_at}
                    detail={
                      diagnostic.process?.pid != null ? `pid ${diagnostic.process.pid}` : undefined
                    }
                  />
                  <ComponentRow
                    label="Node Data"
                    state={diagnostic.data_directory?.state}
                    errorMessage={diagnostic.data_directory?.error_message}
                    observedAt={diagnostic.data_directory?.received_at}
                    detail={
                      diagnostic.data_directory?.size_bytes != null
                        ? formatBytes(diagnostic.data_directory.size_bytes)
                        : undefined
                    }
                  />
                </dl>
              )}
              {!diagnostic && diagnosticsQuery.data && (
                <p className="panel-state">
                  <StatusBadge status="Unknown" tone="neutral" /> No current Agent diagnostic
                  is available for this Node; the Server-owned summary remains authoritative.
                </p>
              )}
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
        <small className="muted"> · {formatObservedAt(observedAt)}</small>
      </dd>
    </div>
  )
}

function AgentPanel({ query }: { query: DiagnosticsQuery }) {
  const agents = query.data ?? []
  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Agent inventory</h2>
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
