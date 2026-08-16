import { useMemo, useState, type FormEvent, type KeyboardEvent } from 'react'
import { Link, useParams, useSearchParams } from 'react-router'
import {
  AdminApiError,
  updateNodeMetadata,
  updateNodeVisibility,
  useAdminNodeDetail,
  useAdminNodePeerChurn,
  useAdminNodes,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
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
import { transferBadge } from './AdminNodeTransfer'

/**
 * PAGE-ADMIN-NODES, PAGE-ADMIN-NODE-DETAIL, and PAGE-ADMIN-NODE-VISIBILITY
 * (design §4.3, §8.2): Owner-only Node inventory and management. Every row
 * is one Node — block, transaction, consensus, peer, and error state never
 * merge across Nodes. Server-owned metadata (display name, visibility,
 * lifecycle guidance) stays distinct from Agent-observed identity and
 * endpoint configuration; lifecycle follows the latest Agent Inventory and
 * the Server never remotely changes a Node.
 */

function shortId(value: string): string {
  return value.length > 12 ? `${value.slice(0, 8)}…${value.slice(-4)}` : value
}

function formatDuration(seconds: number | null | undefined): string {
  if (seconds == null) return 'Open'
  if (seconds < 60) return `${seconds}s`
  const minutes = Math.floor(seconds / 60)
  const remainingSeconds = seconds % 60
  if (minutes < 60) return `${minutes}m ${remainingSeconds}s`
  const hours = Math.floor(minutes / 60)
  return `${hours}h ${minutes % 60}m`
}

function healthTone(health: string): 'ok' | 'error' | 'neutral' {
  return health === 'healthy' ? 'ok' : health === 'unhealthy' ? 'error' : 'neutral'
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
  const freshness = node.freshness === 'current' ? 'ok' : node.freshness === 'stale' ? 'warning' : 'neutral'
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
          <span>{node.lifecycle === 'retired' ? 'Retired' : 'Active'}</span>
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

/** PAGE-ADMIN-NODE-DETAIL: Server-owned metadata, lifecycle guidance,
 * identity disposition, and the independent observation dimensions. */
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
          <div className="page-actions">
            <Link
              className="primary-action"
              to={`/admin/nodes/${query.data.node_id}/visibility`}
            >
              {query.data.visibility === 'public' ? 'Make private' : 'Publish to Home'}
            </Link>
            <Link
              className="secondary-action"
              to={`/admin/nodes/${query.data.node_id}/transfer`}
            >
              Transfer ownership
            </Link>
          </div>
          <MetadataPanel node={query.data} csrfToken={csrfToken} />
          <HealthPanel node={query.data} />
          <IdentityPanel node={query.data} />
          <TransferPanel node={query.data} />
          <ObservationsPanel node={query.data} />
        <PeerSnapshotPanel node={query.data} />
  <PeerChurnPanel nodeId={query.data.node_id} />
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
        <span className="panel-count">{node.visibility === 'public' ? 'Public' : 'Private'}</span>
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
                {node.display_name ?? 'None — Agent ID is shown'}
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
          <dt>Visibility</dt>
          <dd>
            <StatusBadge
              status={node.visibility === 'public' ? 'Public' : 'Private'}
              tone={node.visibility === 'public' ? 'ok' : 'neutral'}
            />{' '}
            <Link className="text-action" to={`/admin/nodes/${node.node_id}/visibility`}>
              Change
            </Link>
          </dd>
        </div>
        <div>
          <dt>Lifecycle</dt>
          <dd>
            {node.lifecycle === 'retired' ? 'Retired' : 'Active'}{' '}
            <span className="muted">· Inventory revision {node.inventory_revision}</span>
          </dd>
        </div>
        <div>
          <dt>Lifecycle guidance</dt>
          <dd>{node.lifecycle_guidance}</dd>
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
      </dl>
    </article>
  )
}

/** Two-phase ownership handover summary (issue #46): the latest typed
 * outcome with a direct link to PAGE-ADMIN-NODE-TRANSFER. */
function TransferPanel({ node }: { node: AdminNodeDetailDto }) {
  const transfer = node.transfer
  const badge = transfer ? transferBadge(transfer.status) : null
  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Node transfer</h2>
        {badge ? <StatusBadge status={badge.label} tone={badge.tone} /> : null}
      </div>
      <p className="panel-copy">
        Transfer is two-phase: the source Agent stays authoritative until the target Agent
        declares the same Node ID with a validated Network Identity. The Server never pushes
        an RPC Endpoint or command to either Agent.
      </p>
      {badge === null ? (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> This Node has never been transferred.{' '}
          <Link className="text-action" to={`/admin/nodes/${node.node_id}/transfer`}>
            Transfer ownership
          </Link>
        </p>
      ) : (
        <dl className="detail-list">
          <div>
            <dt>Status</dt>
            <dd>{badge.label}</dd>
          </div>
          <div>
            <dt>Source → Target</dt>
            <dd>
              {shortId(transfer!.source_agent_id)} → {shortId(transfer!.target_agent_id)}
            </dd>
          </div>
          <div>
            <dt>Expires</dt>
            <dd>{formatObservedAt(transfer!.expires_at)}</dd>
          </div>
          {transfer!.mismatched_fields.length > 0 && (
            <div>
              <dt>Mismatched fields</dt>
              <dd>{transfer!.mismatched_fields.join(', ')}</dd>
            </div>
          )}
          <div>
            <dt>Workflow</dt>
            <dd>
              <Link className="text-action" to={`/admin/nodes/${node.node_id}/transfer`}>
                Open the transfer workflow
              </Link>
            </dd>
          </div>
        </dl>
      )}
      {transfer?.status === 'identity_mismatch' && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Identity mismatch" tone="error" /> Blocking diagnostic: the
          target-declared Network Identity contradicts the registered Network. Ownership stays
          with the source Agent and new history is not merged into the registered Network
          history.
        </p>
      )}
    </article>
  )
}

function HealthPanel({ node }: { node: AdminNodeDetailDto }) {
  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Health summary</h2>
        <StatusBadge status={node.health} tone={healthTone(node.health)} />
      </div>
      <p className="panel-copy">
        {node.health_reason}. Freshness:{' '}
        <StatusBadge
          status={freshnessLabel(node.freshness)}
          tone={node.freshness === 'current' ? 'ok' : node.freshness === 'stale' ? 'warning' : 'neutral'}
        />
      </p>
      <dl className="detail-list">
        <div>
          <dt>Current head</dt>
          <dd>{node.current_head ?? 'Unknown'}</dd>
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

function ObservationsPanel({ node }: { node: AdminNodeDetailDto }) {
  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Per-Node observations</h2>
        <span className="panel-count">5 dimensions</span>
      </div>
      <dl className="detail-list">
        <ObservationRow
          label="Process"
          state={node.process?.state}
          errorMessage={node.process?.error_message}
          observedAt={node.process?.observed_at}
          receivedAt={node.process?.received_at}
          detail={node.process?.pid != null ? `pid ${node.process.pid}` : undefined}
        />
        <ObservationRow
          label="RPC"
          state={node.rpc?.state}
          errorMessage={node.rpc?.error_message}
          observedAt={node.rpc?.observed_at}
          receivedAt={node.rpc?.received_at}
          detail={
            node.rpc?.client_version
              ? `${node.rpc.client_version} · ${node.rpc.namespaces.length} namespaces`
              : undefined
          }
        />
        <ObservationRow
          label="Sync"
          state={node.sync?.state}
          errorMessage={node.sync?.error_message}
          observedAt={node.sync?.observed_at}
          receivedAt={node.sync?.received_at}
          detail={
            node.sync?.current_block != null
              ? `last-good head ${node.sync.current_block}${
                  node.sync.highest_block != null ? ` · highest ${node.sync.highest_block}` : ''
                }`
              : undefined
          }
        />
        <ObservationRow
          label="Consensus"
          state={node.consensus?.state}
          errorMessage={node.consensus?.error_message}
          observedAt={node.consensus?.observed_at}
          receivedAt={node.consensus?.received_at}
          detail={
            node.consensus?.highest_commit_block != null
              ? `last-good commit ${node.consensus.highest_commit_block} · validator ${
                  node.consensus.validator === true ? 'yes' : 'no'
                }`
              : undefined
          }
        />
        <ObservationRow
          label="Peers"
          state={node.peers?.state}
          errorMessage={node.peers?.error_message}
          observedAt={node.peers?.observed_at}
          receivedAt={node.peers?.received_at}
          detail={
            node.peers?.peer_count != null
              ? `${node.peers.peer_count} peers · ${node.peers.freshness}`
              : undefined
          }
        />
      </dl>
    </article>
  )
}

function PeerSnapshotPanel({ node }: { node: AdminNodeDetailDto }) {
  const peers = node.peers
  if (!peers) {
    return (
      <article className="panel">
        <div className="panel-heading">
          <h2>Peer snapshot</h2>
          <StatusBadge status="Unknown" tone="neutral" />
        </div>
        <p className="panel-state">This Node has not reported a Peer Snapshot yet.</p>
      </article>
    )
  }
  const tone = peers.state === 'error' ? 'error' : peers.state === 'ok' ? 'ok' : 'neutral'
  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Peer snapshot</h2>
        <StatusBadge status={componentStateLabel(peers.state)} tone={tone} />
      </div>
      <p className="panel-copy">
        {peers.peer_count == null
          ? 'No successful Peer Snapshot is retained.'
          : `${peers.peer_count} peer${peers.peer_count === 1 ? '' : 's'} · ${peers.inbound_count ?? 0} inbound · ${peers.outbound_count ?? 0} outbound · freshness ${peers.freshness}.`}
        {peers.state === 'error' && peers.error_message ? ` Last collection failed: ${peers.error_message}` : ''}
      </p>
      {peers.peer_count === 0 ? (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> The Agent reported an authoritative empty Peer Snapshot.
        </p>
      ) : peers.peers.length > 0 ? (
        <div className="table-wrap">
          <table className="node-table">
            <caption className="sr-only">Current Peer Snapshot</caption>
            <thead>
              <tr>
                <th scope="col">Peer ID</th>
                <th scope="col">Direction</th>
                <th scope="col">Flags</th>
                <th scope="col">Client</th>
                <th scope="col">Capabilities</th>
                <th scope="col">CBFT</th>
              </tr>
            </thead>
            <tbody>
              {peers.peers.map((peer) => (
                <tr key={peer.peer_id}>
                  <th scope="row"><code>{peer.peer_id}</code></th>
                  <td>{peer.direction}</td>
                  <td>
                    {[peer.trusted && 'trusted', peer.static_peer && 'static', peer.consensus_peer && 'consensus']
                      .filter((value): value is string => Boolean(value))
                      .join(', ') || 'none'}
                  </td>
                  <td>{peer.client_name ?? 'Unknown'}</td>
                  <td>{peer.capabilities.length > 0 ? peer.capabilities.join(', ') : 'None'}</td>
                  <td>
                    {peer.cbft_commit_block != null
                      ? `commit ${peer.cbft_commit_block}`
                      : peer.cbft_protocol_version != null
                        ? `protocol ${peer.cbft_protocol_version}`
                        : 'Unknown'}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : null}
    </article>
  )
}

function PeerChurnPanel({ nodeId }: { nodeId: string }) {
  const { generation } = useAuth()
  const query = useAdminNodePeerChurn(generation, nodeId)
  const churn = query.data
  if (query.isPending) {
    return (
      <article className="panel">
        <div className="panel-heading">
          <h2>Peer churn</h2>
          <StatusBadge status="Starting" tone="neutral" />
        </div>
        <p className="panel-state">Loading recent Peer arrivals and departures…</p>
      </article>
    )
  }
  if (query.isError && !churn) {
    return (
      <article className="panel">
        <div className="panel-heading">
          <h2>Peer churn</h2>
          <StatusBadge status="Error" tone="error" />
        </div>
        <p className="panel-state">Recent Peer churn is unavailable. The last-good Peer Snapshot remains authoritative.</p>
      </article>
    )
  }
  if (!churn || churn.state === 'unknown') {
    return (
      <article className="panel">
        <div className="panel-heading">
          <h2>Peer churn</h2>
          <StatusBadge status="Unknown" tone="neutral" />
        </div>
        <p className="panel-state">
          No successful Peer Snapshot is available for churn history.
        </p>
      </article>
    )
  }
  const refetchError = query.isError
  const status = refetchError
    ? 'Error'
    : churn.state === 'empty'
      ? 'Empty'
      : churn.state === 'error'
        ? 'Error'
        : churn.state === 'unsupported'
          ? 'Unsupported'
          : churn.state === 'disabled'
            ? 'Disabled'
            : churn.state === 'starting'
              ? 'Starting'
              : freshnessLabel(churn.freshness)
  const tone = refetchError || churn.state === 'error'
    ? 'error'
    : churn.state === 'ok' && churn.freshness === 'current'
      ? 'ok'
      : churn.state === 'ok' && churn.freshness === 'stale'
        ? 'warning'
        : 'neutral'
  const intervals = [...churn.recent_arrivals, ...churn.recent_departures].filter(
    (interval, index, all) =>
      all.findIndex(
        (candidate) =>
          candidate.peer_id === interval.peer_id &&
          candidate.opened_at === interval.opened_at &&
          candidate.closed_at === interval.closed_at,
      ) === index,
  )
  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Peer churn</h2>
        <StatusBadge status={status} tone={tone} />
      </div>
      {churn.state === 'empty' ? (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No Peer presence interval has been retained for this Node yet.
        </p>
      ) : (
        <>
          <p className="panel-copy">
            {refetchError
              ? 'Refresh failed; showing retained intervals from the last successful response. '
              : churn.state === 'error'
                ? 'The latest Peer collection failed; retained intervals remain available. '
                : churn.state === 'unsupported'
                  ? 'Peer collection is unsupported; retained intervals remain available. '
                  : churn.state === 'disabled'
                    ? 'Peer collection is disabled; retained intervals remain available. '
                    : ''}
            Freshness: {freshnessLabel(churn.freshness)}. {churn.total_open_intervals} interval{churn.total_open_intervals === 1 ? '' : 's'} currently open; arrivals and departures are bounded to the last 24 hours. Raw addresses are never retained.
          </p>
          <div className="table-wrap">
            <table className="node-table">
              <caption className="sr-only">Recent Peer arrivals and departures</caption>
              <thead>
                <tr>
                  <th scope="col">Peer ID</th>
                  <th scope="col">Arrival</th>
                  <th scope="col">Departure</th>
                  <th scope="col">Duration</th>
                  <th scope="col">Direction</th>
                  <th scope="col">Flags</th>
                  <th scope="col">Client</th>
                </tr>
              </thead>
              <tbody>
                {intervals.map((interval) => (
                  <tr key={`${interval.peer_id}-${interval.opened_at}-${interval.closed_at ?? 'open'}`}>
                    <th scope="row" data-label="Peer ID"><code>{interval.peer_id}</code></th>
                    <td data-label="Arrival">{formatObservedAt(interval.opened_at)}</td>
                    <td data-label="Departure">{interval.closed_at ? formatObservedAt(interval.closed_at) : 'Current'}</td>
                    <td data-label="Duration">{formatDuration(interval.duration_seconds)}</td>
                    <td data-label="Direction">{interval.direction}</td>
                    <td data-label="Flags">
                      {[interval.trusted && 'trusted', interval.static_peer && 'static', interval.consensus_peer && 'consensus']
                        .filter((value): value is string => Boolean(value))
                        .join(', ') || 'none'}
                    </td>
                    <td data-label="Client">{interval.client_name ?? 'Unknown'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
    </article>
  )
}

function ObservationRow({
  label,
  state,
  errorMessage,
  observedAt,
  receivedAt,
  detail,
}: {
  label: string
  state: string | null | undefined
  errorMessage?: string | null
  observedAt?: string | null
  receivedAt?: string | null
  detail?: string
}) {
  const tone = state === 'error' ? 'error' : state === 'ok' ? 'ok' : 'neutral'
  return (
    <div className="component-row">
      <dt>{label}</dt>
      <dd>
        <StatusBadge status={componentStateLabel(state)} tone={tone} />
        {state === 'error' && errorMessage && (
          <span className="component-error"> {errorMessage}</span>
        )}
        {detail && <span className="muted"> {detail}</span>}
        {observedAt && <small className="muted"> · observed {formatObservedAt(observedAt)}</small>}
        {receivedAt && <small className="muted"> · received {formatObservedAt(receivedAt)}</small>}
      </dd>
    </div>
  )
}

/** PAGE-ADMIN-NODE-VISIBILITY: dedicated public/private workflow with
 * confirmation copy and authoritative refetch after success. */
export function AdminNodeVisibility() {
  const { nodeId = '' } = useParams()
  const { generation, status } = useAuth()
  const query = useAdminNodeDetail(generation, nodeId)
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const node = query.data
  const visibility = visibilityBadge(node?.visibility ?? 'private')

  async function confirm() {
    if (!node) return
    setMessage(null)
    setError(null)
    setBusy(true)
    try {
      const target = node.visibility === 'public' ? 'private' : 'public'
      const result = await updateNodeVisibility(node.node_id, target, csrfToken)
      setMessage(
        `${node.display_name ?? node.node_id} is now ${result.visibility}. The Home projection was updated.`,
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to update visibility')
    } finally {
      setBusy(false)
    }
  }

  if (query.isError && query.error instanceof AdminApiError && query.error.code === 'not_found') {
    return (
      <section className="page">
        <h1>Node unavailable</h1>
        <p>This Node is no longer available.</p>
      </section>
    )
  }
  return (
    <section className="page">
      <h1>Node visibility</h1>
      <p className="muted">
        <Link className="text-action" to={`/admin/nodes/${nodeId}`}>
          Back to Node detail
        </Link>
      </p>
      {!node && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading Node state…
        </p>
      )}
      {node && (
        <article className="panel">
          <div className="panel-heading">
            <h2>{node.display_name ?? node.node_id}</h2>
            <StatusBadge status={visibility.label} tone={visibility.tone} />
          </div>
          <p className="panel-copy">
            {node.visibility === 'public'
              ? 'This Node is currently published on the Home projection. Making it private removes it from Home; Admin stays available and the Server keeps collecting.'
              : 'This Node is private. Publishing it adds it to the Home projection with the Server-owned health summary. Endpoint, credential, and identity details stay hidden from Home.'}
          </p>
          <p className="panel-copy">
            Visibility is Server-owned display metadata. It does not change the Agent-local
            configuration, the Node lifecycle, or any observation.
          </p>
          <div className="action-row">
            <button
              type="button"
              className="primary-action"
              onClick={() => void confirm()}
              disabled={busy}
            >
              {node.visibility === 'public' ? 'Make private' : 'Publish to Home'}
            </button>
            <Link className="secondary-action" to={`/admin/nodes/${node.node_id}`}>
              Cancel
            </Link>
          </div>
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
      )}
    </section>
  )
}
