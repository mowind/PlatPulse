import { ArrowUpRight, LoaderCircle } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { Link } from 'react-router'
import {
  acknowledgeAgentAttention,
  useAdminDiagnostics,
  useAdminNodes,
  useAdminOverview,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { formatIdentifier } from '../formatBytes'
import { hasSpoolRisk, livenessTone, receiptTimeText } from '../agentDiagnostics'
import { agentEvidence } from '../agentEvidence'
import { freshnessTone, healthTone } from '../nodeLabels'
import {
  StatusBadge,
  freshnessLabel,
  livenessLabel,
} from '../components/StatusBadge'
import { Alert, AlertDescription } from '../components/ui/alert'
import { Badge } from '../components/ui/badge'
import { Button } from '../components/ui/button'
import { Empty } from '../components/ui/empty'
import { SURFACE_CARD, SURFACE_CARD_STATIC } from '../lib/surface'
import { cn } from '../lib/utils'
import type {
  AdminOverview,
  AgentDiagnostic,
  AttentionItem,
  AdminNodeListItem,
} from '../api/generated'
import { ADMIN_PAGE_TITLE, ADMIN_SECTION_TITLE } from '../components/AdminPage'

/** The read-only Overview panels are static: they carry the working surface
 * tier and no hover glow, opacity change, or lift, so a non-clickable panel
 * never reads as clickable (webui.md §11.1). Only the Overview summary links
 * below opt into interactive-card feedback. */
const PANEL = cn(
  'min-w-0 rounded-md border-none p-4 text-foreground',
  SURFACE_CARD_STATIC,
)
const PANEL_HEADING = 'flex min-w-0 items-center justify-between gap-3 border-b pb-3'
const PANEL_TITLE = ADMIN_SECTION_TITLE
const EYEBROW = 'text-xs font-medium tracking-wider text-muted-foreground uppercase'
const PANEL_STATE = 'flex min-w-0 flex-wrap items-center gap-2 text-sm text-muted-foreground'
const TEXT_LINK = 'inline-flex min-h-11 min-w-11 items-center font-medium text-primary hover:underline'

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
    <section data-slot="admin-overview" className="flex w-full min-w-0 flex-col gap-4 pb-12">
      <OverviewHeader snapshot={snapshot} query={overview} refreshing={overview.isFetching || diagnostics.isFetching || nodes.isFetching} onRefresh={refreshAll} />
      <AttentionPanel query={overview} />
      {snapshot && <SummaryCards summary={snapshot.summary} />}
      {/* Two compact previews side by side when each has a readable width,
          stacked otherwise; the full inventories stay on their own pages. */}
      <div
        data-slot="overview-previews"
        className="grid min-w-0 grid-cols-[repeat(auto-fit,minmax(min(100%,28rem),1fr))] items-start gap-4"
      >
        <NodePanel nodeQuery={nodes} />
        <AgentPanel query={diagnostics} nodeQuery={nodes} />
      </div>
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
      className="flex min-w-0 flex-wrap items-center justify-between gap-x-4 gap-y-2"
    >
      <div className="min-w-0">
        <h1 className={ADMIN_PAGE_TITLE}>Overview</h1>
      </div>
      <div
        data-slot="header-status"
        className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground md:justify-end md:text-right"
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
              {refreshing && <LoaderCircle size={16} className="animate-spin motion-reduce:animate-none" aria-hidden="true" />}
              {refreshing ? 'Refreshing…' : 'Refresh'}
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
    },
    {
      label: 'Active Nodes',
      value: summary.nodes.active,
      legend: `${summary.nodes.healthy} healthy · ${summary.nodes.unhealthy} unhealthy · ${summary.nodes.unknown} unknown`,
      href: '/admin/nodes?lifecycle=active',
    },
    {
      label: 'Retired Nodes',
      value: summary.nodes.retired,
      legend: 'Excluded from live health buckets',
      href: '/admin/nodes?lifecycle=retired',
    },
    {
      label: 'Networks',
      value: summary.networks.total,
      legend: `${summary.networks.with_identity_mismatch} with Network Identity Mismatch`,
      href: '/admin/networks',
    },
  ]
  return (
    <nav aria-label="Overview summaries" className="grid grid-cols-2 gap-2 md:grid-cols-4 md:gap-3">
      {cards.map((card) => (
        <Link
          key={card.label}
          data-slot="summary-card"
          to={card.href}
          className={cn(
            'flex min-h-[6.5rem] min-w-0 flex-col justify-between gap-2 rounded-md p-3 text-foreground no-underline transition-all',
            SURFACE_CARD,
            'hover:-translate-y-0.5 hover:shadow-[0_0_20px,0_0_0_1px] hover:shadow-emerald-600/10',
          )}
        >
          <span className="flex items-center justify-between gap-2">
            <span className={EYEBROW}>{card.label}</span>
            <ArrowUpRight size={16} aria-hidden="true" className="shrink-0 text-muted-foreground" />
          </span>
          <strong className="text-2xl font-bold leading-none tracking-tight tabular-nums md:text-3xl">
            {card.value}
          </strong>
          <span className="text-xs leading-snug break-words text-muted-foreground">
            {card.legend}
          </span>
        </Link>
      ))}
    </nav>
  )
}

function AttentionPanel({ query }: { query: OverviewQuery }) {
  const data = query.data
  const { status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const [announcement, setAnnouncement] = useState('')
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [pending, setPending] = useState<string | null>(null)
  const [feedback, setFeedback] = useState<{ tone: 'error' | 'conflict'; message: string } | null>(null)
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
  /** Owner-only per-item acknowledgment (design §15.6, webui.md §15.3). The
   * Server applies only the evidence boundary shown; a changed boundary comes
   * back under skipped and the authoritative queue is refetched. */
  const acknowledge = async (item: AttentionItem) => {
    setPending(item.id)
    setFeedback(null)
    try {
      const result = await acknowledgeAgentAttention(
        item.subject_id,
        [{ kind: item.kind, evidence_key: item.evidence_key }],
        csrfToken,
      )
      if (result.skipped.length > 0) {
        setFeedback({
          tone: 'conflict',
          message: 'This item changed while you were reviewing it. The authoritative queue was refreshed; review the current evidence before acknowledging again.',
        })
      } else {
        setAnnouncement('Attention item acknowledged. The authoritative queue was refreshed.')
      }
      await query.refetch()
    } catch (error) {
      setFeedback({
        tone: 'error',
        message: error instanceof Error ? error.message : 'Unable to acknowledge the item',
      })
    } finally {
      setPending(null)
    }
  }
  // With nothing to act on, the queue collapses to one line; errors, refresh
  // failures and acknowledgment feedback still render below it in full.
  const quiet = Boolean(data && data.attention.length === 0)
  return (
    <article data-slot="overview-panel" className={cn(PANEL, quiet && 'py-3')}>
      <div className={cn(PANEL_HEADING, quiet && 'border-b-0 pb-0')}>
        <div className="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-0.5">
          <h2 className={PANEL_TITLE}>Attention queue</h2>
          {quiet && (
            <p className="text-sm text-muted-foreground">No attention items. Nothing needs an Owner right now.</p>
          )}
        </div>
        {data && <Badge variant="secondary" className="tabular-nums">{data.attention.length} items</Badge>}
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
      {feedback && (
        <Alert variant={feedback.tone === 'error' ? 'destructive' : 'default'} className="mt-3">
          <AlertDescription role="status" className="flex flex-wrap items-center gap-2">
            <StatusBadge status={feedback.tone === 'error' ? 'Error' : 'Conflict'} tone={feedback.tone === 'error' ? 'error' : 'warning'} />
            {feedback.message}
          </AlertDescription>
        </Alert>
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
                pendingId={pending}
                onAcknowledge={acknowledge}
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

function CopyAgentId({ id }: { id: string }) {
  const [status, setStatus] = useState('')
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(id)
      setStatus('Copied')
    } catch {
      setStatus('Copy unavailable; select the full ID above to copy.')
    }
  }
  return <>
    <Button variant="ghost" size="sm" onClick={() => void copy()}>Copy Agent ID</Button>
    <span role="status">{status}</span>
  </>
}

function AcknowledgeButton({ item, pending, onAcknowledge }: { item: AttentionItem; pending: boolean; onAcknowledge: (item: AttentionItem) => void }) {
  return (
    <Button
      variant="outline"
      size="sm"
      className="min-h-11 shrink-0"
      disabled={pending}
      aria-label={`Acknowledge ${item.kind} for this Agent`}
      onClick={() => onAcknowledge(item)}
    >
      {pending ? 'Acknowledging…' : 'Acknowledge'}
    </Button>
  )
}

function AttentionGroup({ group, expanded, pendingId, onAcknowledge, onToggle }: { group: AttentionGroupData; expanded: boolean; pendingId: string | null; onAcknowledge: (item: AttentionItem) => void; onToggle: () => void }) {
  const primary = group.items.reduce((best, item) => {
    const rank = item.severity === 'critical' ? 0 : item.severity === 'warning' ? 1 : 2
    const bestRank = best.severity === 'critical' ? 0 : best.severity === 'warning' ? 1 : 2
    return rank < bestRank ? item : best
  })
  const known = primary.severity === "critical" || primary.severity === "warning"
  const severity = known ? primary.severity : "unknown"
  const route = safeAttentionRoute(group)
  const additional = group.items.filter((item) => item.id !== primary.id)
  const additionalCritical = additional.filter((item) => item.severity === 'critical').length
  return (
    <li data-slot="attention-item" data-severity={severity}
      className={cn('min-w-0 rounded-md border border-border/60 border-l-[3px] bg-background/60 p-3',
        severity === 'critical' && 'border-l-destructive',
        severity === 'warning' && 'border-l-warning',
        severity === 'unknown' && 'border-l-muted-foreground')}>
      <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
        <StatusBadge
          status={severity === 'critical' ? 'Critical' : severity === 'warning' ? 'Warning' : 'Unknown'}
          tone={severity === 'critical' ? 'error' : severity === 'warning' ? 'warning' : 'neutral'}
        />
        <p className="min-w-0 flex-1 basis-48 text-sm font-medium [overflow-wrap:anywhere]">{primary.message}</p>
        {route ? <Link className={TEXT_LINK} to={route}>View {group.subjectKind}<ArrowUpRight size={16} aria-hidden="true" /></Link>
          : <span className="text-xs text-muted-foreground">No detail route available</span>}
        {group.subjectKind === 'agent' && (
          <AcknowledgeButton item={primary} pending={pendingId === primary.id} onAcknowledge={onAcknowledge} />
        )}
      </div>
      <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
        <span title={group.subjectId} className="min-w-0 [overflow-wrap:anywhere]">
          {group.subjectKind} · <span>{group.subjectKind === 'agent' && (!group.label || group.label === group.subjectId)
            ? formatIdentifier(group.subjectId) : group.label}</span>
        </span>
        <span>{attentionObservedText(primary.observed_at)}</span>
        <span className="[overflow-wrap:anywhere]">{primary.kind}</span>
        {group.subjectKind === 'agent' && (
          <details className="min-w-0 max-w-full">
            <summary className="flex min-h-11 min-w-11 cursor-pointer items-center rounded-sm focus-visible:outline-2 focus-visible:outline-ring">Full Agent ID</summary>
            <code className="select-all [overflow-wrap:anywhere]">{group.subjectId}</code>
            <CopyAgentId id={group.subjectId} />
          </details>
        )}
        {additional.length > 0 && (
          <Button variant="link" size="sm" className="max-w-full px-0 whitespace-normal"
            aria-expanded={expanded} aria-controls={`attention-details-${group.key}`} onClick={onToggle}>
            {expanded ? 'Hide additional issues' : `Show ${additional.length} additional issue${additional.length === 1 ? '' : 's'}`}
            {additionalCritical > 0 ? ` · ${additionalCritical} Critical` : ''}
          </Button>
        )}
      </div>
      {!expanded && additional.filter((item) => item.kind === 'agent_spool_overflow').map((item) => (
        <p key={item.id} className="mt-1 text-sm [overflow-wrap:anywhere]">{item.message}</p>
      ))}
      {additional.length > 0 && (
        <ul id={`attention-details-${group.key}`} hidden={!expanded}
          className="mt-2 list-disc space-y-1 pl-4 text-xs [overflow-wrap:anywhere] text-muted-foreground">
          {additional.map((item) => (
            <li key={item.id} className="flex flex-wrap items-center gap-x-2 gap-y-1">
              <span>
                {item.severity === 'critical' ? 'Critical' : item.severity === 'warning' ? 'Warning' : 'Unknown'} ·{' '}
                {item.message} · {item.kind} · {attentionObservedText(item.observed_at)}
              </span>
              {group.subjectKind === 'agent' && (
                <AcknowledgeButton item={item} pending={pendingId === item.id} onAcknowledge={onAcknowledge} />
              )}
            </li>
          ))}
        </ul>
      )}
    </li>
  )
}

type DiagnosticsQuery = ReturnType<typeof useAdminDiagnostics>
type NodesQuery = ReturnType<typeof useAdminNodes>

/** Each Overview preview shows at most this many subjects; the complete
 * inventory, filters and diagnostics stay on the Nodes and Agents pages. */
const PREVIEW_LIMIT = 5
const PREVIEW_LIST = 'mt-1 grid list-none p-0'
const PREVIEW_ITEM = 'grid min-w-0 gap-0.5 border-b border-border/60 py-2 last:border-b-0'
const PREVIEW_FOOTER = 'mt-2 flex flex-wrap items-center justify-between gap-x-3 gap-y-1 border-t pt-2'

function plural(count: number, noun: string): string {
  return count + ' ' + noun + (count === 1 ? '' : 's')
}

function PanelStates({
  query,
  noun,
  loading,
}: {
  query: NodesQuery | DiagnosticsQuery
  noun: string
  loading: string
}) {
  return (
    <>
      {!query.data && query.isPending && (
        <p className={cn(PANEL_STATE, 'mt-3')} role="status">
          <StatusBadge status="Starting" tone="neutral" /> {loading}
        </p>
      )}
      {!query.data && query.isError && (
        <Alert variant="destructive" className="mt-3">
          <AlertDescription className="flex flex-wrap items-center gap-2">
            <StatusBadge status="Error" tone="error" />{' '}
            {query.error instanceof Error ? query.error.message : 'Unable to load ' + noun}
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
            successful {noun.replace(/s$/, '')} values.
            <Button variant="link" size="sm" onClick={() => void query.refetch()}>Try again</Button>
          </AlertDescription>
        </Alert>
      )}
    </>
  )
}

/**
 * Node Health Summary preview (webui.md §8.4.3): the highest-priority Active
 * Nodes, each with its name, Network, independent Health and Freshness, and
 * only the exceptions that need a reason. Per-component diagnostics live on
 * the Nodes page and the Node detail; what the preview leaves out is counted
 * in its footer so a hidden exception is never mistaken for none.
 */
function NodePanel({ nodeQuery }: { nodeQuery: NodesQuery }) {
  const nodes = nodeQuery.data ?? []
  const activeNodes = nodes.filter((node) => node.lifecycle === 'active')
  const prioritizedNodes = prioritizeActiveNodes(activeNodes)
  const visibleNodes = prioritizedNodes.slice(0, PREVIEW_LIMIT)
  const hiddenNodes = prioritizedNodes.slice(PREVIEW_LIMIT)
  const hidden = [
    { count: hiddenNodes.filter((node) => node.health === 'unhealthy').length, noun: 'unhealthy' },
    { count: hiddenNodes.filter((node) => node.health !== 'unhealthy' && node.health !== 'healthy').length, noun: 'unknown health' },
    { count: hiddenNodes.filter((node) => node.freshness === 'stale').length, noun: 'stale' },
    { count: hiddenNodes.filter((node) => node.identity.state === 'mismatched').length, noun: 'identity mismatch' },
    { count: hiddenNodes.filter((node) => node.identity.state !== 'mismatched' && node.identity.state !== 'matched').length, noun: 'identity not reported' },
  ].filter((entry) => entry.count > 0)

  return (
    <article data-slot="overview-panel" className={PANEL}>
      <div className={PANEL_HEADING}>
        <h2 className={PANEL_TITLE}>Node Health Summary</h2>
        {activeNodes.length > 0 && <Badge variant="secondary" className="tabular-nums">{activeNodes.length}</Badge>}
      </div>
      <PanelStates query={nodeQuery} noun="Nodes" loading="Loading the Node Health Summary…" />
      {nodeQuery.data && activeNodes.length === 0 && <Empty description="No Nodes observed yet." />}
      {nodeQuery.data && activeNodes.length > 0 && (
        <ul data-slot="node-preview" aria-label="Highest-priority Active Nodes" className={PREVIEW_LIST}>
          {visibleNodes.map((node) => (
            <NodePreviewItem key={node.node_id} node={node} />
          ))}
        </ul>
      )}
      {nodeQuery.data && activeNodes.length > 0 && (
        <div className={PREVIEW_FOOTER}>
          <span className="text-sm text-muted-foreground" data-slot="preview-count">
            Showing {visibleNodes.length} of {activeNodes.length} Active Nodes
            {hidden.length > 0 && (
              <span className="block text-xs text-foreground" data-slot="preview-hidden">
                Not shown: {hidden.map((entry) => entry.count + ' ' + entry.noun).join(' · ')}
              </span>
            )}
          </span>
          <Link className={TEXT_LINK} to="/admin/nodes">View all Nodes</Link>
        </div>
      )}
    </article>
  )
}

function NodePreviewItem({ node }: { node: AdminNodeListItem }) {
  const exceptional = node.health !== 'healthy'
  const mismatch = node.identity.state === 'mismatched'
  const resync = node.resync_state && node.resync_state !== 'normal' ? node.resync_state : null
  return (
    <li data-slot="node-preview-item" className={PREVIEW_ITEM}>
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-3">
        <Link
          className="inline-flex min-h-11 min-w-0 max-w-full items-center font-medium [overflow-wrap:anywhere] underline-offset-4 hover:underline"
          to={'/admin/nodes/' + encodeURIComponent(node.node_id)}
        >
          {node.display_name ?? formatIdentifier(node.node_id)}
        </Link>
        <span className="flex flex-wrap items-center gap-1.5">
          <span className="sr-only">Health</span>
          <StatusBadge status={node.health} tone={healthTone(node.health)} />
          <span className="sr-only">Freshness</span>
          <StatusBadge status={freshnessLabel(node.freshness)} tone={freshnessTone(node.freshness)} />
        </span>
      </div>
      <p className="min-w-0 text-xs text-muted-foreground [overflow-wrap:anywhere]">
        {node.network_display_name}
        {exceptional && <span className="text-foreground"> · {node.health_reason}</span>}
      </p>
      {(mismatch || resync) && (
        <p className="min-w-0 text-xs [overflow-wrap:anywhere]">
          {mismatch && (
            <span className="font-medium text-destructive">
              Identity mismatched{node.identity.mismatched_fields.length > 0 ? ': ' + node.identity.mismatched_fields.join(', ') : ''}
            </span>
          )}
          {mismatch && resync && ' · '}
          {resync && <span>Resync {resync}</span>}
        </p>
      )}
    </li>
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

/**
 * Agent inventory preview (webui.md §8.4.4): the highest-priority Agents with
 * their name, Server reporting status, receipt time and retained Node count,
 * plus current attention apart from recorded history. Host resources, Spool
 * capacity, clock and report sequence stay on the Agent detail.
 */
function AgentPanel({ query, nodeQuery }: { query: DiagnosticsQuery; nodeQuery: NodesQuery }) {
  const agents = query.data ?? []
  const prioritized = prioritizeAgents(agents)
  const visibleAgents = prioritized.slice(0, PREVIEW_LIMIT)
  const hiddenAgents = prioritized.slice(PREVIEW_LIMIT)
  // Each left-out Agent is counted once, under its most pressing bucket, so
  // the line never reads as more Agents than were left out.
  const buckets = new Map<string, number>()
  for (const agent of hiddenAgents) {
    const evidence = agentEvidence(agent)
    const bucket = evidence.current.length > 0
      ? 'with current attention'
      : agent.liveness === 'offline'
        ? 'offline'
        : agent.liveness !== 'online'
          ? 'unknown reporting'
          : evidence.unknown.length > 0
            ? 'not fully observed'
            : evidence.recorded.length > 0
              ? 'with recorded history'
              : null
    if (bucket) buckets.set(bucket, (buckets.get(bucket) ?? 0) + 1)
  }
  const hidden = [...buckets].map(([noun, count]) => ({ noun, count }))
  const unhealthyByAgent = new Map<string, number>()
  for (const node of nodeQuery.data ?? []) {
    if (node.health === 'unhealthy') unhealthyByAgent.set(node.agent_id, (unhealthyByAgent.get(node.agent_id) ?? 0) + 1)
  }
  return (
    <article data-slot="overview-panel" className={PANEL}>
      <div className={PANEL_HEADING}>
        <h2 className={PANEL_TITLE}>Agent inventory</h2>
        {agents.length > 0 && <Badge variant="secondary" className="tabular-nums">{agents.length}</Badge>}
      </div>
      <PanelStates query={query} noun="Agents" loading="Loading Agent state…" />
      {query.data && agents.length === 0 && <Empty description="No Agents enrolled yet." />}
      {query.data && agents.length > 0 && (
        <ul data-slot="agent-preview" aria-label="Highest-priority Agents" className={PREVIEW_LIST}>
          {visibleAgents.map((agent) => (
            <AgentPreviewItem key={agent.agent_id} agent={agent} unhealthyNodes={nodeQuery.data ? (unhealthyByAgent.get(agent.agent_id) ?? 0) : null} />
          ))}
        </ul>
      )}
      {query.data && agents.length > 0 && (
        <div className={PREVIEW_FOOTER}>
          <span className="text-sm text-muted-foreground" data-slot="preview-count">
            Showing {visibleAgents.length} of {agents.length} Agents
            {hidden.length > 0 && (
              <span className="block text-xs text-foreground" data-slot="preview-hidden">
                Not shown: {hidden.map((entry) => entry.count + ' ' + entry.noun).join(' · ')}
              </span>
            )}
          </span>
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

function AgentPreviewItem({ agent, unhealthyNodes }: { agent: AgentDiagnostic; unhealthyNodes: number | null }) {
  const evidence = agentEvidence(agent)
  const notes = [...evidence.current, ...evidence.unknown]
  const recorded = evidence.recorded
  const shownRecorded = recorded.slice(0, 2)
  const retained = agent.nodes.length
  return (
    <li data-slot="agent-preview-item" className={PREVIEW_ITEM}>
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-3">
        <Link
          className="inline-flex min-h-11 min-w-0 max-w-full items-center font-medium [overflow-wrap:anywhere] underline-offset-4 hover:underline"
          to={'/admin/agents/' + encodeURIComponent(agent.agent_id)}
          title={agent.agent_id}
        >
          {agent.display_name ?? formatIdentifier(agent.agent_id)}
        </Link>
        <span className="flex flex-wrap items-center gap-1.5">
          <span className="sr-only">Reporting</span>
          <StatusBadge status={livenessLabel(agent.liveness)} tone={livenessTone(agent.liveness)} />
        </span>
      </div>
      <p className="min-w-0 text-xs text-muted-foreground">
        {agent.last_received_at ? (
          <>Received <RelativeTime timestamp={agent.last_received_at} /></>
        ) : (
          receiptTimeText(agent.last_received_at)
        )}
        {' · '}
        {plural(retained, 'retained Node')}
        {unhealthyNodes ? <span className="text-destructive"> · {unhealthyNodes} unhealthy</span> : null}
      </p>
      {notes.length > 0 && (
        // The first current or unobserved item, and how many more: the full
        // list is the Attention queue above and the Agent's own page.
        <p className="min-w-0 text-xs [overflow-wrap:anywhere]" data-slot="agent-preview-current">
          <span className={cn(notes[0].tone === 'error' ? 'font-medium text-destructive' : notes[0].tone === 'warning' ? 'text-foreground' : 'text-muted-foreground')}>
            {notes[0].text}
          </span>
          {notes.length > 1 && <span className="text-muted-foreground"> · +{notes.length - 1} more</span>}
        </p>
      )}
      {recorded.length > 0 && (
        <p className="min-w-0 text-xs text-muted-foreground [overflow-wrap:anywhere]" data-slot="agent-preview-recorded">
          <span className="font-medium">Recorded history:</span>{' '}
          {shownRecorded.map((item, index) => (
            <span key={item.key} className={cn(item.tone === 'error' && 'font-medium text-destructive')}>
              {index > 0 && ' · '}
              {item.short ?? item.text}
            </span>
          ))}
          {recorded.length > shownRecorded.length && ' · +' + (recorded.length - shownRecorded.length) + ' more on the Agent'}
        </p>
      )}
    </li>
  )
}
