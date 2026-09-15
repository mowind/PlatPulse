import { useEffect, useRef, useState, type KeyboardEvent } from 'react'
import { Link } from 'react-router'
import {
  useAdminDiagnostics,
  useAdminNodes,
  useAdminOverview,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { formatBytes, formatBytesUnknown, formatBytesPerSecond, formatIdentifier, formatPercent } from '../formatBytes'
import { hasSpoolRisk, livenessTone, receiptTimeText } from '../agentDiagnostics'
import { freshnessTone, healthTone } from '../nodeLabels'
import {
  StatusBadge,
  componentStateLabel,
  formatObservedAt,
  freshnessLabel,
  livenessLabel,
} from '../components/StatusBadge'
import { Alert, AlertDescription } from '../components/ui/alert'
import { Badge } from '../components/ui/badge'
import { Button } from '../components/ui/button'
import { DataTooltip } from '../components/ui/data-tooltip'
import { Empty } from '../components/ui/empty'
import { SURFACE_CARD } from '../lib/surface'
import { cn } from '../lib/utils'
import type {
  AdminOverview,
  AgentDiagnostic,
  AttentionItem,
  AdminNodeListItem,
  NodeDiagnostic,
} from '../api/generated'

/** Emerald's card surface, applied to every Overview panel. Upstream passes
 * border-none and leans on the surface recipe plus the hover glow. */
const PANEL = cn(
  'min-w-0 rounded-md border-none p-4 text-foreground transition-all',
  SURFACE_CARD,
  'hover:shadow-[0_0_20px,0_0_0_1px] hover:shadow-emerald-600/10',
)
const PANEL_HEADING = 'flex min-w-0 items-start justify-between gap-3 border-b pb-3'
const PANEL_TITLE = 'text-sm font-medium'
const EYEBROW = 'text-xs font-medium tracking-wider text-muted-foreground uppercase'
const PANEL_STATE = 'flex min-w-0 flex-wrap items-center gap-2 text-sm text-muted-foreground'
const TABLE_HEAD = 'whitespace-nowrap border-b px-3 py-2 text-left text-xs font-medium text-muted-foreground'
const TABLE_CELL = 'min-w-0 border-b border-border/60 px-3 py-3 align-top text-sm [overflow-wrap:anywhere]'
const MUTED_SMALL = 'mt-1 block text-[11px] text-muted-foreground'
const TEXT_LINK = 'inline-flex min-h-11 min-w-11 items-center font-medium text-primary hover:underline'
const SUMMARY_ARROW: Record<string, string> = {
  violet: 'text-violet-600 dark:text-violet-400',
  green: 'text-emerald-600 dark:text-emerald-400',
  slate: 'text-muted-foreground',
  red: 'text-destructive',
}

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
    <section data-slot="admin-overview" className="mx-auto flex w-full min-w-0 max-w-[1280px] flex-col gap-4 pb-12">
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
    <header
      data-slot="overview-header"
      className="flex min-w-0 flex-col gap-3 border-b pb-4 md:flex-row md:items-end md:justify-between md:gap-x-8"
    >
      <div className="min-w-0">
        <span className={EYEBROW}>Owner triage</span>
        <h1 className="mt-1 text-2xl font-semibold break-words">Overview</h1>
        <p className="mt-2 max-w-2xl text-sm text-muted-foreground">
          A compact read on what needs intervention across your PlatON estate.
        </p>
      </div>
      <div
        data-slot="header-status"
        className="flex flex-wrap items-center gap-2 text-xs font-medium text-muted-foreground md:justify-end md:text-right"
      >
        {snapshot ? (
          <>
            <span>
              Last good snapshot · <RelativeTime timestamp={snapshot.generated_at} />
            </span>
            <Button
              variant="outline"
              size="sm"
              onClick={() => void onRefresh()}
              disabled={refreshing}
            >
              {query.isFetching ? 'Refreshing…' : 'Refresh'}
            </Button>
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

/** Relative rendering for one Server-owned timestamp. The caller supplies
 * the semantic label (snapshot generation vs last observation); the component
 * never substitutes a different clock source. */
function RelativeTime({ timestamp }: { timestamp: string }) {
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
    <aside data-slot="setup-guide" className={PANEL} aria-labelledby="setup-guide-title">
      <div className={PANEL_HEADING}>
        <div className="min-w-0">
          <span className={EYEBROW}>Next steps</span>
          <h2 id="setup-guide-title" className={PANEL_TITLE}>Set up your first observation</h2>
        </div>
      </div>
      <p className="mt-3 text-sm text-muted-foreground">
        There are no Agents, Nodes, or Networks yet. Complete these steps to begin receiving authoritative observations.
      </p>
      <ol className="mt-2 list-decimal space-y-1 pl-5 text-sm text-muted-foreground">
        <li><Link className={TEXT_LINK} to="/admin/networks">Register the expected Network identity</Link>.</li>
        <li>Provision and start an Agent.</li>
        <li><Link className={TEXT_LINK} to="/admin/settings">Configure the Agent's local Node Inventory</Link>.</li>
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
    <nav aria-label="Overview summaries" className="grid grid-cols-2 gap-2 md:grid-cols-4 md:gap-3">
      {cards.map((card) => (
        <Link
          key={card.label}
          data-slot="summary-card"
          data-accent={card.accent}
          to={card.href}
          className={cn(
            'flex min-h-[7.5rem] min-w-0 flex-col justify-between gap-2 rounded-md p-3 text-foreground no-underline transition-all',
            SURFACE_CARD,
            'hover:-translate-y-0.5 hover:shadow-[0_0_20px,0_0_0_1px] hover:shadow-emerald-600/10',
          )}
        >
          <span className={EYEBROW}>{card.label}</span>
          <strong className="text-2xl font-bold leading-none tracking-tight tabular-nums md:text-3xl">
            {card.value}
          </strong>
          <span className="text-[11px] leading-snug break-words text-muted-foreground">
            {card.legend}
          </span>
          <span aria-hidden="true" className={cn('text-base', SUMMARY_ARROW[card.accent])}>
            ↗
          </span>
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
    <article data-slot="overview-panel" className={PANEL}>
      <div className={PANEL_HEADING}>
        <div className="min-w-0">
          <span className={EYEBROW}>01 · Attention</span>
          <h2 className={PANEL_TITLE}>Attention queue</h2>
        </div>
        {data && <Badge variant="secondary" className="tabular-nums">{data.attention.length}</Badge>}
      </div>
      <p className="sr-only" role="status">{announcement}</p>
      {!data && query.isPending && (
        <p className={cn(PANEL_STATE, 'mt-3')} role="status">
          <StatusBadge status="Starting" tone="neutral" /> Checking the Server for attention…
        </p>
      )}
      {!data && query.isError && (
        <Alert variant="destructive" className="mt-3">
          <AlertDescription className="flex flex-wrap items-center gap-2">
            <StatusBadge status="Error" tone="error" />{' '}
            {query.error instanceof Error ? query.error.message : 'Unable to load attention'}
            <Button variant="link" size="sm" onClick={() => void query.refetch()}>Try again</Button>
          </AlertDescription>
        </Alert>
      )}
      {data && query.isRefetchError && (
        <Alert variant="destructive" className="mt-3">
          <AlertDescription className="flex flex-wrap items-center gap-2">
            <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
            successful attention queue.
            <Button variant="link" size="sm" onClick={() => void query.refetch()}>Try again</Button>
          </AlertDescription>
        </Alert>
      )}
      {data && data.attention.length === 0 && (
        <Empty description="No attention items. Nothing needs an Owner right now." />
      )}
      {data && data.attention.length > 0 && (
        <>
          <p className="mt-3 text-xs text-muted-foreground">
            {data.attention.length} items across {groups.length} subjects · {criticalCount} Critical
          </p>
          <ul className="mt-2 grid list-none gap-2 p-0">
            {visibleGroups.map((group) => (
              <AttentionGroup
                key={group.key}
                group={group}
                expanded={expanded.has(group.key)}
                onToggle={() => setExpanded((current) => {
                  const next = new Set(current)
                  if (next.has(group.key)) {
                    next.delete(group.key)
                  } else {
                    next.add(group.key)
                  }
                  return next
                })}
              />
            ))}
          </ul>
          {hiddenCount > 0 && (
            <Button
              variant="outline"
              size="sm"
              className="mt-3"
              onClick={() => setExpanded((current) => {
                const next = new Set(current)
                if (next.has('__all__')) {
                  next.delete('__all__')
                } else {
                  next.add('__all__')
                }
                return next
              })}
            >
              {showAll ? 'Collapse' : `Show ${hiddenCount} more`}
            </Button>
          )}
        </>
      )}
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

/** One Server-supplied observation time, explicitly labelled. A missing
 * timestamp stays Unknown; the snapshot generation time is never substituted. */
function attentionObservedText(observedAt: string | null | undefined) {
  return observedAt ? (
    <>
      Last observed <RelativeTime timestamp={observedAt} />
    </>
  ) : (
    'Observation time unknown'
  )
}

function AttentionGroup({ group, expanded, onToggle }: { group: AttentionGroupData; expanded: boolean; onToggle: () => void }) {
  const primary = group.items.reduce((best, item) => {
    const rank = item.severity === 'critical' ? 0 : item.severity === 'warning' ? 1 : 2
    const bestRank = best.severity === 'critical' ? 0 : best.severity === 'warning' ? 1 : 2
    return rank < bestRank ? item : best
  })
  const known = primary.severity === "critical" || primary.severity === "warning"
  const severity = known ? primary.severity : "unknown"
  const route = safeAttentionRoute(group)
  const additional = group.items.filter((item) => item.id !== primary.id)
  return (
    <li
      data-slot="attention-item"
      data-severity={severity}
      className={cn(
        'flex min-w-0 items-start gap-3 rounded-md border border-border/60 border-l-[3px] bg-background/60 p-3',
        severity === 'critical' && 'border-l-destructive',
        severity === 'warning' && 'border-l-warning',
        severity === 'unknown' && 'border-l-muted-foreground',
      )}
    >
      <StatusBadge
        status={severity === 'critical' ? 'Critical' : severity === 'warning' ? 'Warning' : 'Unknown'}
        tone={severity === 'critical' ? 'error' : severity === 'warning' ? 'warning' : 'neutral'}
      />
      <div className="min-w-0 flex-1 space-y-1">
        <p className="break-words">
          <strong>{route ? <Link className={cn(TEXT_LINK, 'font-semibold')} to={route}>{group.label}</Link> : group.label}</strong> — {primary.message}
        </p>
        <p className="break-words text-[11px] text-muted-foreground">
          {primary.kind} · {attentionObservedText(primary.observed_at)}
        </p>
        {additional.length > 0 && (
          <>
            <Button
              variant="link"
              size="sm"
              className="px-0 max-w-full whitespace-normal"
              aria-expanded={expanded}
              aria-controls={`attention-details-${group.key}`}
              onClick={onToggle}
            >
              {expanded ? 'Hide additional issues' : `Show ${additional.length} additional issue${additional.length === 1 ? '' : 's'}`}
            </Button>
            <ul
              id={`attention-details-${group.key}`}
              hidden={!expanded}
              className="mt-2 list-disc space-y-1 pl-4 text-[11px] break-words text-muted-foreground"
            >
              {additional.map((item) => (
                <li key={item.id}>
                  {item.severity === 'critical' ? 'Critical' : item.severity === 'warning' ? 'Warning' : 'Unknown'} ·{' '}
                  {item.message} · {item.kind} · {attentionObservedText(item.observed_at)}
                </li>
              ))}
            </ul>
          </>
        )}
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
    <article data-slot="overview-panel" className={PANEL}>
      <div className={PANEL_HEADING}>
        <h2 className={PANEL_TITLE}>Node Health Summary</h2>
        {activeNodes.length > 0 && <Badge variant="secondary" className="tabular-nums">{activeNodes.length}</Badge>}
      </div>
      {!nodeQuery.data && nodeQuery.isPending && (
        <p className={cn(PANEL_STATE, 'mt-3')} role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Node Health Summary…
        </p>
      )}
      {!nodeQuery.data && nodeQuery.isError && (
        <Alert variant="destructive" className="mt-3">
          <AlertDescription className="flex flex-wrap items-center gap-2">
            <StatusBadge status="Error" tone="error" />{' '}
            {nodeQuery.error instanceof Error ? nodeQuery.error.message : 'Unable to load Nodes'}
            <Button variant="link" size="sm" onClick={() => void nodeQuery.refetch()}>
              Try again
            </Button>
          </AlertDescription>
        </Alert>
      )}
      {nodeQuery.data && nodeQuery.isRefetchError && (
        <Alert variant="destructive" className="mt-3">
          <AlertDescription className="flex flex-wrap items-center gap-2">
            <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
            successful Node values.
            <Button variant="link" size="sm" onClick={() => void nodeQuery.refetch()}>Try again</Button>
          </AlertDescription>
        </Alert>
      )}
      {nodeQuery.data && activeNodes.length === 0 && <Empty description="No Nodes observed yet." />}
      {nodeQuery.data && activeNodes.length > 0 && (
        <div className="mt-3 w-full min-w-0 overflow-x-auto">
          <table data-stack className="w-full table-fixed border-collapse text-sm md:min-w-[48rem] md:table-auto">
            <caption className="sr-only">PlatON Node health, freshness, and sync</caption>
            <thead>
              <tr>
                <th scope="col" className={TABLE_HEAD}>Node</th>
                <th scope="col" className={TABLE_HEAD}>Network</th>
                <th scope="col" className={TABLE_HEAD}>Health</th>
                <th scope="col" className={TABLE_HEAD}>Freshness</th>
                <th scope="col" className={TABLE_HEAD}>Head / Sync</th>
                <th scope="col" className={TABLE_HEAD}>Resync</th>
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
        <div className="mt-3 flex flex-wrap items-center justify-between gap-2 border-t pt-3">
          <span className="text-sm text-muted-foreground">Showing {visibleNodes.length} of {activeNodes.length} Active Nodes</span>
          <Link className={TEXT_LINK} to="/admin/nodes">View all Nodes</Link>
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
  return (
    <>
      <tr className="border-b border-border/60" onKeyDown={collapseOnEscape}>
        <th scope="row" data-label="Node" className={cn(TABLE_CELL, 'text-left')}>
          <Button
            ref={toggleRef}
            variant="link"
            size="sm"
            className="h-auto min-w-11 max-w-full justify-start px-0 text-left font-semibold whitespace-normal md:max-w-[14rem]"
            aria-expanded={expanded}
            aria-controls={detailId}
            onClick={onToggle}
          >
            <span aria-hidden="true" className="shrink-0">{expanded ? '▾' : '▸'}</span>
            <span className="min-w-0 [overflow-wrap:anywhere]">{nodeLabel}</span>
          </Button>
          <small className={MUTED_SMALL} title={node.node_id}>
            Node ID · {formatIdentifier(node.node_id)}
          </small>
          <Link className={TEXT_LINK} to={`/admin/nodes/${encodeURIComponent(node.node_id)}`}>View Node</Link>
        </th>
        <td data-label="Network" className={TABLE_CELL}>
          <span className="break-words">{node.network_display_name}</span>
          <small className={MUTED_SMALL}>{node.network_key}</small>
        </td>
        <td data-label="Health" className={TABLE_CELL}>
          <StatusBadge status={node.health} tone={healthTone(node.health)} />
          <span className="mt-1 block text-xs break-words text-muted-foreground">{node.health_reason}</span>
        </td>
        <td data-label="Freshness" className={TABLE_CELL}>
          <StatusBadge status={freshnessLabel(node.freshness)} tone={freshnessTone(node.freshness)} />
          <DataTooltip as="span" className="block" content="Freshness is computed by the Server, not the browser.">
            <small className={MUTED_SMALL}>Server-owned freshness</small>
          </DataTooltip>
        </td>
        <td data-label="Head / Sync" className={TABLE_CELL}>
          <span className="break-words">{node.current_head ?? 'Unknown'}</span>
          <small className={MUTED_SMALL}>{syncSummary(diagnostic)}</small>
        </td>
        <td data-label="Resync" className={TABLE_CELL}>
          <span className="break-words">{node.resync_state}</span>
          {diagnostic?.resync_progress ? (
            <small className={MUTED_SMALL}>{diagnostic.resync_progress}</small>
          ) : null}
        </td>
      </tr>
      {expanded && (
        <tr data-slot="detail-row" className="border-b border-border/60">
          <td colSpan={6} id={detailId} onKeyDown={collapseOnEscape} className="p-0">
            <div className="m-1 mb-3 rounded-md border border-border/60 bg-muted/40 p-3">
              {!diagnostic && diagnosticsQuery.isPending && (
                <p className={PANEL_STATE} role="status">
                  <StatusBadge status="Starting" tone="neutral" /> Loading Node diagnostics…
                </p>
              )}
              {!diagnostic && diagnosticsQuery.isError && (
                <Alert variant="destructive">
                  <AlertDescription className="flex flex-wrap items-center gap-2">
                    <StatusBadge status="Error" tone="error" /> Node diagnostics are unavailable;
                    the summary above remains available.
                    <Button variant="link" size="sm" onClick={() => void diagnosticsQuery.refetch()}>
                      Try again
                    </Button>
                  </AlertDescription>
                </Alert>
              )}
              {diagnostic && (
                <dl className="divide-y divide-border/60">
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
                <p className={PANEL_STATE}>
                  <StatusBadge status="Unknown" tone="neutral" /> No current Agent diagnostic is
                  available for this Node; the Server-owned summary remains authoritative.
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
    <div className="grid grid-cols-1 gap-1 py-2 sm:grid-cols-[minmax(7rem,0.45fr)_minmax(0,1fr)] sm:gap-2">
      <dt className="text-xs font-medium tracking-wider text-muted-foreground">{label}</dt>
      <dd className="m-0 min-w-0 text-sm break-words">
        <StatusBadge status={componentStateLabel(state)} tone={tone} />
        {state === 'error' && errorMessage && (
          <span className="font-semibold text-destructive"> {errorMessage}</span>
        )}
        {detail && <span className="text-muted-foreground"> {detail}</span>}
        <small className="text-[11px] text-muted-foreground">
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
    <article data-slot="overview-panel" className={PANEL}>
      <div className={PANEL_HEADING}>
        <div className="min-w-0">
          <h2 className={PANEL_TITLE}>Agent inventory</h2>
          <p className="mt-1 text-xs text-muted-foreground">
            Compact Server-owned reporting, host, evidence, and retained Node summaries.
          </p>
        </div>
        {agents.length > 0 && <Badge variant="secondary" className="tabular-nums">{agents.length}</Badge>}
      </div>
      {!query.data && query.isPending && (
        <p className={cn(PANEL_STATE, 'mt-3')} role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading Agent state…
        </p>
      )}
      {!query.data && query.isError && (
        <Alert variant="destructive" className="mt-3">
          <AlertDescription className="flex flex-wrap items-center gap-2">
            <StatusBadge status="Error" tone="error" />{' '}
            {query.error instanceof Error ? query.error.message : 'Unable to load Agents'}
            <Button variant="link" size="sm" onClick={() => void query.refetch()}>
              Try again
            </Button>
          </AlertDescription>
        </Alert>
      )}
      {query.data && query.isRefetchError && (
        <Alert variant="destructive" className="mt-3">
          <AlertDescription className="flex flex-wrap items-center gap-2">
            <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
            successful Agent values.
            <Button variant="link" size="sm" onClick={() => void query.refetch()}>Try again</Button>
          </AlertDescription>
        </Alert>
      )}
      {query.data && agents.length === 0 && <Empty description="No Agents enrolled yet." />}
      {query.data && agents.length > 0 && (
        <div className="mt-3 w-full min-w-0 overflow-x-auto">
          {/* Keep Overview field order; the shared stack otherwise orders the full Agent inventory. */}
          <table data-stack className="w-full table-fixed border-collapse text-sm md:min-w-[52rem] [&_tbody_tr>*]:order-none">
            <caption className="sr-only">
              Agent inventory overview with reporting, receipt, host resources, evidence, and retained Node summaries
            </caption>
            <thead>
              <tr>
                <th scope="col" className={cn(TABLE_HEAD, 'md:w-[16%]')}>Agent</th>
                <th scope="col" className={cn(TABLE_HEAD, 'md:w-[13%]')}>Reporting</th>
                <th scope="col" className={cn(TABLE_HEAD, 'md:w-[15%]')}>Last received</th>
                <th scope="col" className={cn(TABLE_HEAD, 'md:w-[21%]')}>Host resources</th>
                <th scope="col" className={cn(TABLE_HEAD, 'md:w-[22%]')}>Evidence</th>
                <th scope="col" className={cn(TABLE_HEAD, 'md:w-[13%]')}>Nodes</th>
              </tr>
            </thead>
            <tbody>
              {visibleAgents.map((agent) => (
                <AgentOverviewRow
                  key={agent.agent_id}
                  agent={agent}
                  nodes={nodesByAgent.get(agent.agent_id)}
                  nodeQuery={nodeQuery}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
      {query.data && agents.length > 0 && (
        <div className="mt-3 flex flex-wrap items-center justify-between gap-2 border-t pt-3">
          <span className="text-sm text-muted-foreground">Showing {visibleAgents.length} of {agents.length} Agents</span>
          <Link className={TEXT_LINK} to="/admin/agents">View all Agents</Link>
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

function AgentOverviewRow({
  agent,
  nodes,
  nodeQuery,
}: {
  agent: AgentDiagnostic
  nodes?: AdminNodeListItem[]
  nodeQuery: NodesQuery
}) {
  const liveness = livenessLabel(agent.liveness)
  const host = agent.host
  const spoolRisk = hasSpoolRisk(host)
  const memory = host && (host.memory_used_bytes != null || host.memory_total_bytes != null)
    ? formatBytesUnknown(host.memory_used_bytes) + ' / ' + formatBytesUnknown(host.memory_total_bytes)
    : 'Unknown'
  return (
    <tr className={cn('border-b border-border/60 align-top', (spoolRisk || agent.security_event_count > 0) && 'bg-destructive/5')}>
      <th scope="row" data-label="Agent" className={cn(TABLE_CELL, 'text-left')}>
        <Link
          className={cn(TEXT_LINK, 'break-all')}
          aria-label="View Agent"
          to={'/admin/agents/' + encodeURIComponent(agent.agent_id)}
        >
          <code className="rounded-sm border bg-muted px-1 py-0.5 text-xs" title={agent.agent_id}>{formatIdentifier(agent.agent_id)}</code>
          <span className="sr-only">View Agent</span>
        </Link>
        <small className={MUTED_SMALL} title={agent.agent_id}>Agent ID · {agent.agent_id}</small>
      </th>
      <td data-label="Reporting" className={TABLE_CELL}>
        <StatusBadge status={liveness} tone={livenessTone(agent.liveness)} />
        <small className={MUTED_SMALL}>Server liveness</small>
      </td>
      <td data-label="Last received" className={TABLE_CELL}>
        {agent.last_received_at ? (
          <time dateTime={agent.last_received_at}>{formatObservedAt(agent.last_received_at)}</time>
        ) : (
          <span>{receiptTimeText(agent.last_received_at)}</span>
        )}
        <small className={MUTED_SMALL}>{agent.last_report_sequence == null ? 'Never received' : 'Report #' + agent.last_report_sequence}</small>
      </td>
      <td data-label="Host resources" className={TABLE_CELL}>
        <dl className="min-w-0 space-y-1">
          <div className="flex items-baseline justify-between gap-2"><dt className="text-[11px] text-muted-foreground">CPU</dt><dd className="m-0 min-w-0 font-medium break-words">{formatPercent(host?.cpu_percent)}</dd></div>
          <div className="flex items-baseline justify-between gap-2"><dt className="text-[11px] text-muted-foreground">Memory</dt><dd className="m-0 min-w-0 font-medium break-words">{memory}</dd></div>
          <div className="flex items-baseline justify-between gap-2"><dt className="text-[11px] text-muted-foreground">RX / TX</dt><dd className="m-0 min-w-0 font-medium break-words">{formatBytesPerSecond(host?.network_rx_bytes_per_sec)} / {formatBytesPerSecond(host?.network_tx_bytes_per_sec)}</dd></div>
        </dl>
        <small className={MUTED_SMALL}>{host ? 'Host snapshot · ' + formatObservedAt(host.updated_at) : 'No Host observation'}</small>
      </td>
      <td data-label="Evidence" className={TABLE_CELL}>
        <dl className="min-w-0 space-y-1">
          <div className="flex items-baseline justify-between gap-2"><dt className="text-[11px] text-muted-foreground">Report gaps</dt><dd className="m-0 min-w-0 font-medium break-words">{agent.sequence_gap_count} report gap{agent.sequence_gap_count === 1 ? '' : 's'}</dd></div>
          <div className="flex items-baseline justify-between gap-2"><dt className="text-[11px] text-muted-foreground">Security events</dt><dd className="m-0 min-w-0 font-medium break-words">{agent.security_event_count} security event{agent.security_event_count === 1 ? '' : 's'}</dd></div>
          <div className="flex items-baseline justify-between gap-2"><dt className="shrink-0 text-[11px] whitespace-nowrap text-muted-foreground">Spool</dt><dd className={cn('m-0 min-w-0 font-medium break-words', (spoolRisk || host?.spool_store_error) && 'font-semibold text-destructive')}>{formatSpoolSummary(host)}</dd></div>
          <div className="flex items-baseline justify-between gap-2"><dt className="text-[11px] text-muted-foreground">Clock</dt><dd className="m-0 min-w-0 font-medium break-words">{clockStatusLabel(agent.clock_status)}{agent.clock_skew_ms != null ? ' · ' + agent.clock_skew_ms + ' ms skew' : ''}</dd></div>
        </dl>
      </td>
      <td data-label="Nodes" className={TABLE_CELL}>
        <AgentNodeSummary nodes={nodes} query={nodeQuery} />
      </td>
    </tr>
  )
}

function AgentNodeSummary({ nodes, query }: { nodes?: AdminNodeListItem[]; query: NodesQuery }) {
  if (!query.data) {
    if (query.isPending) return <span className="text-muted-foreground">Loading Node context…</span>
    if (query.isError) return <span className="font-semibold text-destructive">Node context unavailable; recover in Nodes</span>
    return <span className="text-muted-foreground">Node context unavailable</span>
  }
  if (!nodes || nodes.length === 0) return <span>No Nodes observed yet.</span>
  const active = nodes.filter((node) => node.lifecycle === 'active').length
  const unhealthy = nodes.filter((node) => node.health === 'unhealthy').length
  const unknown = nodes.filter((node) => node.health === 'unknown').length
  return (
    <div className="min-w-0">
      <strong className="text-sm font-semibold">{nodes.length} retained Node{nodes.length === 1 ? '' : 's'}</strong>
      <small className={MUTED_SMALL}>{active} active · {unhealthy} unhealthy · {unknown} unknown</small>
    </div>
  )
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
