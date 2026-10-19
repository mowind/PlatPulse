import { ArrowUp, ArrowUpDown, ChevronDown, ChevronUp, ChevronRight, X } from 'lucide-react'
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
} from 'react'
import { Link, useParams, useSearchParams } from 'react-router'
import {
  AdminApiError,
  purgeNode,
  updateNodeMetadata,
  useAdminNodeDetail,
  useAdminNodeHostMetricHistory,
  useAdminNodeMetricHistory,
  useAdminNodePurgeImpact,
  useAdminNodeStateHistory,
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
import { stateHistoryComponent, type StateHistoryComponent } from '../stateHistory'
import {
  validatorIdentityNeedsAttention,
  validatorIdentityNotice,
  validatorKeyLabel,
} from '../validators'
import {
  HOST_METRIC_SERIES,
  NODE_METRIC_SERIES,
  nodeMetricDefinition,
  type NodeMetricKey,
} from '../metricHistory'
import {
  HostMountPathControl,
  MetricHistoryBody,
  useHostMetricSelection,
  useMetricHistoryWindow,
} from './metricHistoryPanel'
import { StateHistoryBody } from './stateHistoryPanel'
import type {
  AdminNodeDetail as AdminNodeDetailDto,
  AdminNodeListItem,
  NodePurgeResponse,
} from '../api/generated'
import { DetailItem, DetailList } from '../components/DetailList'
import { AdminIdentifier } from '../components/AdminIdentifier'
import {
  AdminDetailSections,
  DETAIL_LINK,
  DetailBackLink,
  DetailDimension,
  DetailDimensions,
  DetailDraftProvider,
  DetailNotices,
  draftMarker,
  noticesMarkerTone,
  useDetailDrafts,
  useDetailSection,
  useReportDraft,
  type DetailNotice,
  type DetailSection,
} from '../components/AdminDetailSections'
import {
  ADMIN_DETAIL_BODY,
  ADMIN_DETAIL_ROW,
  ADMIN_PAGE_TITLE,
  ADMIN_PANEL_SURFACE,
  ADMIN_ROW,
  ADMIN_SECONDARY,
  ADMIN_SECTION_TITLE,
  ADMIN_TD,
  ADMIN_TH,
  AdminKeyValue,
  AdminKeyValueList,
  AdminPageHeader,
  AdminPanel,
  AdminToolbar,
  AdminToolbarField,
} from '../components/AdminPage'

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

const CARD_SURFACE = ADMIN_PANEL_SURFACE

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

const NODE_SORTS = ['health', 'name', 'network', 'freshness'] as const
type NodeSort = (typeof NODE_SORTS)[number]
const DEFAULT_NODE_SORT: NodeSort = 'health'

/** The list order is URL state too, so reload and back/forward restore it;
 *  an unknown value falls back to the default order. */
function readSort(search: URLSearchParams): NodeSort {
  const value = search.get('sort')
  return (NODE_SORTS as readonly string[]).includes(value ?? '') ? (value as NodeSort) : DEFAULT_NODE_SORT
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
  const sort = readSort(search)

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
  // Filters only: the chosen order (`sort`) and any other parameter stay.
  const clearFilters = (keys: (keyof NodeFilters)[]) => {
    const next = new URLSearchParams(search)
    for (const key of keys) next.delete(key)
    setSearch(next, { replace: false })
  }
  const setSort = (value: NodeSort) => {
    const next = new URLSearchParams(search)
    if (value === DEFAULT_NODE_SORT) next.delete('sort')
    else next.set('sort', value)
    setSearch(next, { replace: false })
  }
  const clearAllFilters = () => clearFilters(['network', 'health', 'visibility', 'lifecycle'])

  const networks = useMemo(() => {
    const keys = new Set((query.data ?? []).map((node) => node.network_key))
    return [...keys].sort()
  }, [query.data])

  return (
    <section className="w-full min-w-0 space-y-4">
      <AdminPageHeader
        title="Nodes"
        description="Each row is one Node; Health, Freshness and Identity stay separate Server-owned dimensions."
        info={{
          label: 'About Node dimensions',
          content: (
            <p>
              Health, freshness, identity, visibility, and lifecycle are separate Server-owned
              dimensions; lifecycle follows the latest Agent Inventory.
            </p>
          ),
        }}
      />
      {query.data && query.isRefetchError && (
        <div
          className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
          successful Node values.
        </div>
      )}
      <AdminPanel
        slot="node-inventory"
        flush
        title="Node inventory"
        count={
          query.data ? (
            <span className="text-xs text-muted-foreground tabular-nums">
              {nodes.length === query.data.length
                ? nodes.length + ' Node' + (nodes.length === 1 ? '' : 's')
                : nodes.length + ' of ' + query.data.length + ' Nodes'}
            </span>
          ) : undefined
        }
      >
        <div className="border-b border-border/60 px-4 py-3">
          <NodeFiltersBar
            filters={filters}
            networks={networks}
            onChange={setFilter}
            onClear={clearFilters}
            onClearAll={clearAllFilters}
            sort={sort}
            onSort={setSort}
          />
        </div>
        {!query.data && query.isPending && (
          <p className="flex flex-wrap items-center gap-2 px-4 py-6 text-sm text-muted-foreground" role="status">
            <StatusBadge status="Starting" tone="neutral" /> Loading the Node inventory…
          </p>
        )}
        {!query.data && query.isError && (
          <div className="px-4 py-4">
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
          </div>
        )}
        {/* Two different empties: an inventory with no Nodes yet, and Nodes
            that the current filters exclude. Loading and failure above are
            never shown as either. */}
        {query.data && query.data.length === 0 && (
          <Empty description="No Nodes have been reported yet." className="text-center">
            <p data-slot="node-inventory-empty" className="max-w-prose text-sm">
              A Node appears here after an Agent's accepted Report declares it in that Agent's
              local Node Inventory, which is configured on the Agent's Host.
            </p>
            <Link
              className="inline-flex min-h-11 min-w-11 items-center font-medium text-foreground underline-offset-4 hover:underline"
              to="/admin/agents"
            >
              Open Agents
            </Link>
          </Empty>
        )}
        {query.data && query.data.length > 0 && nodes.length === 0 && (
          <Empty description="No Nodes match these filters." className="text-center">
            <p data-slot="node-filtered-empty" className="text-sm">
              {query.data.length === 1 ? 'The one Node in the inventory is' : 'All ' + query.data.length + ' Nodes in the inventory are'}{' '}
              outside the current filters.
            </p>
            <Button type="button" variant="outline" className="min-h-11" onClick={clearAllFilters}>
              Clear filters
            </Button>
          </Empty>
        )}
        {query.data && nodes.length > 0 && (
          <div className="overflow-x-auto max-md:px-3 max-md:pt-3">
            <table data-stack data-slot="node-table" className="w-full text-sm">
              <caption className="sr-only">
                Node inventory: Node and Network, health, freshness, head and sync; identity,
                visibility, lifecycle and endpoint details open per row. Sorted by{' '}
                {sort === 'name' ? 'Node name' : sort === 'network' ? 'Network' : sort === 'freshness' ? 'Freshness' : 'Health'}.
              </caption>
              <thead>
                <tr className="border-b border-border/60">
                  <th
                    scope="col"
                    className={ADMIN_TH}
                    aria-sort={sort === 'name' ? 'ascending' : 'none'}
                  >
                    <SortButton label="Node" column="name" sort={sort} onSort={setSort} />
                  </th>
                  <th
                    scope="col"
                    className={ADMIN_TH}
                    aria-sort={sort === 'health' ? 'ascending' : 'none'}
                  >
                    <SortButton label="Health" column="health" sort={sort} onSort={setSort} />
                  </th>
                  <th
                    scope="col"
                    className={ADMIN_TH}
                    aria-sort={sort === 'freshness' ? 'ascending' : 'none'}
                  >
                    <SortButton label="Freshness" column="freshness" sort={sort} onSort={setSort} />
                  </th>
                  <th scope="col" className={cn(ADMIN_TH, 'whitespace-nowrap')}>
                    Head / Sync
                  </th>
                  <th scope="col" className={cn(ADMIN_TH, 'w-px whitespace-nowrap')}>
                    <span className="sr-only">Row </span>Details
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
        )}
      </AdminPanel>
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
  column: NodeSort
  sort: string
  onSort: (column: NodeSort) => void
}) {
  const active = sort === column
  return (
    <Button
      variant="ghost"
      size="xs"
      // The 44px target fills its header cell (-my-2 cancels the cell's py-2)
      // instead of reaching 2px past it, where the table's scroller clipped
      // its top edge and the focus ring; the ring is drawn inside the button.
      className="-my-2 justify-start gap-1 px-0 text-xs font-medium text-muted-foreground hover:bg-transparent focus-visible:ring-inset aria-pressed:text-foreground"
      aria-pressed={active}
      onClick={() => onSort(column)}
    >
      {label} {active ? <ArrowUp size={12} aria-hidden="true" /> : <ArrowUpDown size={12} aria-hidden="true" />}
    </Button>
  )
}

const VISIBILITY_OPTIONS = [
  { value: 'public', label: 'Public' },
  { value: 'private', label: 'Private' },
]
const LIFECYCLE_OPTIONS = [
  { value: 'active', label: 'Active' },
  { value: 'retired', label: 'Retired' },
]

/** The low-frequency filters, named with their current value, for the
 *  collapsed "More filters" summary. */
function secondaryFilters(filters: NodeFilters): { key: 'visibility' | 'lifecycle'; label: string }[] {
  const active: { key: 'visibility' | 'lifecycle'; label: string }[] = []
  if (filters.visibility !== 'all') {
    const option = VISIBILITY_OPTIONS.find((entry) => entry.value === filters.visibility)
    active.push({ key: 'visibility', label: 'Visibility: ' + (option?.label ?? filters.visibility) })
  }
  if (filters.lifecycle !== 'all') {
    const option = LIFECYCLE_OPTIONS.find((entry) => entry.value === filters.lifecycle)
    active.push({ key: 'lifecycle', label: 'Lifecycle: ' + (option?.label ?? filters.lifecycle) })
  }
  return active
}

function NodeFiltersBar({
  filters,
  networks,
  onChange,
  onClear,
  onClearAll,
  sort,
  onSort,
}: {
  filters: NodeFilters
  networks: string[]
  onChange: (key: keyof NodeFilters, value: string) => void
  onClear: (keys: (keyof NodeFilters)[]) => void
  onClearAll: () => void
  sort: string
  onSort: (column: NodeSort) => void
}) {
  // Presentation only: the filters themselves live in the URL, so opening or
  // closing this group never changes the list.
  const [moreOpen, setMoreOpen] = useState(false)
  const secondary = secondaryFilters(filters)
  const anyFilter =
    filters.network !== 'all' || filters.health !== 'all' || secondary.length > 0
  return (
    <div className="grid min-w-0 gap-2">
      <AdminToolbar label="Node filters">
        <AdminToolbarField label="Network" size="lg">
          <Select value={filters.network} onChange={(event) => onChange('network', event.target.value)}>
            <option value="all">All</option>
            {networks.map((network) => (
              <option key={network} value={network}>
                {network}
              </option>
            ))}
          </Select>
        </AdminToolbarField>
        <AdminToolbarField label="Health">
          <Select value={filters.health} onChange={(event) => onChange('health', event.target.value)}>
            <option value="all">All</option>
            <option value="healthy">Healthy</option>
            <option value="unhealthy">Unhealthy</option>
            <option value="unknown">Unknown</option>
          </Select>
        </AdminToolbarField>
        <AdminToolbarField label="Sort by">
          <Select
            value={sort}
            onChange={(event) => onSort(event.target.value as NodeSort)}
          >
            <option value="health">Health</option>
            <option value="name">Node name</option>
            <option value="network">Network</option>
            <option value="freshness">Freshness</option>
          </Select>
        </AdminToolbarField>
        <div className="col-span-2 flex min-w-0 flex-wrap items-end gap-2 sm:col-span-1">
          <Button
            type="button"
            variant="outline"
            className="min-h-11"
            aria-expanded={moreOpen}
            aria-controls="node-more-filters"
            onClick={() => setMoreOpen((value) => !value)}
          >
            {moreOpen ? <ChevronUp size={16} aria-hidden="true" /> : <ChevronDown size={16} aria-hidden="true" />}
            More filters
            {secondary.length > 0 && (
              <span className="rounded-full bg-primary/10 px-1.5 text-xs tabular-nums">{secondary.length}</span>
            )}
            {secondary.length > 0 && <span className="sr-only">active</span>}
          </Button>
          {anyFilter && (
            <Button
              type="button"
              variant="link"
              className="min-h-11 px-1"
              onClick={onClearAll}
            >
              Clear filters
            </Button>
          )}
        </div>
      </AdminToolbar>
      {moreOpen ? (
        <div
          id="node-more-filters"
          role="group"
          aria-label="More Node filters"
          className="grid grid-cols-2 gap-x-3 gap-y-2 sm:flex sm:flex-wrap sm:items-end"
        >
          <AdminToolbarField label="Visibility">
            <Select value={filters.visibility} onChange={(event) => onChange('visibility', event.target.value)}>
              <option value="all">All</option>
              {VISIBILITY_OPTIONS.map((option) => (
                <option key={option.value} value={option.value}>{option.label}</option>
              ))}
            </Select>
          </AdminToolbarField>
          <AdminToolbarField label="Lifecycle">
            <Select value={filters.lifecycle} onChange={(event) => onChange('lifecycle', event.target.value)}>
              <option value="all">All</option>
              {LIFECYCLE_OPTIONS.map((option) => (
                <option key={option.value} value={option.value}>{option.label}</option>
              ))}
            </Select>
          </AdminToolbarField>
        </div>
      ) : (
        secondary.length > 0 && (
          // Each chip's box is its full 44px row: the clear button's touch
          // area is laid out, not pulled over the neighbouring row with a
          // negative margin. The light pill is drawn inside that box.
          <ul
            aria-label="Active filters from More filters"
            data-slot="node-active-filters"
            className="flex list-none flex-wrap items-center gap-x-2 gap-y-1 p-0"
          >
            {secondary.map((entry) => (
              <li
                key={entry.key}
                data-slot="node-active-filter"
                className="relative isolate inline-flex min-h-11 max-w-full items-center pl-2.5 text-xs"
              >
                <span
                  aria-hidden="true"
                  className="absolute inset-x-0 inset-y-2 -z-10 rounded-full border border-border/60 bg-muted/40"
                />
                <span className="min-w-0 py-2.5 [overflow-wrap:anywhere]">{entry.label}</span>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="size-11 shrink-0 rounded-full text-muted-foreground hover:bg-transparent hover:text-foreground"
                  aria-label={'Clear ' + entry.label}
                  onClick={() => onClear([entry.key])}
                >
                  <X size={14} aria-hidden="true" />
                </Button>
              </li>
            ))}
          </ul>
        )
      )}
    </div>
  )
}

/**
 * One Node summary row: the Node and its Network, the independent Health and
 * Freshness dimensions, and head/sync. Identity, visibility, lifecycle detail,
 * the redacted endpoint and the full Node ID open in the row's own details;
 * an identity that is not established or contradicts the Registry, and a
 * Retired lifecycle, stay visible on the row because they change what the
 * other columns mean.
 */
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
  const lifecycle = lifecycleLabel(node.lifecycle)
  const name = node.display_name ?? node.node_id
  const detailId = 'node-inventory-detail-' + node.node_id
  const toggleRef = useRef<HTMLButtonElement>(null)
  const collapseOnEscape = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== 'Escape' || !expanded) return
    onToggle()
    toggleRef.current?.focus()
  }
  const healthException = node.health !== 'healthy'
  const resyncException = node.resync_state !== 'normal'
  return (
    <>
      <tr className={cn(ADMIN_ROW, node.lifecycle === 'retired' && 'text-muted-foreground')}>
        {/* Two stacked groups, each a plain flex line whose 44px link targets
            sit side by side or one below the other — never pulled over a
            neighbour with negative margins. The cell's own vertical padding
            is dropped on desktop because the targets already provide it. */}
        <th scope="row" data-label="Node" className={cn(ADMIN_TD, 'min-w-0 text-left font-normal md:py-0')}>
          <div data-slot="node-name-group" className="flex min-w-0 flex-wrap items-center gap-x-2">
            <Link
              data-slot="node-name-link"
              className="flex min-h-11 w-fit min-w-11 max-w-full items-center py-1 leading-snug font-medium text-foreground [overflow-wrap:anywhere] underline-offset-4 hover:underline"
              to={'/admin/nodes/' + node.node_id}
            >
              {name}
            </Link>
            {node.lifecycle !== 'active' && (
              <span
                data-slot="node-lifecycle-marker"
                className="rounded-sm border border-border px-1.5 py-0.5 text-xs font-medium text-muted-foreground"
              >
                {lifecycle.label}
              </span>
            )}
          </div>
          {/* Network and, when it is not matched, the separate Identity
              dimension share the secondary group. */}
          {/* The secondary line's text sits at the top of its own 44px target,
              close under the name, instead of centred in it. */}
          <div data-slot="node-network-group" className="flex min-w-0 flex-wrap items-start gap-x-2">
            <Link
              data-slot="node-network-link"
              className={cn('inline-flex min-h-11 w-fit max-w-full items-start pt-1 pb-2 [overflow-wrap:anywhere] underline-offset-4 hover:underline', ADMIN_SECONDARY)}
              to={'/admin/networks/' + node.network_key}
            >
              {node.network_display_name}
            </Link>
            {node.identity.state === 'mismatched' && (
              <span data-slot="node-identity-summary" className="flex flex-wrap items-center gap-1.5 pt-0.5 pb-2 text-xs">
                <span className="text-muted-foreground">Identity</span>
                <StatusBadge status={identity.label} tone={identity.tone} />
                {node.identity.mismatched_fields.length > 0 && (
                  <span className="[overflow-wrap:anywhere]">{node.identity.mismatched_fields.join(', ')}</span>
                )}
              </span>
            )}
            {node.identity.state !== 'matched' && node.identity.state !== 'mismatched' && (
              // Not established is its own dimension, never folded into Health.
              <span data-slot="node-identity-summary" className={cn('pt-1 pb-2', ADMIN_SECONDARY)}>
                · Identity {identity.label === 'Unknown' ? 'not reported yet' : identity.label}
              </span>
            )}
          </div>
        </th>
        <td data-label="Health" className={cn(ADMIN_TD, 'min-w-0')}>
          <StatusBadge status={node.health} tone={healthTone(node.health)} />
          {healthException && (
            <small className={cn('mt-1 block [overflow-wrap:anywhere]', ADMIN_SECONDARY)}>
              {node.health_reason}
            </small>
          )}
        </td>
        <td data-label="Freshness" className={cn(ADMIN_TD, 'min-w-0')}>
          <StatusBadge status={freshnessLabel(node.freshness)} tone={freshnessTone(node.freshness)} />
          {node.freshness === 'stale' && (
            <small className={cn('mt-1 block', ADMIN_SECONDARY)}>Last good values; age on detail</small>
          )}
        </td>
        <td data-label="Head / Sync" className={cn(ADMIN_TD, 'min-w-0')}>
          <span className="font-medium tabular-nums">{node.current_head ?? 'Unknown'}</span>
          {resyncException && (
            <small className={cn('mt-0.5 block', ADMIN_SECONDARY)}>Resync {node.resync_state}</small>
          )}
        </td>
        {/* No desktop padding: the 44px button already spans it, so its label
            lines up with the Node name. */}
        <td data-label="Details" className={cn(ADMIN_TD, 'min-w-0 md:w-px md:py-0 md:whitespace-nowrap')}>
          <Button
            ref={toggleRef}
            variant="ghost"
            size="sm"
            className="min-h-11 gap-1 px-2"
            aria-expanded={expanded}
            aria-controls={detailId}
            onClick={onToggle}
            onKeyDown={collapseOnEscape}
          >
            <ChevronRight size={16} aria-hidden="true" className={cn('transition-transform', expanded && 'rotate-90')} />
            Details <span className="sr-only">for {name}</span>
          </Button>
        </td>
      </tr>
      {expanded && (
        <tr data-slot="detail-row" className={ADMIN_DETAIL_ROW}>
          <td colSpan={5} id={detailId} onKeyDown={collapseOnEscape} className="p-0">
            <div className={ADMIN_DETAIL_BODY}>
              <div className="flex flex-wrap items-center justify-between gap-2">
                <h3 className={ADMIN_SECTION_TITLE}>Node details</h3>
                <Button variant="link" size="sm" className="min-h-11" onClick={onToggle}>
                  Collapse details <ChevronUp size={16} aria-hidden="true" />
                </Button>
              </div>
              <AdminKeyValueList>
                <AdminKeyValue label="Node ID" wide>
                  <AdminIdentifier value={node.node_id} label="Node ID" fullSlot="node-full-id" />
                </AdminKeyValue>
                <AdminKeyValue label="Redacted RPC Endpoint" wide>
                  <code className="font-mono text-xs [overflow-wrap:anywhere]">{node.rpc_endpoint}</code>
                </AdminKeyValue>
                <AdminKeyValue label="Identity disposition">
                  <span className="flex flex-wrap items-center gap-2">
                    <StatusBadge status={identity.label} tone={identity.tone} />
                    {node.identity.mismatched_fields.length > 0
                      ? 'Contradicts the Registry on ' + node.identity.mismatched_fields.join(', ')
                      : null}
                  </span>
                </AdminKeyValue>
                <AdminKeyValue label="Visibility">
                  <StatusBadge status={visibility.label} tone={visibility.tone} />
                </AdminKeyValue>
                <AdminKeyValue label="Lifecycle">
                  <span className="flex flex-wrap items-center gap-2">
                    <StatusBadge status={lifecycle.label} tone={lifecycle.tone} />
                    <span className="tabular-nums">Inventory revision {node.inventory_revision}</span>
                  </span>
                </AdminKeyValue>
                <AdminKeyValue label="Resync state">{node.resync_state}</AdminKeyValue>
                <AdminKeyValue label="Health reason" wide>{node.health_reason}</AdminKeyValue>
                <AdminKeyValue label="Lifecycle guidance" wide>{node.lifecycle_guidance}</AdminKeyValue>
              </AdminKeyValueList>
              <p className={ADMIN_SECONDARY}>
                Health, freshness, identity, visibility and lifecycle are separate Server-owned
                dimensions.{' '}
                <Link
                  className="inline-flex min-h-11 items-center font-medium text-foreground underline-offset-4 hover:underline"
                  to={'/admin/nodes/' + node.node_id + '?section=diagnostics'}
                >
                  Open Node diagnostics
                </Link>
              </p>
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
  const [section, setSection, openSection] = useDetailSection()
  const drafts = useDetailDrafts()
  const notices = query.data ? nodeDetailNotices(query.data) : []
  const diagnosticNotices = notices.length

  if (purged) {
    return (
      <section className="w-full min-w-0 space-y-4">
        <div className="space-y-1">
          <h1 className={ADMIN_PAGE_TITLE}>Node permanently deleted</h1>
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
          <h1 className={ADMIN_PAGE_TITLE}>Node unavailable</h1>
          <p className="text-sm text-muted-foreground">This Node is no longer available.</p>
        </div>
      </section>
    )
  }
  const data = query.data
  return (
    <section className="w-full min-w-0 space-y-4" data-slot="admin-node-detail">
      {data ? (
        <NodeDetailHeader node={data} notices={notices} section={section} onSection={openSection} />
      ) : (
        <div className="space-y-1">
          <DetailBackLink to="/admin/nodes">All Nodes</DetailBackLink>
          <h1 className={ADMIN_PAGE_TITLE}>{shortId(nodeId)}</h1>
        </div>
      )}
      {!data && (
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
      {data && (
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
          <DetailDraftProvider value={drafts.report}>
            <AdminDetailSections
              label="Node detail sections"
              value={section}
              onValueChange={setSection}
              sections={[
                {
                  key: 'overview',
                  label: 'Overview',
                  content: (
                    <>
                      <HealthPanel node={data} />
                      <LifecyclePanel node={data} />
                    </>
                  ),
                },
                {
                  key: 'history',
                  label: 'History',
                  content: (
                    <>
                      <MetricHistoryPanel node={data} />
                      <SyncConsensusHistoryPanel node={data} />
                      <HostMetricHistoryPanel node={data} />
                    </>
                  ),
                },
                {
                  key: 'diagnostics',
                  label: 'Diagnostics',
                  marker: diagnosticNotices > 0 ? diagnosticNotices : undefined,
                  markerTone: noticesMarkerTone(notices),
                  markerLabel:
                    diagnosticNotices > 0
                      ? diagnosticNotices + ' current finding' + (diagnosticNotices === 1 ? '' : 's')
                      : undefined,
                  content: (
                    <>
                      <IdentityPanel node={data} />
                      <ValidatorIdentityPanel node={data} />
                      <RpcDiagnosticsPanel node={data} />
                      <ComponentDiagnosticsPanel node={data} />
                    </>
                  ),
                },
                {
                  key: 'manage',
                  label: 'Manage',
                  ...draftMarker(drafts.pending),
                  content: (
                    <>
                      <MetadataPanel node={data} csrfToken={csrfToken} />
                      <PurgePanel node={data} csrfToken={csrfToken} onPurged={setPurged} />
                    </>
                  ),
                },
              ]}
            />
          </DetailDraftProvider>
        </>
      )}
    </section>
  )
}


/** Current Node conditions whose complete evidence is in Diagnostics. Health
 *  and freshness are already in the header dimensions and are not repeated. */
function nodeDetailNotices(node: AdminNodeDetailDto): DetailNotice[] {
  const notices: DetailNotice[] = []
  if (node.identity.state === 'mismatched') {
    notices.push({
      key: 'identity',
      tone: 'error',
      status: 'Mismatched',
      message:
        'Network identity contradicts the Registry' +
        (node.identity.mismatched_fields.length > 0 ? ' on ' + node.identity.mismatched_fields.join(', ') : '') +
        '; new history is not merged.',
      section: 'diagnostics',
      sectionLabel: 'Diagnostics',
    })
  } else if (node.identity.state !== 'matched') {
    notices.push({
      key: 'identity',
      tone: 'neutral',
      status: 'Unknown',
      message: 'Network identity has not been reported yet.',
      section: 'diagnostics',
      sectionLabel: 'Diagnostics',
    })
  }
  if (node.block_history_rejection) {
    notices.push({
      key: 'history-rejected',
      tone: 'error',
      status: 'History rejected',
      message: "The Server is refusing this Node's Block history.",
      section: 'diagnostics',
      sectionLabel: 'Diagnostics',
    })
  }
  if (node.rpc?.state === 'error') {
    notices.push({
      key: 'rpc',
      tone: 'error',
      status: 'Error',
      // Only a value that was once observed can remain; never imply one.
      message:
        'RPC collection failed' +
        (node.rpc.error_message ? ': ' + node.rpc.error_message : '') +
        (node.rpc.observed_at || node.rpc.client_version ? '; last-good values remain.' : '.'),
      section: 'diagnostics',
      sectionLabel: 'Diagnostics',
    })
  }
  // Not evaluated is not unresolved (webui.md §15.20): only an evaluation
  // that did not establish an identity is a finding.
  if (node.validator_identity && node.validator_identity.state !== 'not_evaluated' && validatorIdentityNeedsAttention(node.validator_identity)) {
    const notice = validatorIdentityNotice(node.validator_identity!)
    notices.push({
      key: 'validator',
      tone: notice.tone === 'error' ? 'error' : notice.tone === 'warning' ? 'warning' : 'neutral',
      status: notice.label,
      message: 'Automatic Validator identity is not established.',
      section: 'diagnostics',
      sectionLabel: 'Diagnostics',
    })
  }
  return notices
}

/** The always-visible Node header: name and context, the independent
 *  Server-owned dimensions, and the current conditions that need a look. */
function NodeDetailHeader({
  node,
  notices,
  section,
  onSection,
}: {
  node: AdminNodeDetailDto
  notices: DetailNotice[]
  section: DetailSection
  onSection: (next: DetailSection) => void
}) {
  const identity = identityBadge(node.identity)
  const lifecycle = lifecycleLabel(node.lifecycle)
  return (
    <header className="grid min-w-0 gap-3" data-slot="node-detail-header">
      <DetailBackLink to="/admin/nodes">All Nodes</DetailBackLink>
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-x-4 gap-y-2">
        <div className="min-w-0 flex-[1_1_18rem] space-y-1">
          <h1 className={ADMIN_PAGE_TITLE}>{node.display_name ?? shortId(node.node_id)}</h1>
          <p className={cn('flex min-w-0 flex-wrap items-center gap-x-1', ADMIN_SECONDARY)}>
            <Link className={cn(DETAIL_LINK, '-my-3 text-foreground')} to={'/admin/networks/' + node.network_key}>
              {node.network_display_name}
            </Link>
            <span aria-hidden="true">·</span>
            <span>Agent</span>
            <Link className={cn(DETAIL_LINK, '-my-3 text-foreground')} to={'/admin/agents/' + node.agent_id}>
              {shortId(node.agent_id)}
            </Link>
          </p>
          <AdminIdentifier value={node.node_id} label="Node ID" fullSlot="node-full-id" />
        </div>
        <nav aria-label="Node links" className="flex flex-wrap items-center gap-x-3 text-sm">
          <Link
            className={DETAIL_LINK}
            to={`/admin/alerts/incidents?subject=node&subject_key=${encodeURIComponent(node.node_id)}`}
          >
            Incidents for this Node
          </Link>
          <Link className={DETAIL_LINK} to={`/admin/nodes/${encodeURIComponent(node.node_id)}/investigation`}>
            Investigate this Node
          </Link>
        </nav>
      </div>
      <DetailDimensions label="Current Node dimensions">
        <DetailDimension label="Health">
          <span className="flex flex-wrap items-center gap-2">
            <StatusBadge status={node.health} tone={healthTone(node.health)} />
            {node.health !== 'healthy' && <span className={ADMIN_SECONDARY}>{node.health_reason}</span>}
          </span>
        </DetailDimension>
        <DetailDimension label="Freshness">
          <StatusBadge status={freshnessLabel(node.freshness)} tone={freshnessTone(node.freshness)} />
        </DetailDimension>
        <DetailDimension label="Identity">
          <StatusBadge status={identity.label} tone={identity.tone} />
        </DetailDimension>
        <DetailDimension label="Lifecycle">
          <StatusBadge status={lifecycle.label} tone={lifecycle.tone} />
        </DetailDimension>
        <DetailDimension label="Current head">
          <span className="font-medium tabular-nums">{node.current_head ?? 'Unknown'}</span>
        </DetailDimension>
      </DetailDimensions>
      <DetailNotices
        label="Current Node findings"
        notices={notices}
        current={section}
        onOpen={onSection}
      />
    </header>
  )
}

/** Manage: the Server-owned display name. The Agent Inventory, endpoint, and
 *  Node ID are not editable here. */
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
  useReportDraft(
    'node-display-name',
    editing && (confirming || displayName !== (node.display_name ?? '')),
  )

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
      header={<h2 className={ADMIN_SECTION_TITLE}>Server-owned metadata</h2>}
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
                    <span className="text-xs text-muted-foreground">
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
        <DetailItem label="Audit trail">
          Every mutation is recorded in the Server Audit log.{' '}
          <Link className={DETAIL_LINK} to="/admin/access/audit">
            Open the Audit log
          </Link>
        </DetailItem>
      </dl>
    </CardX>
  )
}

/** Overview: Node Inventory/lifecycle (CONTEXT.md: Active Node / Retired
 * Node) and the Node's context. The latest valid Agent Inventory decides
 * Active vs Retired; this is not Agent liveness or Node health, and the
 * Server never changes it remotely. */
function LifecyclePanel({ node }: { node: AdminNodeDetailDto }) {
  const lifecycle = lifecycleLabel(node.lifecycle)
  const known = lifecycle.label === 'Active' || lifecycle.label === 'Retired'
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={
        <>
          <h2 className={ADMIN_SECTION_TITLE}>Node Inventory &amp; lifecycle</h2>
          <StatusBadge status={lifecycle.label} tone={lifecycle.tone} />
        </>
      }
    >
      <DetailList>
        <DetailItem label="Lifecycle">
          {lifecycle.label}{' '}
          <span className="text-muted-foreground">· Inventory revision {node.inventory_revision}</span>
        </DetailItem>
        <DetailItem label="Lifecycle guidance">
          {known ? node.lifecycle_guidance : 'No lifecycle disposition has been observed yet.'}
        </DetailItem>
        <DetailItem label="Agent">
          <Link className={DETAIL_LINK} to={'/admin/agents/' + node.agent_id}>
            {shortId(node.agent_id)}
          </Link>
        </DetailItem>
        <DetailItem label="Network">
          <Link className={DETAIL_LINK} to={'/admin/networks/' + node.network_key}>
            {node.network_display_name}
          </Link>{' '}
          <span className="text-muted-foreground">({node.network_key})</span>
        </DetailItem>
        <DetailItem label="First seen">{formatObservedAt(node.first_seen_at)}</DetailItem>
        <DetailItem label="Metadata updated">{formatObservedAt(node.updated_at)}</DetailItem>
      </DetailList>
      <p className="mt-3 text-xs text-muted-foreground">
        Lifecycle follows the latest valid Agent Inventory; it is not Agent liveness or Node
        health, and the Server never changes it remotely.
      </p>
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
          <h2 className={ADMIN_SECTION_TITLE}>Chain head and data</h2>
        </>
      }
    >
      {/* Health and Freshness are the header's own dimensions above. */}
      <div>
        <DetailList>
          <DetailItem label="Node data size">
            <span className="font-bold leading-none tracking-tight">
              {formatBytes(node.data_directory?.size_bytes)}
            </span>
            {node.data_directory?.state && (
              <span className="text-xs font-medium text-muted-foreground">
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
            <span className="text-xs font-medium text-muted-foreground">
              {' '}· {node.network_reference_confidence}
            </span>
          </DetailItem>
        </DetailList>
      </div>
    </CardX>
  )
}

/**
 * PAGE-ADMIN-NODES and PAGE-ADMIN-NODE-DETAIL (design §4.3, §8.2, §11.4, §11.6; webui.md §8.2):
 * the Node's OWN Process series, charted through the shared metric-history body so this Node's
 * Process and the Host series it shares with its siblings are answered by one implementation.
 */
const NODE_PROCESS_INTRO =
  'Stored observations for one Node Process series, with the delay between observing and receiving each one, the aggregate buckets that answer the stretches older than the raw window, the silences between them, and the series state that outlives them all. Every value is evidence the Server still holds: a stored observation, or a bucket that kept the count, the minimum and the maximum of the observations it summarizes, never a value averaged over an interval the Server cannot restate.'

function MetricHistoryPanel({ node }: { node: AdminNodeDetailDto }) {
  const { generation } = useAuth()
  const [metric, setMetric] = useState<NodeMetricKey>('process_cpu_percent')
  const history = useMetricHistoryWindow(24)
  const definition = nodeMetricDefinition(metric)
  const query = useAdminNodeMetricHistory(
    generation,
    node.node_id,
    metric,
    history.range.from,
    history.range.to,
    history.olderThan ?? undefined,
  )
  return (
    <MetricHistoryBody
      title="Metric history"
      subject="Node"
      surface="node-process"
      intro={NODE_PROCESS_INTRO}
      definitions={NODE_METRIC_SERIES}
      selection={{
        metric,
        definition,
        onMetric(next) {
          history.setOlderThan(null)
          setMetric(next as NodeMetricKey)
        },
      }}
      view={history}
      answer={query}
    />
  )
}

/**
 * The recorded sync and consensus states of this Node (issue #217, design §11.4, §11.7): what the
 * Server actually recorded, when it recorded it, and what it could not record. A state is evidence
 * rather than a value: an unchanged state is re-recorded as an anchor at most once per anchor
 * interval, a failed probe is recorded as a failure instead of a false flag, a retained value keeps
 * the instant it was really observed at, and a stretch nobody observed is a silence with its own
 * kind and reason rather than a constant state carried across it.
 */
const NODE_STATE_INTRO =
  'Recorded sync and consensus states of this Node, as the Server recorded them: one row per recorded state, the age of the value each row refers to, the failure where a probe did not succeed instead of a flag it could not read, an anchor row where an unchanged state was re-recorded, and every silence with its kind, its reason and the observations it swallowed. Nothing here is inferred: a state the Server never recorded is shown as absent rather than as a value, and the silence after the newest state is not reported as a gap.'

function SyncConsensusHistoryPanel({ node }: { node: AdminNodeDetailDto }) {
  const { generation } = useAuth()
  const [component, setComponent] = useState<StateHistoryComponent>('sync')
  const history = useMetricHistoryWindow(24)
  const query = useAdminNodeStateHistory(
    generation,
    node.node_id,
    component,
    history.range.from,
    history.range.to,
    history.olderThan ?? undefined,
  )
  return (
    <StateHistoryBody
      title="Sync and consensus history"
      subject="Node"
      surface="node-state"
      intro={NODE_STATE_INTRO}
      component={component}
      onComponent={(next) => {
        history.setOlderThan(null)
        setComponent(stateHistoryComponent(next))
      }}
      view={history}
      answer={query}
    />
  )
}

/**
 * The Host series of the Agent this Node belongs to (issue #215, design §11.5): the machine's CPU,
 * physical memory, load averages, network rate and per-mount storage, stored once per Agent and
 * counted once per Agent. It is not the Node's Process series, so the two never share a chart, and
 * the Agent that owns the evidence is named on the card: a reading two Nodes show is never silent
 * about whose it is. A storage series exists once per mount path, so the Operator names the path the
 * Agent reported and a changed path is a different series rather than a continued one.
 */
const HOST_INTRO =
  "Stored observations of the machine the Agent behind this Node observes once for every Node it serves. The series belongs to the Agent, not to this Node: it is the same evidence here, on the Agent's page, and on every other Node of that Agent, and no Node's Purge removes it."

function HostMetricHistoryPanel({ node }: { node: AdminNodeDetailDto }) {
  const { generation } = useAuth()
  const history = useMetricHistoryWindow(24)
  const selection = useHostMetricSelection(history)
  // The route is the Node's, because the page is this Node's page; the answer
  // names the Agent whose shared series it is.
  const query = useAdminNodeHostMetricHistory(
    generation,
    node.node_id,
    selection.metric,
    selection.dimension,
    history.range.from,
    history.range.to,
    history.olderThan ?? undefined,
  )
  return (
    <MetricHistoryBody
      title="Host metric history"
      subject="Host"
      surface="node-host"
      intro={HOST_INTRO}
      definitions={HOST_METRIC_SERIES}
      selection={selection}
      extraControl={
        selection.needsMount ? (
          <HostMountPathControl
            value={selection.mountPath}
            onChange={selection.onMountPath}
          />
        ) : undefined
      }
      note={
        <p
          className="mt-2 text-xs text-muted-foreground"
          data-slot="host-metric-history-owner"
        >
          The Agent <span className="break-all font-mono">{node.agent_id}</span> reports
          this Host once for every Node it observes, so this is the same series here and
          on its sibling Nodes; this Node's own Purge does not remove it.
        </p>
      }
      promptWhenUnasked="Enter the mount path the Agent reported to read one storage series."
      view={history}
      answer={query}
      unasked={!selection.asked}
    />
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
          <h2 className={ADMIN_SECTION_TITLE}>Network identity</h2>
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
      {/* The Server's own refusal diagnosis (ADR 0011 decision 4, issue #235):
          the reason is rendered verbatim, and the panel says outright that it
          can be present while the observation above matches the Registry, so
          "identity matched, alert resolved, history still rejected" never
          reads as a contradiction the UI cannot explain. */}
      {node.block_history_rejection && (
        <div className="mt-2 space-y-2 text-sm" role="alert" data-slot="block-history-rejection">
          <p className="flex flex-wrap items-center gap-2">
            <StatusBadge status="History rejected" tone="error" /> The Server is refusing
            this Node's Block history.
          </p>
          {/* The reason names stamped/registered values such as a 66-character
              genesis hash, so it must wrap instead of widening the viewport. */}
          <p className="break-words text-muted-foreground">
            {node.block_history_rejection.reason}
          </p>
          <DetailList>
            <DetailItem label="Refused heights">
              <span className="tabular-nums">
                {node.block_history_rejection.rejected_heights.toLocaleString()}
              </span>{' '}
              heights, {node.block_history_rejection.from_height.toLocaleString()} to{' '}
              {node.block_history_rejection.to_height.toLocaleString()}
            </DetailItem>
            <DetailItem label="Most recent refusal">
              {formatObservedAt(node.block_history_rejection.recorded_at)}
            </DetailItem>
          </DetailList>
          {node.identity.state === 'matched' && (
            <p className="text-muted-foreground">
              The observation above matches the Registry, so the refusal is about the Block
              Identity Stamp the Agent carries for this Node, not about the Registry.
            </p>
          )}
          <p className="text-muted-foreground">
            See &quot;When the observation matches but the Block history is still
            rejected&quot; in docs/deployment.md.
          </p>
        </div>
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
            <code className="break-all text-xs">
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
  useReportDraft('node-purge', confirming)

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
      className={cn(CARD_SURFACE, 'ring-1 ring-destructive/25')}
      data-slot="danger-zone"
      header={<h2 className={ADMIN_SECTION_TITLE}>Danger zone</h2>}
    >
      <h3 className="text-sm font-medium">Permanent deletion</h3>
      <p className="mt-1 text-sm text-muted-foreground">
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

/**
 * Node entry to the Validator dimension (issue #218, design §15.4): the
 * automatic Network plus full-P2P-key identity the Server resolved for this
 * Node, what an unresolved evaluation could not establish, and the chain
 * Validator it belongs to. An unresolved state is Unknown rather than
 * absence, and the Public projection is not assumed to match this one: a
 * resolved identity still requires an Active Node before Public shows the
 * association.
 */
function ValidatorIdentityPanel({ node }: { node: AdminNodeDetailDto }) {
  const identity = node.validator_identity ?? null
  const notice = identity
    ? validatorIdentityNotice(identity)
    : {
        label: 'Not evaluated',
        tone: 'neutral' as const,
        description:
          'No automatic Validator identity evaluation is recorded for this Node yet.',
      }
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={
        <>
          <h2 className={ADMIN_SECTION_TITLE}>Validator identity</h2>
          <StatusBadge status={notice.label} tone={notice.tone} />
        </>
      }
    >
      <p className="text-sm text-muted-foreground">
        The correspondence is resolved automatically from this Node's Network and the full P2P
        public key its Agent reported: a chain identity, never an ownership claim, a manual role,
        or consensus membership. An unresolved identification state stays Unknown instead of
        absence, and a PlatScan deployment that cannot be read never becomes a negative verdict.
      </p>
      <p className="mt-2 flex flex-wrap items-center gap-2 text-sm">
        <StatusBadge status={notice.label} tone={notice.tone} />
        <span className="min-w-0 break-words">{notice.description}</span>
      </p>
      <div className="mt-3">
        <DetailList>
          <DetailItem label="State">
            {identity ? notice.label : 'Not evaluated'}
          </DetailItem>
          <DetailItem label="Observed P2P public key" wide>
            {identity?.observedValidatorNodeKey ? (
              <code
                className="select-all font-mono text-xs [overflow-wrap:anywhere]"
                title={identity.observedValidatorNodeKey}
              >
                {identity.observedValidatorNodeKey}
              </code>
            ) : (
              <code className="text-xs">{validatorKeyLabel(identity?.observedValidatorNodeKey)}</code>
            )}
          </DetailItem>
          <DetailItem label="Chain Validator">
            {identity?.validatorId ? (
              <Link
                className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
                to={'/admin/validators/' + identity.validatorId}
              >
                {validatorKeyLabel(identity.validatorNodeKey)}
              </Link>
            ) : identity ? (
              'Not established'
            ) : (
              'Not evaluated'
            )}
          </DetailItem>
          <DetailItem label="Automatic Link">
            {identity?.validatorId
              ? identity.associationEffective
                ? 'Open, and shown in the Public projection'
                : 'Open, but the Node is not Active so Public shows no association'
              : 'No automatic Link to project'}
          </DetailItem>
          <DetailItem label="Evaluated at">
            {identity?.evaluatedAt ? formatObservedAt(identity.evaluatedAt) : 'Never evaluated'}
          </DetailItem>
          <DetailItem label="Lifecycle">
            {identity ? lifecycleLabel(identity.lifecycle).label : 'Unknown'}
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
          <h2 className={ADMIN_SECTION_TITLE}>RPC diagnostics</h2>
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
            <code className="break-all text-xs">{node.rpc_endpoint}</code>
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

/**
 * Per-component collection evidence for this Node (moved here from the
 * Overview's former in-place disclosure, webui.md §8.7): collection state,
 * the safe error, the last-good value and its time, and the attempt time for
 * RPC, Sync, Consensus, Peers, Process and Node Data. Values are the Server's
 * own; a component never observed reads Never observed, not zero.
 */
function ComponentDiagnosticsPanel({ node }: { node: AdminNodeDetailDto }) {
  const lastGood = (state: string | null | undefined) => (state === 'error' ? 'last-good ' : '')
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      data-slot="component-diagnostics"
      header={<h2 className={ADMIN_SECTION_TITLE}>Component diagnostics</h2>}
    >
      <dl className="divide-y divide-border/60">
        <ComponentRow
          label="RPC"
          component={node.rpc}
          detail={node.rpc?.client_version ? lastGood(node.rpc.state) + node.rpc.client_version + ' · ' + node.rpc.namespaces.length + ' namespaces' : undefined}
        />
        <ComponentRow
          label="Sync"
          component={node.sync}
          detail={
            node.sync?.current_block != null
              ? lastGood(node.sync.state) + 'head ' + node.sync.current_block +
                (node.sync.highest_block != null ? ' · highest ' + node.sync.highest_block : '')
              : undefined
          }
        />
        <ComponentRow
          label="Consensus"
          component={node.consensus}
          detail={node.consensus?.highest_commit_block != null ? lastGood(node.consensus.state) + 'commit ' + node.consensus.highest_commit_block : undefined}
        />
        <ComponentRow
          label="Peers"
          component={node.peers}
          detail={node.peers?.peer_count != null ? lastGood(node.peers.state) + node.peers.peer_count + ' peers · ' + node.peers.freshness : undefined}
        />
        <ComponentRow
          label="Process"
          component={node.process}
          detail={node.process?.pid != null ? lastGood(node.process.state) + 'pid ' + node.process.pid : undefined}
        />
        <ComponentRow
          label="Node Data"
          component={node.data_directory}
          detail={node.data_directory?.size_bytes != null ? lastGood(node.data_directory.state) + formatBytes(node.data_directory.size_bytes) : undefined}
        />
      </dl>
    </CardX>
  )
}

type ComponentEvidence = {
  state?: string | null
  error_message?: string | null
  observed_at?: string | null
  attempted_at?: string | null
  received_at?: string | null
}

function ComponentRow({
  label,
  component,
  detail,
}: {
  label: string
  component: ComponentEvidence | null | undefined
  detail?: string
}) {
  const state = component?.state
  const tone = state === 'error' ? 'error' : state === 'ok' ? 'ok' : 'neutral'
  return (
    <div className="grid grid-cols-1 gap-1 py-2 sm:grid-cols-[minmax(7rem,0.3fr)_minmax(0,1fr)] sm:gap-2">
      <dt className="text-xs font-medium text-muted-foreground">{label}</dt>
      <dd className="m-0 min-w-0 text-sm [overflow-wrap:anywhere]">
        <StatusBadge status={componentStateLabel(state)} tone={tone} />
        {state === 'error' && component?.error_message && (
          <span className="font-semibold text-destructive"> {component.error_message}</span>
        )}
        {detail && <span className="text-muted-foreground"> {detail}</span>}
        <small className="block text-xs text-muted-foreground">
          {state === 'error'
            ? component?.observed_at
              ? 'Last good · ' + formatObservedAt(component.observed_at)
              : 'Never observed'
            : formatObservedAt(component?.observed_at)}
          {state === 'error' && (component?.attempted_at ?? component?.received_at)
            ? ' · Attempted ' + formatObservedAt(component?.attempted_at ?? component?.received_at)
            : ''}
        </small>
      </dd>
    </div>
  )
}
