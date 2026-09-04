import { useEffect, useRef, useState, type KeyboardEvent } from 'react'
import { Link } from 'react-router'
import './AdminOverview.css'
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
  const refreshAll = async () => {
    await Promise.allSettled([overview.refetch(), diagnostics.refetch(), nodes.refetch()])
  }

  return (
    <section className="page admin-overview">
      <OverviewHeader snapshot={snapshot} query={overview} refreshing={overview.isFetching || diagnostics.isFetching || nodes.isFetching} onRefresh={refreshAll} />
      <AttentionPanel query={overview} />
      {snapshot && <SummaryCards summary={snapshot.summary} />}
      <NodePanel nodeQuery={nodes} diagnosticsQuery={diagnostics} />
      <AgentPanel query={diagnostics} nodeQuery={nodes} />
      {snapshot &&
        nodes.data &&
        diagnostics.data &&
        !overview.isError &&
        !nodes.isError &&
        !diagnostics.isError &&
        !overview.isFetching &&
        !nodes.isFetching &&
        !diagnostics.isFetching &&
        isAuthoritativelyEmpty(snapshot, nodes.data, diagnostics.data) && <SetupGuide />}
    </section>
  )
}

type OverviewQuery = ReturnType<typeof useAdminOverview>

function OverviewHeader({
  snapshot,
  query,
  refreshing,
  onRefresh,
}: {
  snapshot: AdminOverview | undefined
  query: OverviewQuery
  refreshing: boolean
  onRefresh: () => Promise<void>
}) {
  return (
    <header className="admin-overview-header">
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
              onClick={() => void onRefresh()}
              disabled={refreshing}
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

function isAuthoritativelyEmpty(snapshot: AdminOverview, nodes: AdminNodeListItem[], agents: AgentDiagnostic[]): boolean {
  return snapshot.summary.agents.total === 0 &&
    snapshot.summary.nodes.total === 0 &&
    snapshot.summary.networks.total === 0 &&
    nodes.length === 0 &&
    agents.length === 0
}

function SetupGuide() {
  return (
    <aside className="panel setup-guide" aria-labelledby="setup-guide-title">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Next steps</span>
          <h2 id="setup-guide-title">Set up your first observation</h2>
        </div>
      </div>
      <p>There are no Agents, Nodes, or Networks yet. Complete these steps to begin receiving authoritative observations.</p>
      <ol>
        <li><Link to="/admin/networks">Register the expected Network identity</Link>.</li>
        <li>Provision and start an Agent.</li>
        <li><Link to="/admin/settings">Configure the Agent's local Node Inventory</Link>.</li>
        <li>Wait for the first accepted Agent Report.</li>
      </ol>
    </aside>
  )
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
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const previousCount = useRef<number | null>(null)
  const attentionLength = data?.attention.length
  useEffect(() => {
    if (attentionLength === undefined) return
    if (previousCount.current !== null && previousCount.current !== attentionLength) setAnnouncement(`Attention queue updated: ${attentionLength} items need attention.`)
    previousCount.current = attentionLength
  }, [attentionLength])
  const groups = groupAttention(data?.attention ?? [])
  const showAll = expanded.has("__all__")
  const visibleGroups = showAll ? groups : groups.slice(0, 6)
  const hiddenCount = Math.max(0, groups.length - 6)
  const criticalCount = (data?.attention ?? []).filter((item) => item.severity === "critical").length
  return (
    <article className="panel overview-panel attention-panel">
      <div className="panel-heading"><div><span className="eyebrow">01 · Attention</span><h2>Attention queue</h2></div>{data && <span className="panel-count">{data.attention.length}</span>}</div>
      <p className="sr-only" role="status">{announcement}</p>
      {!data && query.isPending && <p className="panel-state" role="status"><StatusBadge status="Starting" tone="neutral" /> Checking the Server for attention…</p>}
      {!data && query.isError && <p className="panel-state" role="alert"><StatusBadge status="Error" tone="error" /> {query.error instanceof Error ? query.error.message : "Unable to load attention"} <button type="button" className="text-action" onClick={() => void query.refetch()}>Try again</button></p>}
      {data && query.isRefetchError && <p className="panel-state" role="alert"><StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last successful attention queue. <button type="button" className="text-action" onClick={() => void query.refetch()}>Try again</button></p>}
      {data && data.attention.length === 0 && <p className="panel-state"><StatusBadge status="Empty" tone="ok" /> No attention items. Nothing needs an Owner right now.</p>}
      {data && data.attention.length > 0 && <><p className="attention-counts">{data.attention.length} items across {groups.length} subjects · {criticalCount} Critical</p><ul className="attention-list">{visibleGroups.map((group) => <AttentionGroup key={group.key} group={group} expanded={expanded.has(group.key)} onToggle={() => setExpanded((current) => { const next = new Set(current); if (next.has(group.key)) { next.delete(group.key) } else { next.add(group.key) } return next })} />)}</ul>{hiddenCount > 0 && <button type="button" className="quiet-button" onClick={() => setExpanded((current) => { const next = new Set(current); if (next.has("__all__")) { next.delete("__all__") } else { next.add("__all__") } return next })}>{showAll ? "Collapse" : `Show ${hiddenCount} more`}</button>}</>}
    </article>
  )
}

type AttentionGroupData = { key: string; subjectKind: string; subjectId: string; label: string; items: AttentionItem[] }
function groupAttention(items: AttentionItem[]): AttentionGroupData[] {
  const groups = new Map<string, AttentionGroupData>()
  for (const item of items) { const key = `${item.subject_kind.length}:${item.subject_kind}${item.subject_id}`; const group = groups.get(key); if (group) group.items.push(item); else groups.set(key, { key, subjectKind: item.subject_kind, subjectId: item.subject_id, label: item.subject_label, items: [item] }) }
  return [...groups.values()]
}

function safeAttentionRoute(group: AttentionGroupData): string | null {
  if (!group.subjectId || group.subjectId.includes("/") || group.subjectId.includes("\\")) return null
  if (group.subjectKind === "agent") return "/admin/agents/" + encodeURIComponent(group.subjectId)
  if (group.subjectKind === "node") return "/admin/nodes/" + encodeURIComponent(group.subjectId)
  if (group.subjectKind === "network") return "/admin/networks/" + encodeURIComponent(group.subjectId)
  if (group.subjectKind === "settings" && group.subjectId === "settings") return "/admin/settings"
  return null
}

function AttentionGroup({ group, expanded, onToggle }: { group: AttentionGroupData; expanded: boolean; onToggle: () => void }) {
  const primary = group.items[0]
  const known = primary.severity === "critical" || primary.severity === "warning"
  const severity = known ? primary.severity : "unknown"
  const route = safeAttentionRoute(group)
  return <li className={`attention-item attention-group ${severity}`}><StatusBadge status={severity === "critical" ? "Critical" : severity === "warning" ? "Warning" : "Unknown"} tone={severity === "critical" ? "error" : severity === "warning" ? "warning" : "neutral"} /><div className="attention-body"><p><strong>{route ? <Link to={route}>{group.label}</Link> : group.label}</strong> — {primary.message}</p><p className="muted">{primary.kind} · <SnapshotTime timestamp={primary.observed_at} /></p>{group.items.length > 1 && <><button type="button" className="quiet-button" aria-expanded={expanded} onClick={onToggle}>{expanded ? "Hide additional issues" : `Show ${group.items.length - 1} additional issues`}</button>{expanded && <ul>{group.items.slice(1).map((item) => <li key={item.id}>{item.severity === "critical" ? "Critical" : item.severity === "warning" ? "Warning" : "Unknown"} · {item.message} · {item.kind}</li>)}</ul>}</>}</div></li>
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
  const [expanded, setExpanded] = useState<string | null>(null)
  const nodes = nodeQuery.data ?? []
  const activeNodes = nodes.filter((node) => node.lifecycle === 'active')
  const prioritizedNodes = prioritizeActiveNodes(activeNodes)
  const visibleNodes = prioritizedNodes.slice(0, 10)

  useEffect(() => {
    if (expanded !== null && !visibleNodes.some((node) => node.node_id === expanded)) {
      setExpanded(null)
    }
  }, [expanded, visibleNodes])

  const diagnosticsByNode = new Map(
    (diagnosticsQuery.data ?? [])
      .flatMap((agent) => agent.nodes)
      .map((node) => [node.node_id, node] as const),
  )

  const toggle = (nodeId: string) => {
    setExpanded((current) => (current === nodeId ? null : nodeId))
  }

  return (
    <article className="panel overview-panel node-panel">
      <div className="panel-heading">
        <h2>Node Health Summary</h2>
        {activeNodes.length > 0 && <span className="panel-count">{activeNodes.length}</span>}
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
          <button type="button" className="text-action" onClick={() => void nodeQuery.refetch()}>Try again</button>
        </p>
      )}
      {nodeQuery.data && activeNodes.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No Nodes observed yet.
        </p>
      )}
      {nodeQuery.data && activeNodes.length > 0 && (
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
              {visibleNodes.map((node) => (
                <NodeRows
                  key={node.node_id}
                  node={node}
                  diagnostic={diagnosticsByNode.get(node.node_id)}
                  diagnosticsQuery={diagnosticsQuery}
                  expanded={expanded === node.node_id}
                  onToggle={() => toggle(node.node_id)}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
      {nodeQuery.data && activeNodes.length > 0 && (
        <div className="panel-heading">
          <span className="muted">Showing {visibleNodes.length} of {activeNodes.length} Active Nodes</span>
          <Link className="text-action" to="/admin/nodes">View all Nodes</Link>
        </div>
      )}
    </article>
  )
}

function prioritizeActiveNodes(nodes: AdminNodeListItem[]): AdminNodeListItem[] {
  const healthRank: Record<string, number> = { unhealthy: 0, unknown: 1, healthy: 3 }
  const rank = (node: AdminNodeListItem) => {
    if (node.health === 'unhealthy') return 0
    if (node.health === 'unknown') return 1
    if (node.freshness === 'stale') return 2
    return healthRank[node.health] ?? 1
  }
  const compareText = (left: string, right: string) => {
    const a = left.toLocaleLowerCase('en-US')
    const b = right.toLocaleLowerCase('en-US')
    return a < b ? -1 : a > b ? 1 : 0
  }
  return [...nodes].sort((left, right) =>
    rank(left) - rank(right) ||
    compareText(left.network_display_name, right.network_display_name) ||
    compareText(left.network_key, right.network_key) ||
    compareText(left.display_name ?? '', right.display_name ?? '') ||
    compareText(left.node_id, right.node_id),
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
  const toggleRef = useRef<HTMLButtonElement>(null)
  const nodeLabel = node.display_name ?? node.node_id
  const collapseOnEscape = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== 'Escape' || !expanded) return
    event.preventDefault()
    onToggle()
    toggleRef.current?.focus()
  }
  const detailId = `node-detail-${node.node_id}`
  const healthTone =
    node.health === 'healthy' ? 'ok' : node.health === 'unhealthy' ? 'error' : 'neutral'
  const freshnessTone =
    node.freshness === 'current' ? 'ok' : node.freshness === 'stale' ? 'warning' : 'neutral'
  return (
    <>
      <tr onKeyDown={collapseOnEscape}>
        <th scope="row" data-label="Node">
          <button
            ref={toggleRef}
            type="button"
            className="node-toggle"
            aria-expanded={expanded}
            aria-controls={detailId}
            onClick={onToggle}
          >
            <span aria-hidden="true">{expanded ? '▾' : '▸'}</span> {nodeLabel}
          </button>
          <small className="muted" title={node.node_id}>
            Node ID · {node.node_id.slice(0, 8)}…
          </small>
          <button
            type="button"
            className="diagnostics-toggle text-action"
            aria-expanded={expanded}
            aria-controls={detailId}
            onClick={onToggle}
          >
            {expanded ? 'Hide diagnostics' : 'Show diagnostics'}
          </button>
          <Link className="text-action" to={`/admin/nodes/${encodeURIComponent(node.node_id)}`}>View Node</Link>
        </th>
        <td data-label="Network">
          {node.network_display_name}
          <small className="muted">{node.network_key}</small>
        </td>
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
          <small className="muted">{syncSummary(diagnostic)}</small>
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
                    observedAt={diagnostic.rpc?.observed_at}
                    attemptedAt={diagnostic.rpc?.attempted_at}
                    receivedAt={diagnostic.rpc?.received_at}
                    detail={
                      diagnostic.rpc?.client_version
                        ? `${diagnostic.rpc.state === 'error' ? 'last-good ' : ''}${diagnostic.rpc.client_version} · ${diagnostic.rpc.namespaces.length} namespaces`
                        : undefined
                    }
                  />
                  <ComponentRow
                    label="Sync"
                    state={diagnostic.sync?.state}
                    errorMessage={diagnostic.sync?.error_message}
                    observedAt={diagnostic.sync?.observed_at}
                    attemptedAt={diagnostic.sync?.attempted_at}
                    receivedAt={diagnostic.sync?.received_at}
                    detail={
                      diagnostic.sync?.current_block != null
                        ? `${diagnostic.sync.state === 'error' ? 'last-good ' : ''}head ${diagnostic.sync.current_block}${
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
                    observedAt={diagnostic.consensus?.observed_at}
                    attemptedAt={diagnostic.consensus?.attempted_at}
                    receivedAt={diagnostic.consensus?.received_at}
                    detail={
                      diagnostic.consensus?.highest_commit_block != null
                        ? `${diagnostic.consensus.state === 'error' ? 'last-good ' : ''}commit ${diagnostic.consensus.highest_commit_block}`
                        : undefined
                    }
                  />
                  <ComponentRow
                    label="Peers"
                    state={diagnostic.peers?.state}
                    errorMessage={diagnostic.peers?.error_message}
                    observedAt={diagnostic.peers?.observed_at}
                    attemptedAt={diagnostic.peers?.attempted_at}
                    receivedAt={diagnostic.peers?.received_at}
                    detail={
                      diagnostic.peers?.peer_count != null
                        ? `${diagnostic.peers.state === 'error' ? 'last-good ' : ''}${diagnostic.peers.peer_count} peers · ${diagnostic.peers.freshness}`
                        : undefined
                    }
                  />
                  <ComponentRow
                    label="Process"
                    state={diagnostic.process?.state}
                    errorMessage={diagnostic.process?.error_message}
                    observedAt={diagnostic.process?.observed_at}
                    attemptedAt={diagnostic.process?.attempted_at}
                    receivedAt={diagnostic.process?.received_at}
                    detail={
                      diagnostic.process?.pid != null
                        ? `${diagnostic.process.state === 'error' ? 'last-good ' : ''}pid ${diagnostic.process.pid}`
                        : undefined
                    }
                  />
                  <ComponentRow
                    label="Node Data"
                    state={diagnostic.data_directory?.state}
                    errorMessage={diagnostic.data_directory?.error_message}
                    observedAt={diagnostic.data_directory?.observed_at}
                    attemptedAt={diagnostic.data_directory?.attempted_at}
                    receivedAt={diagnostic.data_directory?.received_at}
                    detail={
                      diagnostic.data_directory?.size_bytes != null
                        ? `${diagnostic.data_directory.state === 'error' ? 'last-good ' : ''}${formatBytes(diagnostic.data_directory.size_bytes)}`
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
  attemptedAt,
  receivedAt,
  detail,
}: {
  label: string
  state: string | null | undefined
  errorMessage?: string | null
  observedAt?: string | null
  attemptedAt?: string | null
  receivedAt?: string | null
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
        <small className="muted">
          · {state === 'error'
            ? observedAt
              ? `Last good · ${formatObservedAt(observedAt)}`
              : 'Never observed'
            : formatObservedAt(observedAt)}
          {state === 'error' && (attemptedAt ?? receivedAt)
            ? ` · Attempted ${formatObservedAt(attemptedAt ?? receivedAt)}`
            : ''}
        </small>
      </dd>
    </div>
  )
}

function AgentPanel({ query, nodeQuery }: { query: DiagnosticsQuery; nodeQuery: NodesQuery }) {
  const agents = query.data ?? []
  const visibleAgents = prioritizeAgents(agents).slice(0, 6)
  const nodesByAgent = new Map<string, AdminNodeListItem[]>()
  for (const node of nodeQuery.data ?? []) {
    const existing = nodesByAgent.get(node.agent_id) ?? []
    existing.push(node)
    nodesByAgent.set(node.agent_id, existing)
  }
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
          <button type="button" className="text-action" onClick={() => void query.refetch()}>Try again</button>
        </p>
      )}
      {query.data && agents.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No Agents enrolled yet.
        </p>
      )}
      {query.data && agents.length > 0 && (
        <div className="agent-grid">
          {visibleAgents.map((agent) => (
            <AgentCard key={agent.agent_id} agent={agent} nodes={nodesByAgent.get(agent.agent_id)} nodeQuery={nodeQuery} />
          ))}
        </div>
      )}
      {query.data && agents.length > 0 && (
        <div className="panel-heading">
          <span className="muted">Showing {visibleAgents.length} of {agents.length} Agents</span>
          <Link className="text-action" to="/admin/agents">View all Agents</Link>
        </div>
      )}
    </article>
  )
}

export function prioritizeAgents(agents: AgentDiagnostic[]): AgentDiagnostic[] {
  const rank = (agent: AgentDiagnostic) => {
    const host = agent.host
    const critical = hasSpoolRisk(host) || agent.security_event_count > 0
    if (critical) return 0
    if (agent.liveness === 'offline') return 1
    if (agent.liveness !== 'online') return 2
    return 3
  }
  return [...agents].sort((left, right) => rank(left) - rank(right) || left.agent_id.localeCompare(right.agent_id, 'en-US'))
}

function AgentCard({ agent, nodes, nodeQuery }: { agent: AgentDiagnostic; nodes?: AdminNodeListItem[]; nodeQuery: NodesQuery }) {
  const liveness = livenessLabel(agent.liveness)
  const livenessTone = agent.liveness === 'online' ? 'ok' : agent.liveness === 'offline' ? 'error' : 'neutral'
  const host = agent.host
  const spoolRisk = hasSpoolRisk(host)
  const spoolText = formatSpoolSummary(host)
  const memory = host?.memory_used_bytes != null && host.memory_total_bytes != null ? `${formatBytes(host.memory_used_bytes)} / ${formatBytes(host.memory_total_bytes)} memory` : 'Unknown memory'
  return (
    <article className={`agent-card${spoolRisk || agent.security_event_count > 0 ? ' agent-card-critical' : ''}`}>
      <div className="agent-card-heading"><h3>{agent.agent_id}</h3><Link className="text-action" to={`/admin/agents/${encodeURIComponent(agent.agent_id)}`}>View Agent</Link></div>
      <p><StatusBadge status={liveness} tone={livenessTone} /> {spoolRisk && <StatusBadge status="Critical" tone="error" />}</p>
      <dl className="detail-list">
        <div><dt>Last report</dt><dd>{agent.last_report_sequence == null ? 'Unknown' : `#${agent.last_report_sequence} · ${formatObservedAt(agent.last_received_at)}`}</dd></div>
        <div><dt>Host</dt><dd>{host ? `${host.cpu_percent != null ? `${host.cpu_percent}% CPU` : 'Unknown CPU'} · ${memory}` : 'Unknown CPU · Unknown memory'}</dd></div>
        <div><dt>Durable Spool</dt><dd className={spoolRisk || host?.spool_store_error ? 'diagnostic-critical' : ''}>{spoolText}</dd></div>
        <div><dt>Clock</dt><dd>{clockStatusLabel(agent.clock_status)}{agent.clock_skew_ms != null ? ` · ${agent.clock_skew_ms} ms skew` : ''}</dd></div>
        <div><dt>Evidence</dt><dd>{agent.sequence_gap_count} report gap{agent.sequence_gap_count === 1 ? '' : 's'} · {agent.security_event_count} security event{agent.security_event_count === 1 ? '' : 's'}</dd></div>
        <div><dt>Nodes</dt><dd><NodeContext nodes={nodes} query={nodeQuery} /></dd></div>
      </dl>
    </article>
  )
}

function NodeContext({ nodes, query }: { nodes?: AdminNodeListItem[]; query: NodesQuery }) {
  if (!query.data) {
    if (query.isPending) return <span className="muted">Loading Node context…</span>
    if (query.isError) return <span className="diagnostic-critical">Node context unavailable; recover in Nodes</span>
    return <span className="muted">Node context unavailable</span>
  }
  if (!nodes || nodes.length === 0) return <span>No Nodes observed yet.</span>
  const active = nodes.filter((node) => node.lifecycle === 'active').length
  const unhealthy = nodes.filter((node) => node.health === 'unhealthy').length
  const unknown = nodes.filter((node) => node.health === 'unknown').length
  return (
    <div className="agent-node-context">
      <span className="muted">{nodes.length} Node{nodes.length === 1 ? '' : 's'} · {active} active · {unhealthy} unhealthy · {unknown} unknown</span>
      <ul>
        {nodes.map((node) => (
          <li key={node.node_id}>
            <span>{node.display_name ?? node.node_id} <small>({node.node_id})</small></span>
            <span>{node.lifecycle} · {nodeHealthLabel(node.health)} · {freshnessLabel(node.freshness)}</span>
          </li>
        ))}
      </ul>
    </div>
  )
}

function nodeHealthLabel(health: string): string {
  if (health === 'healthy') return 'Healthy'
  if (health === 'unhealthy') return 'Unhealthy'
  return 'Unknown'
}

function hasSpoolRisk(host: AgentDiagnostic['host']): boolean {
  return host?.spool_store_fatal === true ||
    host?.spool_dropped_sequence_from != null ||
    host?.spool_dropped_sequence_to != null ||
    host?.spool_dropped_height_from != null ||
    host?.spool_dropped_height_to != null ||
    host?.spool_report_too_large === true
}

function formatSpoolSummary(host: AgentDiagnostic['host']): string {
  if (!host) return 'Unknown'
  const observed = [
    host.spool_capacity_bytes, host.spool_dropped_sequence_from, host.spool_dropped_sequence_to,
    host.spool_dropped_height_from, host.spool_dropped_height_to, host.spool_in_flight,
    host.spool_queued_bytes, host.spool_queued_reports, host.spool_report_too_large,
    host.spool_store_error, host.spool_store_fatal, host.spool_pending_history_gaps,
  ].some((value) => value != null)
  if (!observed) return 'Unknown'
  const parts = [
    host.spool_queued_reports != null ? `${host.spool_queued_reports} queued` : null,
    host.spool_queued_bytes != null ? `${formatBytes(host.spool_queued_bytes)} queued bytes` : null,
    host.spool_capacity_bytes != null ? `capacity ${formatBytes(host.spool_capacity_bytes)}` : null,
    host.spool_store_fatal ? 'fatal storage' : null,
    host.spool_store_error ? `store error: ${host.spool_store_error}` : null,
    hasSpoolRisk(host) && !host.spool_store_fatal ? 'discarded reports' : null,
    host.spool_report_too_large ? 'report too large' : null,
    host.spool_pending_history_gaps != null ? `${host.spool_pending_history_gaps} history gaps` : null,
  ].filter((part): part is string => part !== null)
  return parts.length > 0 ? parts.join(' · ') : 'Normal'
}

function syncSummary(diagnostic: NodeDiagnostic | undefined): string {
  const state = componentStateLabel(diagnostic?.sync?.state)
  const current = diagnostic?.sync?.current_block ?? diagnostic?.current_head
  const highest = diagnostic?.sync?.highest_block
  if (current == null && highest == null) return `Sync ${state}`
  if (highest == null || current == null) return `Sync ${state} · ${current ?? highest}`
  const delta = highest - current
  const lag = delta === 0 ? '' : ` (${delta} behind)`
  return `Sync ${state} · ${current} / ${highest}${lag}`
}

function clockStatusLabel(status: string | null | undefined): string {
  if (!status || status === 'unknown') return 'Unknown'
  return status
}
