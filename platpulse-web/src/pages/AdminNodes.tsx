import { useMemo, useState, type FormEvent, type KeyboardEvent } from 'react'
import { Link, useParams, useSearchParams } from 'react-router'
import {
  AdminApiError,
  updateNodeMetadata,
  useAdminNodeDetail,
  useAdminNodes,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { formatBytes } from '../formatBytes'
import {
  StatusBadge,
  componentStateLabel,
  formatObservedAt,
  freshnessLabel,
} from '../components/StatusBadge'
import type {
  AdminNodeDetail as AdminNodeDetailDto,
  AdminNodeListItem,
  NodeIdentityStatus,
} from '../api/generated'

/**
 * PAGE-ADMIN-NODES and PAGE-ADMIN-NODE-DETAIL (design §4.3, §8.2; webui.md
 * §8.2): Owner-only Node inventory and administrative diagnostics. Every row
 * is one Node — block, transaction, consensus, peer, and error state never
 * merge across Nodes. Server-owned metadata (display name, lifecycle
 * guidance) stays distinct from Agent-observed identity and endpoint
 * configuration; lifecycle follows the latest Agent Inventory, the Server
 * never remotely changes a Node, and the Admin detail never reproduces
 * Home's full observation view.
 */

function shortId(value: string): string {
  return value.length > 12 ? `${value.slice(0, 8)}…${value.slice(-4)}` : value
}

function healthTone(health: string): 'ok' | 'error' | 'neutral' {
  return health === 'healthy' ? 'ok' : health === 'unhealthy' ? 'error' : 'neutral'
}

/** Server freshness dimension → badge tone (current/stale/unknown). */
function freshnessTone(freshness: string): 'ok' | 'warning' | 'neutral' {
  return freshness === 'current' ? 'ok' : freshness === 'stale' ? 'warning' : 'neutral'
}

function identityBadge(identity: NodeIdentityStatus): {
  label: string
  tone: 'ok' | 'warning' | 'error' | 'neutral'
} {
  switch (identity.state) {
    case 'matched':
      return { label: 'Matched', tone: 'ok' }
    case 'mismatched':
      return { label: 'Mismatched', tone: 'error' }
    default:
      return { label: 'Unknown', tone: 'neutral' }
  }
}

function visibilityBadge(visibility: string): { label: string; tone: 'ok' | 'neutral' } {
  return visibility === 'public'
    ? { label: 'Public', tone: 'ok' }
    : { label: 'Private', tone: 'neutral' }
}

/** Lifecycle follows the latest Agent Inventory with a fixed vocabulary:
 * only `active` and `retired` are known states — anything else renders as
 * Unknown rather than as a definite lifecycle (preserve-last-good). */
function lifecycleLabel(lifecycle: string): { label: string; tone: 'ok' | 'neutral' } {
  if (lifecycle === 'active') return { label: 'Active', tone: 'ok' }
  if (lifecycle === 'retired') return { label: 'Retired', tone: 'neutral' }
  return { label: 'Unknown', tone: 'neutral' }
}

/** URL-state filters (design §10.1: back/forward preserves them). */
type NodeFilters = {
  visibility: string
  lifecycle: string
  network: string
  health: string
}

function readFilters(search: URLSearchParams): NodeFilters {
  return {
    visibility: search.get('visibility') ?? 'all',
    lifecycle: search.get('lifecycle') ?? 'all',
    network: search.get('network') ?? 'all',
    health: search.get('health') ?? 'all',
  }
}

/** PAGE-ADMIN-NODES: per-Node inventory with filters, sorting, freshness,
 * health summary, and identity disposition. */
export default function AdminNodesList() {
  const { generation } = useAuth()
  const query = useAdminNodes(generation)
  const [search, setSearch] = useSearchParams()
  const filters = readFilters(search)
  const [sort, setSort] = useState<'name' | 'network' | 'health' | 'freshness'>('health')

  const [expanded, setExpanded] = useState<Set<string>>(new Set())

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

  const nodes = useMemo(() => {
    const all = query.data ?? []
    const filtered = all.filter((node) => {
      if (filters.visibility !== 'all' && node.visibility !== filters.visibility) return false
      if (filters.lifecycle !== 'all' && node.lifecycle !== filters.lifecycle) return false
      if (filters.network !== 'all' && node.network_key !== filters.network) return false
      if (filters.health !== 'all' && node.health !== filters.health) return false
      return true
    })
    const rank = (node: AdminNodeListItem): number =>
      node.health === 'healthy' ? 0 : node.health === 'unhealthy' ? 2 : 1
    const freshnessRank = (node: AdminNodeListItem): number =>
      node.freshness === 'current' ? 0 : node.freshness === 'stale' ? 1 : 2
    const byName = (a: AdminNodeListItem, b: AdminNodeListItem): number =>
      (a.display_name ?? a.node_id).localeCompare(b.display_name ?? b.node_id)
    const byNetwork = (a: AdminNodeListItem, b: AdminNodeListItem): number =>
      a.network_key.localeCompare(b.network_key) || byName(a, b)
    const sorted = [...filtered]
    switch (sort) {
      case 'name':
        sorted.sort(byName)
        break
      case 'network':
        sorted.sort(byNetwork)
        break
      case 'freshness':
        sorted.sort((a, b) => freshnessRank(a) - freshnessRank(b) || byName(a, b))
        break
      default:
        sorted.sort((a, b) => rank(a) - rank(b) || byName(a, b))
    }
    return sorted
  }, [query.data, filters, sort])

  const setFilter = (key: keyof NodeFilters, value: string) => {
    const next = new URLSearchParams(search)
    if (value === 'all') next.delete(key)
    else next.set(key, value)
    setSearch(next, { replace: false })
  }

  const networks = useMemo(() => {
    const keys = new Set((query.data ?? []).map((node) => node.network_key))
    return [...keys].sort()
  }, [query.data])

  return (
    <section className="page">
      <h1>Nodes</h1>
      <p className="muted">
        Each row is one Node. Health, freshness, identity, visibility, and lifecycle are
        separate Server-owned dimensions; lifecycle follows the latest Agent Inventory.
      </p>
      <NodeFiltersBar
        filters={filters}
        networks={networks}
        onChange={setFilter}
        sort={sort}
        onSort={setSort}
      />
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Node inventory…
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
          <StatusBadge status="Empty" tone="ok" /> No Nodes match these filters.
        </p>
      )}
      {query.data && nodes.length > 0 && (
        <div className="table-wrap">
          <table className="node-table">
            <caption className="sr-only">
              Node inventory: identity, health, freshness, visibility, and lifecycle
            </caption>
            <thead>
              <tr>
                <th scope="col">
                  <SortButton label="Node" column="name" sort={sort} onSort={setSort} />
                </th>
                <th scope="col">
                  <SortButton label="Network" column="network" sort={sort} onSort={setSort} />
                </th>
                <th scope="col">
                  <SortButton label="Health" column="health" sort={sort} onSort={setSort} />
                </th>
                <th scope="col">
                  <SortButton label="Freshness" column="freshness" sort={sort} onSort={setSort} />
                </th>
                <th scope="col">Identity</th>
                <th scope="col">Visibility</th>
                <th scope="col">Head / Sync</th>
                <th scope="col">Lifecycle</th>
              </tr>
            </thead>
            <tbody>
              {nodes.map((node) => (
                <NodeListRow
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
    </section>
  )
}

function SortButton({
  label,
  column,
  sort,
  onSort,
}: {
  label: string
  column: 'name' | 'network' | 'health' | 'freshness'
  sort: string
  onSort: (column: 'name' | 'network' | 'health' | 'freshness') => void
}) {
  const active = sort === column
  return (
    <button
      type="button"
      className="sort-button"
      aria-sort={active ? (column === 'name' || column === 'network' ? 'ascending' : 'ascending') : 'none'}
      onClick={() => onSort(column)}
    >
      {label} <span aria-hidden="true">{active ? '↑' : '⇅'}</span>
    </button>
  )
}

function NodeFiltersBar({
  filters,
  networks,
  onChange,
  sort,
  onSort,
}: {
  filters: NodeFilters
  networks: string[]
  onChange: (key: keyof NodeFilters, value: string) => void
  sort: string
  onSort: (column: 'name' | 'network' | 'health' | 'freshness') => void
}) {
  return (
    <div className="filter-bar" role="group" aria-label="Node filters">
      <label>
        Sort by
        <select value={sort} onChange={(event) => onSort(event.target.value as 'name' | 'network' | 'health' | 'freshness')}>
          <option value="health">Health</option>
          <option value="name">Node name</option>
          <option value="network">Network</option>
          <option value="freshness">Freshness</option>
        </select>
      </label>
      <label>
        Network
        <select
          value={filters.network}
          onChange={(event) => onChange('network', event.target.value)}
        >
          <option value="all">All</option>
          {networks.map((network) => (
            <option key={network} value={network}>
              {network}
            </option>
          ))}
        </select>
      </label>
      <label>
        Visibility
        <select
          value={filters.visibility}
          onChange={(event) => onChange('visibility', event.target.value)}
        >
          <option value="all">All</option>
          <option value="public">Public</option>
          <option value="private">Private</option>
        </select>
      </label>
      <label>
        Lifecycle
        <select
          value={filters.lifecycle}
          onChange={(event) => onChange('lifecycle', event.target.value)}
        >
          <option value="all">All</option>
          <option value="active">Active</option>
          <option value="retired">Retired</option>
        </select>
      </label>
      <label>
        Health
        <select value={filters.health} onChange={(event) => onChange('health', event.target.value)}>
          <option value="all">All</option>
          <option value="healthy">Healthy</option>
          <option value="unhealthy">Unhealthy</option>
          <option value="unknown">Unknown</option>
        </select>
      </label>
    </div>
  )
}

function NodeListRow({
  node,
  expanded,
  onToggle,
}: {
  node: AdminNodeListItem
  expanded: boolean
  onToggle: () => void
}) {
  const identity = identityBadge(node.identity)
  const visibility = visibilityBadge(node.visibility)
  const health = healthTone(node.health)
  const freshness = freshnessTone(node.freshness)
  const detailId = `node-inventory-detail-${node.node_id}`
  const collapseOnEscape = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === 'Escape' && expanded) onToggle()
  }
  return (
    <>
      <tr>
        <th scope="row" data-label="Node">
          <button
            type="button"
            className="node-toggle"
            aria-label={`${expanded ? 'Collapse' : 'Expand'} ${node.display_name ?? node.node_id}`}
            aria-expanded={expanded}
            aria-controls={detailId}
            onClick={onToggle}
            onKeyDown={collapseOnEscape}
          >
            <span aria-hidden="true">{expanded ? '▾' : '▸'}</span>
          </button>{' '}
          <Link className="agent-link" to={`/admin/nodes/${node.node_id}`}>
            {node.display_name ?? node.node_id}
          </Link>
          <small className="muted" title={node.node_id}>
            Node ID · {shortId(node.node_id)} · {node.rpc_endpoint}
          </small>
        </th>
        <td data-label="Network">
          <Link className="text-action" to={`/admin/networks/${node.network_key}`}>
            {node.network_display_name}
          </Link>
        </td>
        <td data-label="Health">
          <StatusBadge status={node.health} tone={health} />
          <small className="muted">{node.health_reason}</small>
        </td>
        <td data-label="Freshness">
          <StatusBadge status={freshnessLabel(node.freshness)} tone={freshness} />
          <small className="muted">
            {node.freshness === 'current' ? 'Reporting now' : 'Check age on detail'}
          </small>
        </td>
        <td data-label="Identity">
          <StatusBadge status={identity.label} tone={identity.tone} />
          {node.identity.mismatched_fields.length > 0 && (
            <small className="muted">{node.identity.mismatched_fields.join(', ')}</small>
          )}
        </td>
        <td data-label="Visibility">
          <StatusBadge status={visibility.label} tone={visibility.tone} />
        </td>
        <td data-label="Head / Sync">
          {node.current_head ?? 'Unknown'}
          <small className="muted">{node.resync_state}</small>
        </td>
        <td data-label="Lifecycle">
          <span>{lifecycleLabel(node.lifecycle).label}</span>
          <small className="muted">rev {node.inventory_revision}</small>
        </td>
      </tr>
      {expanded && (
        <tr className="node-detail-row">
          <td colSpan={8} id={detailId} onKeyDown={collapseOnEscape}>
            <div className="node-detail">
              <button type="button" className="text-action" onClick={onToggle}>
                Collapse details <span aria-hidden="true">▴</span>
              </button>
              <dl className="detail-list">
                <div>
                  <dt>Lifecycle guidance</dt>
                  <dd>{node.lifecycle_guidance}</dd>
                </div>
                <div>
                  <dt>Identity disposition</dt>
                  <dd>
                    {identity.label}
                    {node.identity.mismatched_fields.length > 0
                      ? ` — contradicts the Registry on ${node.identity.mismatched_fields.join(', ')}`
                      : ''}
                  </dd>
                </div>
                <div>
                  <dt>Health reason</dt>
                  <dd>{node.health_reason}</dd>
                </div>
                <div>
                  <dt>Resync state</dt>
                  <dd>{node.resync_state}</dd>
                </div>
              </dl>
            </div>
          </td>
        </tr>
      )}
    </>
  )
}

/** PAGE-ADMIN-NODE-DETAIL (webui.md §8.2): administrative diagnostics only —
 * Server-owned display name with its audited mutation flow, Node
 * Inventory/lifecycle, freshness summary, safe Agent/Network context, and
 * redacted RPC Endpoint diagnostics. Home-style observation cards, Block
 * History, Peer History, Node Transfer, per-Node Visibility, and
 * remote-operation controls are not part of this page. */
export function AdminNodeDetail() {
  const { nodeId = '' } = useParams()
  const { generation, status } = useAuth()
  const query = useAdminNodeDetail(generation, nodeId)
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const notFound = query.isError && query.error instanceof AdminApiError && query.error.code === 'not_found'

  if (notFound) {
    return (
      <section className="page">
        <h1>Node unavailable</h1>
        <p>This Node is no longer available.</p>
      </section>
    )
  }
  return (
    <section className="page">
      <h1>
        {query.data?.display_name ?? shortId(nodeId)}
        <span className="heading-muted">{nodeId}</span>
      </h1>
      <p className="muted">
        <Link className="text-action" to="/admin/nodes">
          All Nodes
        </Link>{' '}
        · Server-owned metadata stays distinct from Agent-observed identity and configuration.
        Lifecycle is Node Inventory state and is never confused with Agent liveness or Node
        health.
      </p>
      {!query.data && (
        <>
          {query.isPending && (
            <p className="panel-state" role="status">
              <StatusBadge status="Starting" tone="neutral" /> Loading Node state…
            </p>
          )}
          {query.isError && (
            <p className="panel-state" role="alert">
              <StatusBadge status="Error" tone="error" />{' '}
              {query.error instanceof Error ? query.error.message : 'Unable to load the Node'}
              <button type="button" className="text-action" onClick={() => void query.refetch()}>
                Try again
              </button>
            </p>
          )}
        </>
      )}
      {query.data && (
        <>
          {query.isRefetchError && (
            <p className="panel-state" role="alert">
              <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
              successful Node values.
            </p>
          )}
          <MetadataPanel node={query.data} csrfToken={csrfToken} />
          <LifecyclePanel node={query.data} />
          <HealthPanel node={query.data} />
          <IdentityPanel node={query.data} />
          <RpcDiagnosticsPanel node={query.data} />
        </>
      )}
    </section>
  )
}

function MetadataPanel({
  node,
  csrfToken,
}: {
  node: AdminNodeDetailDto
  csrfToken: string
}) {
  const [editing, setEditing] = useState(false)
  const [confirming, setConfirming] = useState(false)
  const [displayName, setDisplayName] = useState(node.display_name ?? '')
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setMessage(null)
    setError(null)
    setConfirming(true)
  }

  async function confirm() {
    setConfirming(false)
    try {
      const result = await updateNodeMetadata(node.node_id, displayName.trim(), csrfToken)
      setMessage(`Display name is now "${result.displayName}".`)
      setEditing(false)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to update the display name')
    }
  }

  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Server-owned metadata</h2>
      </div>
      <dl className="detail-list">
        <div>
          <dt>Display name</dt>
          <dd>
            {editing ? (
              <form className="inline-form" onSubmit={submit}>
                <label className="sr-only" htmlFor={`display-name-${node.node_id}`}>
                  Display name
                </label>
                <input
                  id={`display-name-${node.node_id}`}
                  value={displayName}
                  onChange={(event) => {
                    setDisplayName(event.target.value)
                    setConfirming(false)
                  }}
                  maxLength={128}
                />
                {confirming ? (
                  <>
                    <span className="muted">
                      Rename this Node in the Server-owned metadata? The Agent Inventory,
                      endpoint, and Node ID are not touched.
                    </span>
                    <button className="primary-action" type="button" onClick={() => void confirm()}>
                      Confirm rename
                    </button>
                    <button
                      className="secondary-action"
                      type="button"
                      onClick={() => setConfirming(false)}
                    >
                      Keep editing
                    </button>
                  </>
                ) : (
                  <button className="primary-action" type="submit">
                    Save
                  </button>
                )}
                <button
                  className="secondary-action"
                  type="button"
                  onClick={() => {
                    setEditing(false)
                    setConfirming(false)
                    setDisplayName(node.display_name ?? '')
                    setError(null)
                  }}
                >
                  Cancel
                </button>
              </form>
            ) : (
              <>
                {displayName.trim() || 'None — Agent ID is shown'}
                <button
                  type="button"
                  className="text-action"
                  onClick={() => setEditing(true)}
                >
                  Edit
                </button>
              </>
            )}
            {message && (
              <span className="form-success" role="status">
                {message}
              </span>
            )}
            {error && (
              <span className="form-error" role="alert">
                {error}
              </span>
            )}
          </dd>
        </div>
        <div>
          <dt>Agent</dt>
          <dd>
            <Link className="text-action" to={`/admin/agents/${node.agent_id}`}>
              {shortId(node.agent_id)}
            </Link>{' '}
            <span className="muted">· {node.rpc_endpoint}</span>
          </dd>
        </div>
        <div>
          <dt>First seen</dt>
          <dd>{formatObservedAt(node.first_seen_at)}</dd>
        </div>
        <div>
          <dt>Metadata updated</dt>
          <dd>{formatObservedAt(node.updated_at)}</dd>
        </div>
        <div>
          <dt>Audit trail</dt>
          <dd>
            Every mutation is recorded in the Server Audit log.{' '}
            <Link className="text-action" to="/admin/access/audit">
              Open the Audit log
            </Link>
          </dd>
        </div>
      </dl>
    </article>
  )
}

/** Node Inventory/lifecycle (CONTEXT.md: Active Node / Retired Node): the
 * latest valid Agent Inventory decides Active vs Retired. This is Server
 * state separate from Agent liveness, Node health, and freshness; retiring
 * is a local configuration fact, never a remote Server action. */
function LifecyclePanel({ node }: { node: AdminNodeDetailDto }) {
  const lifecycle = lifecycleLabel(node.lifecycle)
  const known = lifecycle.label === 'Active' || lifecycle.label === 'Retired'
  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Node Inventory &amp; lifecycle</h2>
        <StatusBadge status={lifecycle.label} tone={lifecycle.tone} />
      </div>
      <p className="panel-copy">
        Lifecycle follows the latest valid Agent Inventory: Active Nodes are eligible for
        current observation and alert evaluation, while Retired Nodes keep their identity
        and history but no longer receive live observation alerts. This is not Agent
        liveness or Node health, and the Server never changes lifecycle remotely.
      </p>
      <dl className="detail-list">
        <div>
          <dt>Lifecycle</dt>
          <dd>
            {lifecycle.label}{' '}
            <span className="muted">· Inventory revision {node.inventory_revision}</span>
          </dd>
        </div>
        <div>
          <dt>Lifecycle guidance</dt>
          <dd>
            {known ? node.lifecycle_guidance : 'No lifecycle disposition has been observed yet.'}
          </dd>
        </div>
      </dl>
    </article>
  )
}

function HealthPanel({ node }: { node: AdminNodeDetailDto }) {
  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Health and freshness</h2>
        <StatusBadge status={node.health} tone={healthTone(node.health)} />
      </div>
      <p className="panel-copy">
        {node.health_reason}. Freshness:{' '}
        <StatusBadge
          status={freshnessLabel(node.freshness)}
          tone={freshnessTone(node.freshness)}
        />
      </p>
      <p className="panel-copy">
        Health and freshness are separate Server dimensions: Unknown, Stale, and Error are
        never shown as Healthy or as zero values.
      </p>
      <dl className="detail-list">
        <div>
          <dt>Node data size</dt>
          <dd>
            {formatBytes(node.data_directory?.size_bytes)}
            {node.data_directory?.state && (
              <span className="muted">
                {' '}· {componentStateLabel(node.data_directory.state)}
              </span>
            )}
          </dd>
        </div>
        <div>
          <dt>Current head</dt>
          <dd>{node.current_head ?? 'Unknown'}</dd>
        </div>
        <div>
          <dt>Last-good head</dt>
          <dd>
            {node.sync?.current_block != null
              ? `last-good head ${node.sync.current_block}`
              : 'Unknown'}
          </dd>
        </div>
        <div>
          <dt>Historical high watermark</dt>
          <dd>{node.historical_high_watermark ?? 'Unknown'}</dd>
        </div>
        <div>
          <dt>Resync</dt>
          <dd>
            {node.resync_state}
            {node.resync_progress ? <span className="muted"> · {node.resync_progress}</span> : null}
          </dd>
        </div>
        <div>
          <dt>Network reference head</dt>
          <dd>
            {node.network_reference_head ?? 'Unknown'}
            <span className="muted"> · {node.network_reference_confidence}</span>
          </dd>
        </div>
      </dl>
    </article>
  )
}

function IdentityPanel({ node }: { node: AdminNodeDetailDto }) {
  const identity = identityBadge(node.identity)
  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Network identity</h2>
        <StatusBadge status={identity.label} tone={identity.tone} />
      </div>
      <p className="panel-copy">
        The Agent-observed identity never overwrites the Registry. A mismatch blocks history
        merging and is a separate diagnostic from RPC Error or Node Offline.
      </p>
      {node.identity.state === 'unknown' && (
        <p className="panel-state">
          <StatusBadge status="Unknown" tone="neutral" /> This Node has not reported Network
          identity yet.
        </p>
      )}
      {node.identity.mismatched_fields.length > 0 && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Mismatched" tone="error" /> Contradicts the Registry:{' '}
          {node.identity.mismatched_fields.join(', ')}. New history is not merged into the
          registered Network history.
        </p>
      )}
      <dl className="detail-list">
        <div>
          <dt>Network</dt>
          <dd>
            <Link className="text-action" to={`/admin/networks/${node.network_key}`}>
              {node.network_display_name}
            </Link>{' '}
            <span className="muted">({node.network_key})</span>
          </dd>
        </div>
        <div>
          <dt>Observed genesis hash</dt>
          <dd>
            <code>{node.identity.observed?.genesis_hash ?? 'Not observed'}</code>
          </dd>
        </div>
        <div>
          <dt>Observed chain ID / P2P network</dt>
          <dd>
            {node.identity.observed?.chain_id ?? 'Not observed'} /{' '}
            {node.identity.observed?.p2p_network_id ?? 'Not observed'}
          </dd>
        </div>
        <div>
          <dt>Observed address HRP</dt>
          <dd>{node.identity.observed?.address_hrp ?? 'Not observed'}</dd>
        </div>
        <div>
          <dt>Node key fingerprint</dt>
          <dd>{node.node_key_fingerprint ?? 'Unknown'}</dd>
        </div>
      </dl>
    </article>
  )
}

function RpcDiagnosticsPanel({ node }: { node: AdminNodeDetailDto }) {
  const rpc = node.rpc
  const state = componentStateLabel(rpc?.state)
  const tone = state === 'Current' ? 'ok' : state === 'Error' ? 'error' : 'neutral'
  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>RPC diagnostics</h2>
        <StatusBadge status={state} tone={tone} />
      </div>
      <p className="panel-copy">
        Administrative connection diagnostics only. The complete Node observation view remains
        on Home; the RPC Endpoint is always redacted.
      </p>
      <dl className="detail-list">
        <div>
          <dt>Redacted RPC Endpoint</dt>
          <dd><code>{node.rpc_endpoint}</code></dd>
        </div>
        <div>
          <dt>Client version</dt>
          <dd>{rpc?.client_version ?? 'Unknown'}</dd>
        </div>
        <div>
          <dt>RPC namespaces</dt>
          <dd>{rpc?.namespaces.length ? rpc.namespaces.join(', ') : 'Unknown'}</dd>
        </div>
        <div>
          <dt>Probed methods</dt>
          <dd>{rpc?.methods.length ? rpc.methods.join(', ') : 'Unknown'}</dd>
        </div>
        <div>
          <dt>Observed / received</dt>
          <dd>
            {formatObservedAt(rpc?.observed_at)} · {formatObservedAt(rpc?.received_at)}
          </dd>
        </div>
        {rpc?.error_message && (
          <div>
            <dt>Last RPC error</dt>
            <dd><span className="component-error">{rpc.error_message}</span></dd>
          </div>
        )}
      </dl>
    </article>
  )
}


