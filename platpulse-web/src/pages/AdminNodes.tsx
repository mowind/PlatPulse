import { ArrowUp, ArrowUpDown, ChevronUp, ChevronRight } from 'lucide-react'
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
} from 'react'
import { Link, useParams, useSearchParams } from 'react-router'
import {
  AdminApiError,
  purgeNode,
  updateNodeMetadata,
  useAdminNodeDetail,
  useAdminNodeMetricHistory,
  useAdminNodePurgeImpact,
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
import { SURFACE_CARD_STATIC, SURFACE_TOOLBAR } from '../lib/surface'
import {
  METRIC_HISTORY_PRESETS,
  NODE_METRIC_SERIES,
  formatHistoryDuration,
  formatSampleDelay,
  metricAvailabilityNotice,
  metricBandPath,
  metricChartGeometry,
  metricGapKindLabel,
  metricHistoryRange,
  metricLinePath,
  nodeMetricDefinition,
  type NodeMetricKey,
} from '../metricHistory'
import type {
  AdminNodeDetail as AdminNodeDetailDto,
  AdminNodeListItem,
  NodePurgeResponse,
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

const CARD_SURFACE = cn('rounded-md border-none', SURFACE_CARD_STATIC)

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
  const [purged, setPurged] = useState<NodePurgeResponse | null>(null)
  const notFound = query.isError && query.error instanceof AdminApiError && query.error.code === 'not_found'

  if (purged) {
    return (
      <section className="w-full min-w-0 space-y-4">
        <div className="space-y-1">
          <h1 className="text-lg font-semibold">Node permanently deleted</h1>
          <p className="text-sm text-muted-foreground">
            {purged.node_id} was permanently deleted at {formatObservedAt(purged.deleted_at)}.{' '}
            {purged.removed.total_owned_rows} Node-owned rows were removed. The same Node ID
            will not return; re-monitoring this deployment requires a new locally configured
            Node ID.
          </p>
          <p className="text-sm text-muted-foreground">
            The remote process was not stopped and local configuration was not changed.
            Shared Agent/Host/Network data, independent Validator history, and existing
            Incident evidence remain.
          </p>
          <p className="text-sm">
            <Link
              autoFocus
              className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
              to="/admin/nodes"
            >
              Back to All Nodes
            </Link>
          </p>
        </div>
      </section>
    )
  }

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
          ·{' '}
          <Link
            className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
            to={`/admin/alerts/incidents?subject=node&subject_key=${encodeURIComponent(nodeId)}`}
          >
            Incidents for this Node
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
          <MetricHistoryPanel node={query.data} />
          <IdentityPanel node={query.data} />
          <RpcDiagnosticsPanel node={query.data} />
          <PurgePanel node={query.data} csrfToken={csrfToken} onPurged={setPurged} />
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

/** Owner-only raw metric history of one Node series (issue #213, design
 * §11.4): the stored observations with the timing evidence that belongs to
 * each one, the silences between them, and the state of the series itself.
 * A series the Node never reported is named as absent, a range older than the
 * retained raw window is answered as unavailable, and nothing is ever filled
 * in with zeros or a line drawn across a stretch the Server did not observe. */
function MetricHistoryPanel({ node }: { node: AdminNodeDetailDto }) {
  const { generation } = useAuth()
  const [metric, setMetric] = useState<NodeMetricKey>('process_cpu_percent')
  const [hours, setHours] = useState<number>(24)
  // The answered range is fixed per selection: the query key stays stable
  // while the page is open, and "Reload window" moves it explicitly.
  const [range, setRange] = useState(() => metricHistoryRange(24, new Date()))
  const definition = nodeMetricDefinition(metric)
  const query = useAdminNodeMetricHistory(generation, node.node_id, metric, range.from, range.to)
  const data = query.data
  const fixedMax = definition.fixedMax
  const geometry = useMemo(
    () =>
      data
        ? metricChartGeometry(
            data.items,
            data.gaps,
            Date.parse(data.from),
            Date.parse(data.to),
            fixedMax,
          )
        : null,
    [data, fixedMax],
  )
  const notice = metricAvailabilityNotice(data?.availability)
  const series = data?.series
  const newest = data ? data.items.slice(-8).reverse() : []
  const state = !data ? 'Loading' : series?.observed ? 'Observed' : 'Never observed'
  const stateTone = !data ? 'neutral' : series?.observed ? 'ok' : 'neutral'

  function selectRange(nextHours: number) {
    setHours(nextHours)
    setRange(metricHistoryRange(nextHours, new Date()))
  }

  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={
        <>
          <h2 className="text-lg font-semibold">Metric history</h2>
          <StatusBadge status={state} tone={stateTone} />
        </>
      }
    >
      <p className="text-sm text-muted-foreground">
        Stored raw observations for one Node series, with the delay between observing and
        receiving each sample, the silences between samples, and the series state that outlives
        them. This is the raw grain only: every value below is one observation the Node
        actually sent, never a value averaged over a coarser interval.
      </p>
      <div className="mt-3 flex flex-wrap items-end gap-3">
        <label className="grid min-w-40 gap-1 text-xs font-medium text-muted-foreground">
          Metric series
          <Select
            className="min-h-11"
            value={metric}
            onChange={(event) => setMetric(event.currentTarget.value as NodeMetricKey)}
          >
            {NODE_METRIC_SERIES.map((item) => (
              <option key={item.metric} value={item.metric}>
                {item.label}
              </option>
            ))}
          </Select>
        </label>
        <div className="flex flex-wrap gap-2" role="group" aria-label="History range">
          {METRIC_HISTORY_PRESETS.map((preset) => (
            <Button
              key={preset.label}
              variant={hours === preset.hours ? 'default' : 'outline'}
              aria-pressed={hours === preset.hours}
              className="min-h-11"
              onClick={() => selectRange(preset.hours)}
            >
              {preset.label}
            </Button>
          ))}
        </div>
        <Button
          variant="outline"
          className="min-h-11"
          onClick={() => setRange(metricHistoryRange(hours, new Date()))}
        >
          Reload window
        </Button>
      </div>
      <p className="mt-2 text-xs text-muted-foreground">
        {definition.label}: {definition.unit}. {definition.description}
      </p>
      {!data && query.isPending && (
        <p className="mt-3 flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
          <StatusBadge status="Loading" tone="neutral" /> Loading the stored series…
        </p>
      )}
      {query.isError && (
        <div
          className="mt-3 flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" />{' '}
          <span className="min-w-0 break-words">
            {query.error instanceof Error ? query.error.message : 'Unable to load the metric history'}
          </span>
          <Button variant="link" size="sm" onClick={() => void query.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {data && query.isRefetchError && (
        <div
          className="mt-3 flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
          successful samples.
        </div>
      )}
      {data && notice && (
        <p className="mt-3 text-sm text-muted-foreground" data-slot="metric-history-availability">
          {notice}
        </p>
      )}
      {data && (
        <>
          <div className="mt-3">
            <DetailList>
              <DetailItem label="Ledger">
                {series?.observed
                  ? series.observationCount + ' stored observation(s) since the first one'
                  : 'No observation was ever recorded for this series'}
              </DetailItem>
              <DetailItem label="First observed">{formatObservedAt(series?.firstObservedAt)}</DetailItem>
              <DetailItem label="Last observed">{formatObservedAt(series?.lastObservedAt)}</DetailItem>
              <DetailItem label="Last received">{formatObservedAt(series?.lastReceivedAt)}</DetailItem>
              <DetailItem label="Coverage">
                {formatHistoryDuration(series?.coverageSeconds ?? 0)} of{' '}
                {formatHistoryDuration(series?.windowSeconds ?? 0)}
                <span className="text-[11px] text-muted-foreground">
                  {' '}· proven only between stored samples, never assumed across a silence
                </span>
              </DetailItem>
              <DetailItem label="Carried deliveries">
                {series?.replayedCount ?? 0} replay(s), {series?.correctedCount ?? 0} correction(s)
                <span className="text-[11px] text-muted-foreground">
                  {' '}· counted apart from observations
                </span>
              </DetailItem>
              <DetailItem label="Samples in this answer">
                {series?.sampledCount ?? 0} · grain {data.grain}
                {data.aggregateSupported ? '' : ' (raw only)'}
              </DetailItem>
              <DetailItem label="Newest delay">
                {formatSampleDelay(series?.latestDelaySeconds)}
                {series?.latestClockSuspect && (
                  <span className="text-destructive"> · clock suspect</span>
                )}
              </DetailItem>
              <DetailItem label="Retained raw window">
                {data.rawRetentionDays} {data.rawRetentionDays === 1 ? 'day' : 'days'} · requested
                from {formatObservedAt(data.requestedFrom)}
              </DetailItem>
            </DetailList>
          </div>
          {data.truncated && (
            <p className="mt-3 text-sm" role="status" data-slot="metric-history-truncated">
              This window holds more samples than one answer carries: the newest{' '}
              {data.items.length} are drawn and the older ones are omitted, so the plot starts at
              the oldest sample it actually has rather than inventing one at the window edge.
            </p>
          )}
          {series && !series.observed && (
            <p className="mt-3 text-sm text-muted-foreground" role="status" data-slot="metric-history-empty">
              This Node never reported {definition.label}. Nothing is charted and nothing is shown
              as zero; the series appears with the first accepted observation.
            </p>
          )}
          {series?.observed && data.items.length === 0 && (
            <p className="mt-3 text-sm text-muted-foreground" role="status" data-slot="metric-history-empty">
              The series is observed, but no stored sample falls inside this window. The value is
              reported as unknown, never as zero.
            </p>
          )}
          {geometry && geometry.samples > 0 && (
            <div className="mt-3 grid min-w-0 grid-cols-[3rem_minmax(0,1fr)] gap-x-2">
              <div
                className="flex flex-col justify-between pr-1 text-right text-[11px] tabular-nums text-muted-foreground"
                aria-hidden="true"
              >
                <span>{definition.axisFormat(geometry.max)}</span>
                <span>{definition.axisFormat(geometry.max / 2)}</span>
                <span>{definition.axisFormat(0)}</span>
              </div>
              <svg
                viewBox="0 0 600 150"
                preserveAspectRatio="none"
                role="img"
                aria-label={
                  definition.label +
                  ' raw history from ' +
                  formatObservedAt(data.from) +
                  ' to ' +
                  formatObservedAt(data.to)
                }
                className="col-start-2 h-40 w-full text-primary"
                data-slot="metric-history-chart"
              >
                <title>{definition.label} raw history over {formatHistoryDuration(data.windowSeconds)}</title>
                <desc>
                  {geometry.samples} stored observation(s) folded into {geometry.segments.length}{' '}
                  drawn stretch(es); {data.gaps.length} reported silence(s) are left undrawn.
                </desc>
                <g aria-hidden="true">
                  <line x1="0" y1="8" x2="600" y2="8" className="stroke-border [vector-effect:non-scaling-stroke]" />
                  <line x1="0" y1="75" x2="600" y2="75" className="stroke-border [vector-effect:non-scaling-stroke]" />
                  <line x1="0" y1="142" x2="600" y2="142" className="stroke-border [vector-effect:non-scaling-stroke]" />
                  {geometry.gapBands.map((band) => (
                    <rect
                      key={band.x.toFixed(2)}
                      data-slot="metric-history-gap-band"
                      className="fill-muted-foreground opacity-20"
                      x={band.x}
                      y={8}
                      width={Math.max(1, band.width)}
                      height={134}
                    />
                  ))}
                </g>
                <g aria-hidden="true">
                  {geometry.segments.map((segment, index) => (
                    <g key={segment[0].x.toFixed(2) + '-' + index}>
                      {segment.length > 1 && (
                        <>
                          <path
                            className="fill-current opacity-15"
                            d={metricBandPath(segment)}
                          />
                          <path
                            data-slot="metric-history-line"
                            className="fill-none stroke-current [vector-effect:non-scaling-stroke]"
                            strokeWidth={2}
                            strokeLinecap="round"
                            strokeLinejoin="round"
                            d={metricLinePath(segment)}
                          />
                        </>
                      )}
                      {segment.length === 1 && (
                        <>
                          {/* One column, but possibly several observations: a
                              vertical whisker spans the bucket's minimum and
                              maximum so an isolated spike is never hidden by
                              the single point drawn at its newest value. It
                              stays inside the column: no line is interpolated
                              across the time the column does not cover. */}
                          <line
                            data-slot="metric-history-whisker"
                            className="stroke-current [vector-effect:non-scaling-stroke]"
                            strokeWidth={2}
                            strokeLinecap="round"
                            x1={segment[0].x}
                            x2={segment[0].x}
                            y1={segment[0].top}
                            y2={segment[0].bottom}
                          />
                          <circle
                            data-slot="metric-history-point"
                            className="fill-current stroke-background [vector-effect:non-scaling-stroke]"
                            strokeWidth={1.5}
                            cx={segment[0].x}
                            cy={segment[0].y}
                            r={3.5}
                          />
                        </>
                      )}
                    </g>
                  ))}
                </g>
              </svg>
              <div
                className="col-start-2 flex justify-between pt-1 text-[11px] tabular-nums text-muted-foreground"
                aria-hidden="true"
              >
                <span>{formatObservedAt(data.from)}</span>
                <span>{formatObservedAt(data.to)}</span>
              </div>
            </div>
          )}
          {data.gaps.length > 0 && (
            <div className="mt-4" data-slot="metric-history-gaps">
              <h3 className="text-sm font-medium">Silences in this window</h3>
              <ul className="mt-1 space-y-1 text-sm text-muted-foreground">
                {data.gaps.map((gap) => (
                  <li key={gap.from + gap.to}>
                    <span className="font-medium text-foreground">
                      {metricGapKindLabel(gap.kind)}
                    </span>
                    {': '}
                    {formatObservedAt(gap.from)} → {formatObservedAt(gap.to)} (
                    {formatHistoryDuration(gap.seconds)}) · {gap.reason}
                    {gap.skippedCount != null
                      ? ' · ' + gap.skippedCount + ' observation(s) skipped'
                      : ''}
                  </li>
                ))}
              </ul>
            </div>
          )}
          {newest.length > 0 && (
            <div className="mt-4">
              <h3 className="text-sm font-medium">Newest observations</h3>
              <div className="mt-1 overflow-x-auto">
                <table className="w-full text-left text-sm" data-slot="metric-history-samples">
                  <thead>
                    <tr className="text-xs font-medium tracking-wider text-muted-foreground">
                      <th scope="col" className="py-1 pr-3">Observed</th>
                      <th scope="col" className="py-1 pr-3">Received</th>
                      <th scope="col" className="py-1 pr-3">Value</th>
                      <th scope="col" className="py-1">Delay</th>
                    </tr>
                  </thead>
                  <tbody>
                    {newest.map((sample) => (
                      <tr key={sample.observedAt + '-' + sample.value} data-slot="metric-history-sample">
                        <td className="py-1 pr-3 tabular-nums">{formatObservedAt(sample.observedAt)}</td>
                        <td className="py-1 pr-3 tabular-nums">{formatObservedAt(sample.receivedAt)}</td>
                        <td className="py-1 pr-3 tabular-nums font-medium">
                          {definition.format(sample.value)}
                        </td>
                        <td className="py-1 tabular-nums">
                          {formatSampleDelay(sample.delaySeconds)}
                          {sample.clockSuspect && (
                            <span className="block text-[11px] text-destructive">
                              {sample.clockNote ?? 'The observation is stamped after the receipt.'}
                            </span>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <p className="mt-1 text-xs text-muted-foreground">
                The newest {newest.length} of {data.items.length} samples in this answer. Observed,
                received, and the delay belong to the same observation, so a spooled or retried
                delivery stays visible per sample.
              </p>
            </div>
          )}
        </>
      )}
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

/** Owner-only permanent Node Purge (design §15.3, webui.md §15.2): explicit,
 * irreversible deletion with a Server-computed impact preview. Nothing is
 * optimistically removed; the success view is driven by the authoritative
 * Server response, and a failed mutation leaves the Node actionable. */
function PurgePanel({
  node,
  csrfToken,
  onPurged,
}: {
  node: AdminNodeDetailDto
  csrfToken: string
  onPurged: (result: NodePurgeResponse) => void
}) {
  const { generation } = useAuth()
  const [confirming, setConfirming] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [restoreFocus, setRestoreFocus] = useState(false)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const impact = useAdminNodePurgeImpact(generation, node.node_id, confirming)

  // Cancelling the confirmation restores focus to the safe trigger, so a
  // keyboard user is never dropped onto the document body.
  useEffect(() => {
    if (!confirming && restoreFocus) {
      triggerRef.current?.focus()
      setRestoreFocus(false)
    }
  }, [confirming, restoreFocus])

  async function confirm() {
    setBusy(true)
    setError(null)
    try {
      onPurged(await purgeNode(node.node_id, node.node_id, csrfToken))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to delete the Node')
      setBusy(false)
    }
  }

  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={<h2 className="text-lg font-semibold">Permanent deletion</h2>}
    >
      <p className="text-sm text-muted-foreground">
        Permanently delete this Node to remove it from Home and current monitoring and to
        delete its observations, monitoring history, and Node Validator Links. This cannot be
        undone, and the same Node ID will not return; re-monitoring this deployment requires a
        new locally configured Node ID.
      </p>
      <p className="mt-2 text-sm text-muted-foreground">
        The remote Agent/Node process is not stopped or uninstalled, and local configuration is
        not changed, so you must still handle the Host locally. Shared Agent/Host/Network data,
        independent Validator history, and existing Incident evidence are preserved. This is
        not retirement or a visibility change.
      </p>
      {!confirming && (
        <div className="mt-3">
          <Button
            ref={triggerRef}
            variant="destructive"
            onClick={() => setConfirming(true)}
          >
            Permanently delete Node
          </Button>
        </div>
      )}
      {confirming && (
        <div
          className="mt-3 space-y-3 rounded-md border border-destructive/40 bg-destructive/5 p-3"
          role="alertdialog"
          aria-label="Confirm permanent Node deletion"
        >
          <p className="text-sm font-medium">
            Confirm permanent deletion of {node.display_name ?? node.node_id}?
          </p>
          {!impact.data && impact.isPending && (
            <p role="status" className="text-sm text-muted-foreground">
              Measuring the deletion scope…
            </p>
          )}
          {impact.isError && (
            <div className="text-sm" role="alert">
              <span className="text-destructive">
                {impact.error instanceof Error
                  ? impact.error.message
                  : 'Unable to measure the deletion scope'}
              </span>{' '}
              <Button variant="link" size="sm" onClick={() => void impact.refetch()}>
                Try again
              </Button>
            </div>
          )}
          {impact.data && (
            <ul className="list-disc space-y-1 pl-5 text-sm text-muted-foreground">
              <li>{impact.data.counts.total_owned_rows} Node-owned rows will be deleted.</li>
              <li>
                {impact.data.counts.block_summaries} Block Summaries,{' '}
                {impact.data.counts.peer_presence_intervals} Peer presence intervals, and{' '}
                {impact.data.counts.peer_aggregate_5m +
                  impact.data.counts.peer_aggregate_5m_countries +
                  impact.data.counts.peer_aggregate_1h +
                  impact.data.counts.peer_aggregate_1h_countries}{' '}
                Peer aggregates will be deleted.
              </li>
              <li>
                {impact.data.counts.validator_links} Node Validator Link(s) and{' '}
                {impact.data.counts.transfers} Transfer record(s) will be deleted.
              </li>
              <li>
                Independent Validator history, shared Agent/Host/Network data, and Alert
                Incident evidence are not deleted.
              </li>
            </ul>
          )}
          {error && (
            <p className="text-sm text-destructive" role="alert">
              {error}
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button
              variant="destructive"
              disabled={busy || !impact.data}
              onClick={() => void confirm()}
            >
              {busy ? 'Deleting…' : 'Confirm permanent deletion'}
            </Button>
            <Button
              variant="outline"
              autoFocus
              disabled={busy}
              onClick={() => {
                setConfirming(false)
                setError(null)
                setRestoreFocus(true)
              }}
            >
              Cancel
            </Button>
          </div>
        </div>
      )}
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
