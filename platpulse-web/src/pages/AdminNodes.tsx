import { ArrowUp, ArrowUpDown, ChevronUp, ChevronRight } from 'lucide-react'
import { useMemo, useState, type FormEvent, type KeyboardEvent, type ReactNode } from 'react'
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
  freshnessTone,
  healthTone,
  identityBadge,
  lifecycleLabel,
  visibilityBadge,
} from '../nodeLabels'
import {
  StatusBadge,
  componentStateLabel,
  formatObservedAt,
  freshnessLabel,
} from '../components/StatusBadge'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import { Input, Select } from '../components/ui/input'
import { cn } from '../lib/utils'
import { SURFACE_CARD, SURFACE_TOOLBAR } from '../lib/surface'
import type {
  AdminNodeDetail as AdminNodeDetailDto,
  AdminNodeListItem,
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

const CARD_SURFACE = cn('rounded-md border-none', SURFACE_CARD)

function shortId(value: string): string {
  return value.length > 12 ? value.slice(0, 8) + '…' + value.slice(-4) : value
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

/** Emerald detail grid: label above its value, stacked on narrow viewports. */
function DetailList({ children }: { children: ReactNode }) {
  return <dl className="grid grid-cols-1 gap-3 sm:grid-cols-2">{children}</dl>
}

function DetailItem({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs font-medium tracking-wider text-muted-foreground">{label}</dt>
      <dd className="mt-0.5 min-w-0 break-words text-sm">{children}</dd>
    </div>
  )
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
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className="text-lg font-semibold break-words">Nodes</h1>
        <p className="text-sm text-muted-foreground">
          Each row is one Node. Health, freshness, identity, visibility, and lifecycle are
          separate Server-owned dimensions; lifecycle follows the latest Agent Inventory.
        </p>
      </div>
      <NodeFiltersBar
        filters={filters}
        networks={networks}
        onChange={setFilter}
        sort={sort}
        onSort={setSort}
      />
      {!query.data && query.isPending && (
        <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Node inventory…
        </p>
      )}
      {!query.data && query.isError && (
        <div
          className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" />{' '}
          <span className="min-w-0 break-words">
            {query.error instanceof Error ? query.error.message : 'Unable to load Nodes'}
          </span>
          <Button variant="link" size="sm" onClick={() => void query.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {query.data && query.isRefetchError && (
        <div
          className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
          successful Node values.
        </div>
      )}
      {query.data && nodes.length === 0 && (
        <CardX size="medium" className={CARD_SURFACE}>
          <Empty description="No Nodes match these filters." />
        </CardX>
      )}
      {query.data && nodes.length > 0 && (
        <CardX
          size="medium"
          className={CARD_SURFACE}
          contentClassName="p-0"
          segmented
          title="Node inventory"
        >
          <div className="overflow-x-auto">
            <table data-stack data-slot="node-table" className="w-full text-sm">
              <caption className="sr-only">
                Node inventory: identity, health, freshness, visibility, and lifecycle
              </caption>
              <thead>
                <tr className="border-b">
                  <th
                    scope="col"
                    className="px-3 py-2 text-left text-xs font-medium text-muted-foreground"
                    aria-sort={sort === 'name' ? 'ascending' : 'none'}
                  >
                    <SortButton label="Node" column="name" sort={sort} onSort={setSort} />
                  </th>
                  <th
                    scope="col"
                    className="px-3 py-2 text-left text-xs font-medium text-muted-foreground"
                    aria-sort={sort === 'network' ? 'ascending' : 'none'}
                  >
                    <SortButton label="Network" column="network" sort={sort} onSort={setSort} />
                  </th>
                  <th
                    scope="col"
                    className="px-3 py-2 text-left text-xs font-medium text-muted-foreground"
                    aria-sort={sort === 'health' ? 'ascending' : 'none'}
                  >
                    <SortButton label="Health" column="health" sort={sort} onSort={setSort} />
                  </th>
                  <th
                    scope="col"
                    className="px-3 py-2 text-left text-xs font-medium text-muted-foreground"
                    aria-sort={sort === 'freshness' ? 'ascending' : 'none'}
                  >
                    <SortButton label="Freshness" column="freshness" sort={sort} onSort={setSort} />
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Identity
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Visibility
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Lifecycle
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Head / Sync
                  </th>
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
        </CardX>
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
    <Button
      variant="ghost"
      size="xs"
      className="justify-start gap-1 px-0 text-xs font-medium text-muted-foreground hover:bg-transparent"
      aria-pressed={active}
      onClick={() => onSort(column)}
    >
      {label} {active ? <ArrowUp size={12} aria-hidden="true" /> : <ArrowUpDown size={12} aria-hidden="true" />}
    </Button>
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
    <div
      className={cn(
        'flex flex-wrap items-end gap-3 rounded-md border-none p-3',
        SURFACE_TOOLBAR,
      )}
      role="group"
      aria-label="Node filters"
    >
      <label className="flex flex-col gap-1 text-xs font-medium tracking-wider text-muted-foreground">
        Sort by
        <Select
          className="w-auto min-w-36"
          value={sort}
          onChange={(event) => onSort(event.target.value as 'name' | 'network' | 'health' | 'freshness')}
        >
          <option value="health">Health</option>
          <option value="name">Node name</option>
          <option value="network">Network</option>
          <option value="freshness">Freshness</option>
        </Select>
      </label>
      <label className="flex flex-col gap-1 text-xs font-medium tracking-wider text-muted-foreground">
        Network
        <Select
          className="w-auto min-w-36"
          value={filters.network}
          onChange={(event) => onChange('network', event.target.value)}
        >
          <option value="all">All</option>
          {networks.map((network) => (
            <option key={network} value={network}>
              {network}
            </option>
          ))}
        </Select>
      </label>
      <label className="flex flex-col gap-1 text-xs font-medium tracking-wider text-muted-foreground">
        Visibility
        <Select
          className="w-auto min-w-36"
          value={filters.visibility}
          onChange={(event) => onChange('visibility', event.target.value)}
        >
          <option value="all">All</option>
          <option value="public">Public</option>
          <option value="private">Private</option>
        </Select>
      </label>
      <label className="flex flex-col gap-1 text-xs font-medium tracking-wider text-muted-foreground">
        Lifecycle
        <Select
          className="w-auto min-w-36"
          value={filters.lifecycle}
          onChange={(event) => onChange('lifecycle', event.target.value)}
        >
          <option value="all">All</option>
          <option value="active">Active</option>
          <option value="retired">Retired</option>
        </Select>
      </label>
      <label className="flex flex-col gap-1 text-xs font-medium tracking-wider text-muted-foreground">
        Health
        <Select
          className="w-auto min-w-36"
          value={filters.health}
          onChange={(event) => onChange('health', event.target.value)}
        >
          <option value="all">All</option>
          <option value="healthy">Healthy</option>
          <option value="unhealthy">Unhealthy</option>
          <option value="unknown">Unknown</option>
        </Select>
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
  const detailId = 'node-inventory-detail-' + node.node_id
  const collapseOnEscape = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === 'Escape' && expanded) onToggle()
  }
  return (
    <>
      <tr className="border-b border-border/60 align-top">
        <th scope="row" data-label="Node" className="min-w-0 px-3 py-3 text-left">
          <Button
            variant="ghost"
            size="icon-xs"
            className="align-middle"
            aria-label={(expanded ? 'Collapse ' : 'Expand ') + (node.display_name ?? node.node_id)}
            aria-expanded={expanded}
            aria-controls={detailId}
            onClick={onToggle}
            onKeyDown={collapseOnEscape}
          >
            <ChevronRight size={16} aria-hidden="true" className={expanded ? "rotate-90" : ""} />
          </Button>{' '}
          <Link
            className="inline-flex min-h-11 min-w-11 items-center break-all font-medium underline-offset-4 hover:underline"
            to={'/admin/nodes/' + node.node_id}
          >
            {node.display_name ?? node.node_id}
          </Link>
          <small className="mt-0.5 block text-[11px] text-muted-foreground break-all" title={node.node_id}>
            Node ID · {shortId(node.node_id)} · {node.rpc_endpoint}
          </small>
        </th>
        <td data-label="Network" className="min-w-0 px-3 py-3">
          <Link
            className="inline-flex min-h-11 min-w-11 items-center break-all font-medium underline-offset-4 hover:underline"
            to={'/admin/networks/' + node.network_key}
          >
            {node.network_display_name}
          </Link>
        </td>
        <td data-label="Health" className="min-w-0 px-3 py-3">
          <StatusBadge status={node.health} tone={health} />
          <small className="mt-0.5 block text-[11px] text-muted-foreground break-words">
            {node.health_reason}
          </small>
        </td>
        <td data-label="Freshness" className="min-w-0 px-3 py-3">
          <StatusBadge status={freshnessLabel(node.freshness)} tone={freshness} />
          <small className="mt-0.5 block text-[11px] text-muted-foreground">
            {node.freshness === 'current' ? 'Reporting now' : 'Check age on detail'}
          </small>
        </td>
        <td data-label="Identity" className="min-w-0 px-3 py-3">
          <StatusBadge status={identity.label} tone={identity.tone} />
          {node.identity.mismatched_fields.length > 0 && (
            <small className="mt-0.5 block text-[11px] text-muted-foreground">
              {node.identity.mismatched_fields.join(', ')}
            </small>
          )}
        </td>
        <td data-label="Visibility" className="min-w-0 px-3 py-3">
          <StatusBadge status={visibility.label} tone={visibility.tone} />
        </td>
        <td data-label="Lifecycle" className="min-w-0 px-3 py-3">
          <span className="text-sm">{lifecycleLabel(node.lifecycle).label}</span>
          <small className="mt-0.5 block text-[11px] text-muted-foreground">
            rev {node.inventory_revision}
          </small>
        </td>
        <td data-label="Head / Sync" className="min-w-0 px-3 py-3">
          <span className="font-bold leading-none tracking-tight">{node.current_head ?? 'Unknown'}</span>
          <small className="mt-0.5 block text-[11px] text-muted-foreground">{node.resync_state}</small>
        </td>
      </tr>
      {expanded && (
        <tr data-slot="detail-row" className="border-b border-border/60 bg-muted/30">
          <td colSpan={8} id={detailId} onKeyDown={collapseOnEscape} className="p-0">
            <div className="space-y-3 p-3">
              <Button variant="link" size="sm" onClick={onToggle}>
                Collapse details <ChevronUp size={16} aria-hidden="true" />
              </Button>
              <DetailList>
                <DetailItem label="Lifecycle guidance">{node.lifecycle_guidance}</DetailItem>
                <DetailItem label="Identity disposition">
                  {identity.label}
                  {node.identity.mismatched_fields.length > 0
                    ? ' — contradicts the Registry on ' + node.identity.mismatched_fields.join(', ')
                    : ''}
                </DetailItem>
                <DetailItem label="Health reason">{node.health_reason}</DetailItem>
                <DetailItem label="Resync state">{node.resync_state}</DetailItem>
              </DetailList>
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
      <section className="w-full min-w-0 space-y-4">
        <div className="space-y-1">
          <h1 className="text-lg font-semibold">Node unavailable</h1>
          <p className="text-sm text-muted-foreground">This Node is no longer available.</p>
        </div>
      </section>
    )
  }
  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className="text-lg font-semibold break-words">
          {query.data?.display_name ?? shortId(nodeId)}
          <span className="mt-0.5 block text-xs font-medium text-muted-foreground break-all">
            {nodeId}
          </span>
        </h1>
        <p className="text-sm text-muted-foreground">
          <Link
            className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
            to="/admin/nodes"
          >
            All Nodes
          </Link>{' '}
          · Server-owned metadata stays distinct from Agent-observed identity and configuration.
          Lifecycle is Node Inventory state and is never confused with Agent liveness or Node
          health.
        </p>
      </div>
      {!query.data && (
        <>
          {query.isPending && (
            <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
              <StatusBadge status="Starting" tone="neutral" /> Loading Node state…
            </p>
          )}
          {query.isError && (
            <div
              className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
              role="alert"
            >
              <StatusBadge status="Error" tone="error" />{' '}
              <span className="min-w-0 break-words">
                {query.error instanceof Error ? query.error.message : 'Unable to load the Node'}
              </span>
              <Button variant="link" size="sm" onClick={() => void query.refetch()}>
                Try again
              </Button>
            </div>
          )}
        </>
      )}
      {query.data && (
        <>
          {query.isRefetchError && (
            <div
              className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
              role="alert"
            >
              <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
              successful Node values.
            </div>
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
      setMessage('Display name is now "' + result.displayName + '".')
      setEditing(false)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to update the display name')
    }
  }

  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={<h2 className="text-lg font-semibold">Server-owned metadata</h2>}
    >
      <dl className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <div className="min-w-0">
          <dt className="text-xs font-medium tracking-wider text-muted-foreground">Display name</dt>
          <dd className="mt-0.5 min-w-0 break-words text-sm">
            {editing ? (
              <form className="flex flex-wrap items-center gap-2" onSubmit={submit}>
                <label className="sr-only" htmlFor={'display-name-' + node.node_id}>
                  Display name
                </label>
                <Input
                  id={'display-name-' + node.node_id}
                  className="max-w-xs"
                  value={displayName}
                  onChange={(event) => {
                    setDisplayName(event.target.value)
                    setConfirming(false)
                  }}
                  maxLength={128}
                />
                {confirming ? (
                  <>
                    <span className="text-[11px] text-muted-foreground">
                      Rename this Node in the Server-owned metadata? The Agent Inventory,
                      endpoint, and Node ID are not touched.
                    </span>
                    <Button type="button" onClick={() => void confirm()}>
                      Confirm rename
                    </Button>
                    <Button
                      variant="outline"
                      type="button"
                      onClick={() => setConfirming(false)}
                    >
                      Keep editing
                    </Button>
                  </>
                ) : (
                  <Button type="submit">Save</Button>
                )}
                <Button
                  variant="outline"
                  type="button"
                  onClick={() => {
                    setEditing(false)
                    setConfirming(false)
                    setDisplayName(node.display_name ?? '')
                    setError(null)
                  }}
                >
                  Cancel
                </Button>
              </form>
            ) : (
              <span className="inline-flex flex-wrap items-center gap-2">
                {displayName.trim() || 'None — Agent ID is shown'}
                <Button variant="link" size="sm" onClick={() => setEditing(true)}>
                  Edit
                </Button>
              </span>
            )}
            {message && (
              <span className="mt-1 block text-sm text-success" role="status">
                {message}
              </span>
            )}
            {error && (
              <span className="mt-1 block text-sm text-destructive" role="alert">
                {error}
              </span>
            )}
          </dd>
        </div>
        <DetailItem label="Agent">
          <Link
            className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
            to={'/admin/agents/' + node.agent_id}
          >
            {shortId(node.agent_id)}
          </Link>{' '}
          <span className="text-muted-foreground">· {node.rpc_endpoint}</span>
        </DetailItem>
        <DetailItem label="First seen">{formatObservedAt(node.first_seen_at)}</DetailItem>
        <DetailItem label="Metadata updated">{formatObservedAt(node.updated_at)}</DetailItem>
        <DetailItem label="Audit trail">
          Every mutation is recorded in the Server Audit log.{' '}
          <Link
            className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
            to="/admin/access/audit"
          >
            Open the Audit log
          </Link>
        </DetailItem>
      </dl>
    </CardX>
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
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={
        <>
          <h2 className="text-lg font-semibold">Node Inventory &amp; lifecycle</h2>
          <StatusBadge status={lifecycle.label} tone={lifecycle.tone} />
        </>
      }
    >
      <p className="text-sm text-muted-foreground">
        Lifecycle follows the latest valid Agent Inventory: Active Nodes are eligible for
        current observation and alert evaluation, while Retired Nodes keep their identity
        and history but no longer receive live observation alerts. This is not Agent
        liveness or Node health, and the Server never changes lifecycle remotely.
      </p>
      <div className="mt-3">
        <DetailList>
          <DetailItem label="Lifecycle">
            {lifecycle.label}{' '}
            <span className="text-muted-foreground">· Inventory revision {node.inventory_revision}</span>
          </DetailItem>
          <DetailItem label="Lifecycle guidance">
            {known ? node.lifecycle_guidance : 'No lifecycle disposition has been observed yet.'}
          </DetailItem>
        </DetailList>
      </div>
    </CardX>
  )
}

function HealthPanel({ node }: { node: AdminNodeDetailDto }) {
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={
        <>
          <h2 className="text-lg font-semibold">Health and freshness</h2>
          <StatusBadge status={node.health} tone={healthTone(node.health)} />
        </>
      }
    >
      <p className="text-sm text-muted-foreground">
        {node.health_reason}. Freshness:{' '}
        <StatusBadge
          status={freshnessLabel(node.freshness)}
          tone={freshnessTone(node.freshness)}
        />
      </p>
      <p className="mt-2 text-sm text-muted-foreground">
        Health and freshness are separate Server dimensions: Unknown, Stale, and Error are
        never shown as Healthy or as zero values.
      </p>
      <div className="mt-3">
        <DetailList>
          <DetailItem label="Node data size">
            <span className="font-bold leading-none tracking-tight">
              {formatBytes(node.data_directory?.size_bytes)}
            </span>
            {node.data_directory?.state && (
              <span className="text-[11px] font-medium text-muted-foreground">
                {' '}· {componentStateLabel(node.data_directory.state)}
              </span>
            )}
          </DetailItem>
          <DetailItem label="Current head">
            <span className="font-bold leading-none tracking-tight">
              {node.current_head ?? 'Unknown'}
            </span>
          </DetailItem>
          <DetailItem label="Last-good head">
            {node.sync?.current_block != null
              ? 'last-good head ' + node.sync.current_block
              : 'Unknown'}
          </DetailItem>
          <DetailItem label="Historical high watermark">
            <span className="font-bold leading-none tracking-tight">
              {node.historical_high_watermark ?? 'Unknown'}
            </span>
          </DetailItem>
          <DetailItem label="Resync">
            {node.resync_state}
            {node.resync_progress ? (
              <span className="text-muted-foreground"> · {node.resync_progress}</span>
            ) : null}
          </DetailItem>
          <DetailItem label="Network reference head">
            <span className="font-bold leading-none tracking-tight">
              {node.network_reference_head ?? 'Unknown'}
            </span>
            <span className="text-[11px] font-medium text-muted-foreground">
              {' '}· {node.network_reference_confidence}
            </span>
          </DetailItem>
        </DetailList>
      </div>
    </CardX>
  )
}

function IdentityPanel({ node }: { node: AdminNodeDetailDto }) {
  const identity = identityBadge(node.identity)
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={
        <>
          <h2 className="text-lg font-semibold">Network identity</h2>
          <StatusBadge status={identity.label} tone={identity.tone} />
        </>
      }
    >
      <p className="text-sm text-muted-foreground">
        The Agent-observed identity never overwrites the Registry. A mismatch blocks history
        merging and is a separate diagnostic from RPC Error or Node Offline.
      </p>
      {node.identity.state === 'unknown' && (
        <p className="mt-2 flex flex-wrap items-center gap-2 text-sm">
          <StatusBadge status="Unknown" tone="neutral" /> This Node has not reported Network
          identity yet.
        </p>
      )}
      {node.identity.mismatched_fields.length > 0 && (
        <p className="mt-2 flex flex-wrap items-center gap-2 text-sm" role="alert">
          <StatusBadge status="Mismatched" tone="error" /> Contradicts the Registry:{' '}
          {node.identity.mismatched_fields.join(', ')}. New history is not merged into the
          registered Network history.
        </p>
      )}
      <div className="mt-3">
        <DetailList>
          <DetailItem label="Network">
            <Link
              className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
              to={'/admin/networks/' + node.network_key}
            >
              {node.network_display_name}
            </Link>{' '}
            <span className="text-muted-foreground">({node.network_key})</span>
          </DetailItem>
          <DetailItem label="Observed genesis hash">
            <code className="break-all text-[11px]">
              {node.identity.observed?.genesis_hash ?? 'Not observed'}
            </code>
          </DetailItem>
          <DetailItem label="Observed chain ID / P2P network">
            {node.identity.observed?.chain_id ?? 'Not observed'} /{' '}
            {node.identity.observed?.p2p_network_id ?? 'Not observed'}
          </DetailItem>
          <DetailItem label="Observed address HRP">
            {node.identity.observed?.address_hrp ?? 'Not observed'}
          </DetailItem>
          <DetailItem label="Node key fingerprint">
            {node.node_key_fingerprint ?? 'Unknown'}
          </DetailItem>
        </DetailList>
      </div>
    </CardX>
  )
}

function RpcDiagnosticsPanel({ node }: { node: AdminNodeDetailDto }) {
  const rpc = node.rpc
  const state = componentStateLabel(rpc?.state)
  const tone = state === 'Current' ? 'ok' : state === 'Error' ? 'error' : 'neutral'
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={
        <>
          <h2 className="text-lg font-semibold">RPC diagnostics</h2>
          <StatusBadge status={state} tone={tone} />
        </>
      }
    >
      <p className="text-sm text-muted-foreground">
        Administrative connection diagnostics only. The complete Node observation view remains
        on Home; the RPC Endpoint is always redacted.
      </p>
      <div className="mt-3">
        <DetailList>
          <DetailItem label="Redacted RPC Endpoint">
            <code className="break-all text-[11px]">{node.rpc_endpoint}</code>
          </DetailItem>
          <DetailItem label="Client version">{rpc?.client_version ?? 'Unknown'}</DetailItem>
          <DetailItem label="RPC namespaces">
            {rpc?.namespaces.length ? rpc.namespaces.join(', ') : 'Unknown'}
          </DetailItem>
          <DetailItem label="Probed methods">
            {rpc?.methods.length ? rpc.methods.join(', ') : 'Unknown'}
          </DetailItem>
          <DetailItem label="Observed / received">
            {formatObservedAt(rpc?.observed_at)} · {formatObservedAt(rpc?.received_at)}
          </DetailItem>
          {rpc?.error_message && (
            <DetailItem label="Last RPC error">
              <span className="text-destructive break-words">{rpc.error_message}</span>
            </DetailItem>
          )}
        </DetailList>
      </div>
    </CardX>
  )
}
