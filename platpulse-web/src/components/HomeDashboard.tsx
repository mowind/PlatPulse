import { useEffect, useMemo, useRef, useState } from 'react'
import { useNodeRegionHeights } from './useNodeRegionHeights'
import { Link, useSearchParams } from 'react-router'
import type { PublicConsensusInsight, PublicNetwork, PublicNode } from '../api/generated'
import { realtimeStreamLabel } from './RealtimeNotice'
import { peerInsightCollectionStatus, peerInsightFreshnessStatus, peerInsightValueStatus } from './PeerInsight'
import { NodeHealthMarker, formatRelativeTime, formatUtcDateTime } from './StatusBadge'
import GeoMapBoundary from './GeoMapBoundary'
import GeoWorldMap from './GeoWorldMap'
import { geoMapStatus, homeGeoOverview } from '../homeGeo'
import { formatNodeDataBytes } from '../formatBytes'
import { formatDuration } from '../formatDuration'
import { nodeDataProgress } from '../nodeData'
import { MetricRow } from './MetricRow'
import type { ProgressStatus } from './ui/progress-thin'
import { CardX } from './ui/card-x'
import { Alert, AlertDescription } from './ui/alert'
import { Empty } from './ui/empty'
import { Input, Select } from './ui/input'
import { Tabs, TabsList, TabsTrigger } from './ui/tabs'
import { Server, HeartPulse, TriangleAlert, Network, ChevronUp, ChevronDown, Info } from 'lucide-react'
import { SURFACE_TOOLBAR } from '../lib/surface'
import { cn } from '../lib/utils'
import {
  healthCategory,
  healthTone,
  homeNetworkScope,
  homeNodeLabel,
  readHomeFilters,
  selectHomeRecords,
  writeHomeFilters,
  type HomeFilterRejection,
  type HomeFilters,
  type HomeHealthFilter,
  type HomeSort,
  type HomeValidatorFilter,
  type HomeView,
} from '../homeFilters'
import { LinkedValidatorSection, validatorDataStatus, type ValidatorDataStatus } from './LinkedValidator'
import { ValidatorTotalCard } from './ValidatorTotals'
import { ValidatorActivityBadge } from './ValidatorActivityBadge'
import { SURFACE_CARD_INTERACTIVE } from '../lib/surface'
import { SummaryMetricCard } from './SummaryMetricCard'
import { OverviewBand } from './OverviewBand'
import { Button } from './ui/button'
import { Dialog, DialogTrigger, DialogContent, DialogTitle, DialogDescription } from './ui/dialog'

type HomeDashboardProps = {
  networks: PublicNetwork[]
  realtimeStatus: 'connecting' | 'connected' | 'disconnected'
  online: boolean
  resetting: boolean
  error: string | null
  hasLastGood?: boolean
  loading: boolean
}

type NodeRecord = { network: PublicNetwork; node: PublicNode }

const sortOptions: Array<{ value: HomeSort; label: string }> = [
  { value: 'health', label: 'Health' },
  { value: 'name', label: 'Name' },
  { value: 'head', label: 'Current Head' },
  { value: 'peers', label: 'Peers' },
  { value: 'process_cpu', label: 'Process CPU' },
  { value: 'process_memory', label: 'Process memory' },
]

/** Emerald Home: six equal overview cards beside a proportional Peer map.
 * Node cards keep their independent, content-width-driven column rules. */
export default function HomeDashboard({
  networks,
  realtimeStatus,
  online,
  error,
  hasLastGood = true,
  loading,
}: HomeDashboardProps) {
  const [search, setSearch] = useSearchParams()
  // An ordinary Home URL is the whole filter state (design §9, #222): the
  // Network selection, search, and sorting survive a refresh or a direct link,
  // and a value this deployment cannot honour falls back visibly instead of
  // being obeyed or silently dropped.
  const { filters, rejected } = useMemo(() => readHomeFilters(search), [search])
  const records = useMemo<NodeRecord[]>(
    () => networks.flatMap((network) => network.nodes.map((node) => ({ network, node }))),
    [networks],
  )
  const hasProjection = !loading && (error === null || hasLastGood)
  // Only the projection can say whether a Network selection still exists, so
  // the selection stays provisional until a projection is available.
  const networkKeys = useMemo(() => networks.map((network) => network.networkKey), [networks])
  const scope = homeNetworkScope(filters.network, hasProjection ? networkKeys : null)
  // Two scopes, read once: the Network selection covers the overview band and
  // the map, while search, health, and Validator status narrow the list only.
  const { scoped, matching } = useMemo(
    () => selectHomeRecords(records, { ...filters, network: scope.network }),
    [filters, records, scope.network],
  )
  const rejectedFilters = scope.rejected ? [scope.rejected, ...rejected] : rejected
  // The search box owns its own text while the reader types; the address bar
  // owns it again as soon as it says something the box did not write. A
  // keystroke commits the URL at once, but that commit is painted a frame
  // later, so a value read straight back from the address bar would erase every
  // character typed in the meantime; the draft keeps fast typing whole while
  // the URL ends up with the same text.
  const [queryDraft, setQueryDraft] = useState<string | null>(null)
  const queryText = queryDraft ?? filters.query
  // The address bar owns the box again the moment the reader arrives at a URL
  // the box did not type. Back and Forward are the browser's own history, so
  // they are taken over directly, which is what makes Back work even when the
  // keystroke that wrote the current URL is still in flight; a keystroke this
  // box wrote never raises that event, so fast typing stays whole. An in-app
  // link that drops the query, or a direct load, is covered by the empty query
  // and by mounting with the URL already read.
  useEffect(() => {
    const takeOver = () => setQueryDraft(null)
    window.addEventListener('popstate', takeOver)
    return () => window.removeEventListener('popstate', takeOver)
  }, [])
  useEffect(() => {
    if (filters.query === '') setQueryDraft(null)
  }, [filters.query])
  // One reader action can reach this toolbar twice: a Network pill reports its
  // value on focus and again on press, and asking the address bar for the same
  // URL twice would leave a Back step that appears to do nothing. The note also
  // has to be dropped as soon as the address bar lands anywhere, or a value
  // pasted after going Back would be mistaken for a write still in flight.
  const issuedSearch = useRef<string | null>(null)
  const renderedSearch = search.toString()
  useEffect(() => {
    issuedSearch.current = null
  }, [renderedSearch])

  /** A discrete choice is a step the reader can undo; typing in the search box
   *  replaces the current entry so history does not grow per keystroke. */
  const updateFilters = (patch: Partial<HomeFilters>, replace = false) => {
    const next = writeHomeFilters({ ...filters, network: scope.network, ...patch }, search)
    const nextSearch = next.toString()
    if (nextSearch === renderedSearch || nextSearch === issuedSearch.current) return
    issuedSearch.current = nextSearch
    setSearch(next, { replace })
  }

  const scopeName = scope.network === 'all'
    ? null
    : networks.find((network) => network.networkKey === scope.network)?.displayName ?? scope.network
  // The map receives a projection, never raw Network input; the same overview
  // also covers the selected Network, and only that selection changes it.
  const geoOverview = useMemo(
    () => homeGeoOverview(networks, scope.network),
    [networks, scope.network],
  )
  const geoStatus = geoMapStatus(geoOverview, { loading, hasProjection })
  // The measured region minima belong to the card grid: leave the measurement
  // off in the list view, and let the changed flag remeasure the grid when the
  // reader switches back, because that grid element is mounted again.
  const nodeGridRef = useNodeRegionHeights(matching, hasProjection && filters.view === 'card')
  const scopedNetworks = scope.network === 'all' ? networks : networks.filter(network => network.networkKey === scope.network)
  const healthyCount = hasProjection ? scoped.filter(({ node }) => healthCategory(node.health) === 'healthy').length : null
  const streamLabel = realtimeStreamLabel(realtimeStatus)
  return (
    <section aria-label="Home">
      <div className="px-4 pt-4" data-realtime-status={realtimeStatus}>
        {error || streamLabel || !online ? (
          <>
          {error && (
            <Alert variant="destructive" className="border-none bg-red-400/10 rounded-md">
              <AlertDescription role="alert">{error}</AlertDescription>
            </Alert>
          )}
          {streamLabel && (
            <p className="mt-2 flex items-center gap-2 text-xs text-muted-foreground" role="status" aria-live="polite">
              <span className="inline-block size-1.5 rounded-full bg-emerald-600 animate-pulse" aria-hidden="true" />
              {streamLabel}
            </p>
          )}
          {!online && (
            <p className="mt-2 flex items-center gap-2 text-xs text-muted-foreground" role="status" aria-live="polite">
              <span className="inline-block size-1.5 rounded-full bg-yellow-600 animate-pulse" aria-hidden="true" />
              You are offline
            </p>
          )}
          </>
        ) : null}
      </div>
      {loading && (
        <p role="status" className="px-4 pt-4 text-sm text-muted-foreground">
          Starting Home…
        </p>
      )}

      {/* A value this deployment cannot honour is reported, not obeyed: the
          fallback is what makes an ordinary Home link safe to refresh. */}
      {rejectedFilters.length > 0 && (
        <div className="px-4 pt-4">
          <p
            data-slot="home-filter-notice"
            role="status"
            className="rounded-md border-none bg-amber-400/10 px-4 py-3 text-xs [overflow-wrap:anywhere] text-amber-600 dark:text-amber-400"
          >
            {rejectedFilterNotice(rejectedFilters)}
          </p>
        </div>
      )}

      {/* Home's overview band. The map track is deliberately wider than the
            statistics track (5:6), matching the Emerald reference where the map
            is the larger half. When the six tiles are shorter than the map band
            they sit on its floor, so the gap down to the Network group is the
            same 16px rhythm that separates that group from the Node cards.
            Node Detail renders this same band, so the two pages cannot drift. */}
      <OverviewBand
        className="p-4"
        dataSlot="home-overview"
        mapSlot="home-map"
        metricsLabel="Home summary"
        mapFirst
        metrics={<>
          <SummaryCard label="Active Nodes" value={hasProjection ? scoped.length : null} tone="green" icon="server" />
          <SummaryCard label="Healthy Nodes" value={healthyCount} tone="green" icon="heart" />
          <ValidatorTotalCard networks={scopedNetworks} metric="blocks" availability={loading ? 'loading' : hasProjection ? 'ready' : 'unavailable'} />
          <SummaryCard label="Attention" value={healthyCount === null ? null : scoped.length - healthyCount}
            tone={healthyCount !== null && scoped.length === healthyCount ? 'green' : 'red'} icon="alert" />
          <SummaryCard label="Networks" value={hasProjection ? scopedNetworks.length : null} tone="green" icon="network" />
          <ValidatorTotalCard networks={scopedNetworks} metric="rewards" availability={loading ? 'loading' : hasProjection ? 'ready' : 'unavailable'} />
        </>}
        map={<GeoMapBoundary>
          <GeoWorldMap overview={geoOverview} status={geoStatus} />
        </GeoMapBoundary>}
      />

      <div className="relative p-4 pt-0 md:static">
        <div className="flex flex-col gap-2" aria-label="Node filters and sorting">
          <div className="flex flex-wrap items-start gap-2 md:flex-nowrap md:items-center">
          <div className="w-full overflow-x-auto rounded-sm py-1.5 -my-1.5 md:relative md:z-10 md:w-auto">
            <Tabs
              value={scope.network}
              onValueChange={(value) => updateFilters({ network: value })}
              className="w-full flex-col gap-4"
            >
              <TabsList className={cn('compact-tabs min-h-0 group-data-[orientation=horizontal]/tabs:h-8 h-8 w-max rounded-md md:bg-background', SURFACE_TOOLBAR)} aria-label="Network filter">
                <TabsTrigger value="all" className="min-h-0 h-6.5 shrink-0 flex-none rounded-sm border-none text-xs shadow-none data-[state=active]:text-emerald-600 dark:data-[state=active]:text-emerald-600">
                  All Networks
                </TabsTrigger>
                {networks.map((network) => (
                  <TabsTrigger
                    key={network.networkKey}
                    value={network.networkKey}
                    className="min-h-0 h-6.5 shrink-0 flex-none rounded-sm border-none text-xs shadow-none data-[state=active]:text-emerald-600 dark:data-[state=active]:text-emerald-600"
                  >
                    {network.displayName}
                  </TabsTrigger>
                ))}
              </TabsList>
            </Tabs>
          </div>
          <label className="ml-auto flex items-center gap-2 text-xs text-muted-foreground md:relative md:z-10 md:shrink-0 md:rounded-md md:bg-background md:pl-2">
            Sort
            <Select
              aria-label="Sort"
              className={cn('compact-select h-8 w-auto rounded-md border-x-0 border-y-[6px] border-transparent bg-clip-padding -my-1.5 shadow-none md:bg-background md:text-foreground dark:md:bg-background', SURFACE_TOOLBAR)}
              value={filters.sort}
              onChange={(event) => updateFilters({ sort: event.target.value as HomeSort })}
            >
              {sortOptions.map((option) => (
                <option key={option.value} value={option.value}>{option.label}</option>
              ))}
            </Select>
          </label>
          {/* The card/list choice is part of the same ordinary Home URL contract
              as the filters and the sort (design §9). It sits after the Network
              pills on purpose: the pill row stays the first tab stop on the
              surface. On a phone the row wraps, so the pills keep the whole
              width and this control drops to the next line without leaving the
              reading order. */}
          <Tabs
            value={filters.view}
            onValueChange={(value) => updateFilters({ view: value as HomeView })}
            className="flex-none md:relative md:z-10"
          >
            <TabsList className={cn('flex-none rounded-md md:bg-background', SURFACE_TOOLBAR)} aria-label="View">
              <TabsTrigger value="card" className="flex-none text-xs">Cards</TabsTrigger>
              <TabsTrigger value="list" className="flex-none text-xs">List</TabsTrigger>
            </TabsList>
          </Tabs>
          </div>

          {/* Search, health, and Validator status narrow the list below. None of
              them changes the overview band or the map, which follow the Network
              selection alone (design §9). Every value searched here is public:
              the Node display name, the Node ID, and the Network display name. */}
          <div className="flex flex-wrap items-end gap-x-3 gap-y-2">
            <label className="flex min-w-0 grow flex-col gap-1 text-xs font-medium tracking-wider text-muted-foreground sm:grow-0">
              Search
              <Input
                type="search"
                aria-label="Search Active Nodes"
                className="w-full min-w-48 sm:w-64"
                placeholder="Name, Node ID, or Network"
                value={queryText}
                onChange={(event) => {
                  setQueryDraft(event.target.value)
                  updateFilters({ query: event.target.value }, true)
                }}
                onBlur={() => setQueryDraft(null)}
              />
            </label>
            <label className="flex flex-col gap-1 text-xs font-medium tracking-wider text-muted-foreground">
              Health
              <Select
                aria-label="Health filter"
                className="w-auto min-w-32"
                value={filters.health}
                onChange={(event) => updateFilters({ health: event.target.value as HomeHealthFilter })}
              >
                <option value="all">All health</option>
                <option value="healthy">Healthy</option>
                <option value="unhealthy">Unhealthy</option>
                <option value="unknown">Unknown</option>
              </Select>
            </label>
            <label className="flex flex-col gap-1 text-xs font-medium tracking-wider text-muted-foreground">
              Validator status
              <Select
                aria-label="Validator status filter"
                className="w-auto min-w-36"
                value={filters.validator}
                onChange={(event) => updateFilters({ validator: event.target.value as HomeValidatorFilter })}
              >
                <option value="all">All Validator status</option>
                <option value="validator">Validator</option>
                <option value="not_validator">Not a Validator</option>
                <option value="unknown">Unknown</option>
              </Select>
            </label>
          </div>
        </div>

        <div className="mt-4">
          {/* 300px is Emerald's own auto-fill minimum track (HomeView.vue's
              minmax(300px, 1fr)), so the Home grid keeps the reference's card
              size. The card's own two-column switch is an independent, measured
              22.5rem (see the node-card rules in emerald.css): a 303px
              auto-filled card is narrower than it, so each chain count and each
              ordinary Validator parameter takes one full-width line (Txs/Peers
              and the two cumulative Validator cells stay paired), while a wider
              card uses its width. */}
          {loading || (error && !hasLastGood) ? null : (
            <>
              {/* The list is one scope inside the Network scope: it says how many
                  Active Nodes match, against how many the selection holds, so the
                  overview counters and the cards can never look contradictory. */}
              <p
                data-slot="home-result-count"
                role="status"
                aria-live="polite"
                className="mb-3 text-xs text-muted-foreground"
              >
                Showing <span className="tabular-nums">{matching.length.toLocaleString()}</span> of{' '}
                <span className="tabular-nums">{scoped.length.toLocaleString()}</span> Active Nodes
                {scopeName ? <> in <span className="[overflow-wrap:anywhere]">{scopeName}</span></> : null}. Search,
                health, and Validator status narrow this list only; the Home summary and the Peer map cover the whole
                Network selection.
              </p>
              {scoped.length === 0 ? (
                <Empty description="No Active Nodes in this view.">
                  <span className="text-xs">Retired Nodes are not listed on Home.</span>
                </Empty>
              ) : matching.length === 0 ? (
                <Empty description="No Active Nodes match these filters.">
                  <span className="text-xs">
                    Clear the search or widen the filters to list all{' '}
                    <span className="tabular-nums">{scoped.length.toLocaleString()}</span> Active Nodes in this
                    Network selection.
                  </span>
                </Empty>
              ) : filters.view === 'list' ? (
                <HomeNodeList records={matching} />
              ) : (
                <div
                  className="grid auto-rows-fr grid-cols-1 gap-3 sm:grid-cols-[repeat(auto-fill,minmax(300px,1fr))]"
                  ref={nodeGridRef}
                  data-slot="node-grid"
                  aria-label="Active Nodes"
                >
                  {matching.map(({ network, node }) => (
                    <div data-slot="node-card-frame" className="min-w-0" key={node.nodeId}>
                      <HomeNodeCard network={network} node={node} />
                    </div>
                  ))}
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </section>
  )
}

const SUMMARY_ICONS = {
  server: Server,
  heart: HeartPulse,
  alert: TriangleAlert,
  network: Network,
} as const

function SummaryCard({ label, value, tone, icon }: {
  label: string; value: number | null; tone: 'green' | 'red'; icon: keyof typeof SUMMARY_ICONS
}) {
  return <SummaryMetricCard label={label} value={value === null ? 'Unknown' : value.toLocaleString()}
    tone={value !== null && value > 0 ? tone : 'green'} icon={SUMMARY_ICONS[icon]} />
}

/**
 * Compact Home card, styled as Emerald's NodeCard: a stretched semantic title
 * link to Node Detail, an emerald status dot with a ping ring, a two-column
 * resource grid with thin progress bars, and a label-over-value consensus
 * grid without per-row leaders. Copy, disclosure and identity controls sit
 * outside the anchor so they never trigger navigation. Healthy Nodes carry no
 * routine prose. Exceptional health keeps a short diagnostic line (issue #97);
 * resync progress is a separate, lightweight status area.
 */
/** The compact list view (stories 72, 73, 79): one row per matching Node with
 * the seven public columns this ticket names, inside one local horizontal
 * scroller whose Node name and health word stay pinned (emerald.css owns those
 * two columns and their offsets). Every value comes from the same public field
 * and the same formatter the card uses, so a metric the Server never attested
 * still reads Unknown here and a real zero still reads 0. QC, Locked and
 * Committed stay card-only columns, exactly as the design says. */
function HomeNodeList({ records }: { records: readonly NodeRecord[] }) {
  return (
    <div data-slot="node-list-scroll" className="overflow-x-auto rounded-md bg-background">
      <table data-slot="node-list" aria-label="Active Nodes" className="w-full text-sm">
        <thead>
          <tr>
            <th scope="col" data-column="name" className="text-left">Node</th>
            <th scope="col" data-column="network" className="text-left">Network</th>
            <th scope="col" data-column="health" className="text-left">Health</th>
            <th scope="col" data-column="head" className="text-right">Current Head</th>
            <th scope="col" data-column="peers" className="text-right">Peers</th>
            <th scope="col" data-column="process-cpu" className="text-right">Process CPU</th>
            <th scope="col" data-column="process-memory" className="text-right">Process memory</th>
          </tr>
        </thead>
        <tbody>
          {records.map(({ network, node }) => (
            <tr key={node.nodeId} data-slot="node-list-row">
              <td data-column="name">
                <Link
                  to={'/nodes/' + node.nodeId}
                  aria-label={homeNodeLabel(node)}
                  title={homeNodeLabel(node)}
                  className="inline-flex min-h-11 min-w-11 max-w-full items-center rounded-sm focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                >
                  <span className="truncate">{homeNodeLabel(node)}</span>
                </Link>
              </td>
              <td data-column="network" className="text-muted-foreground">
                <span className="block truncate" title={network.displayName}>{network.displayName}</span>
              </td>
              <td data-column="health">
                <span className="inline-flex items-center gap-1.5 whitespace-nowrap">
                  <NodeHealthMarker health={node.health} />
                  {healthLabel(node.health)}
                </span>
              </td>
              <td data-column="head" className="text-right tabular-nums">{formatNumber(node.currentHead)}</td>
              <td data-column="peers" className="text-right tabular-nums">
                {formatPeerCount(node)}
                {peerRetentionCue(node) && (
                  <small data-slot="node-list-peers-cue" className="block text-[11px] text-muted-foreground">
                    {peerRetentionCue(node)}
                  </small>
                )}
              </td>
              <td data-column="process-cpu" className="text-right tabular-nums">{formatPercent(node.processCpuPercent)}</td>
              <td data-column="process-memory" className="text-right tabular-nums">{formatPercent(node.processMemoryPercent)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function HomeNodeCard({ network, node }: NodeRecord) {
  const tone = healthTone(node.health)
  const diagnostic = exceptionalDiagnostic(node)
  // Every displayed chain value is computed once: the grid borrows the same
  // strings to decide whether a compact two-column cell can hold them.
  const consensusStatus = consensusValueStatus(node.consensus)
  const business = {
    head: formatNumber(node.currentHead),
    qc: formatConsensusBlock(node.consensus?.highestQcBlock, consensusStatus),
    locked: formatConsensusBlock(node.consensus?.highestLockBlock, consensusStatus),
    committed: formatConsensusBlock(node.consensus?.highestCommitBlock, consensusStatus),
    txs: formatNumber(node.latestBlockTransactionCount),
    peers: formatPeerCount(node),
  }
  const businessWide = Object.values(business).some((value) => value.length > 12)
  const resyncing = (node.resyncState ?? '').toLowerCase() === 'resyncing'
  // One compact, abnormal-only Provider data cue for the identity row. A
  // routine Current value stays silent; the region below owns the authoritative
  // no-live-Validator empty state.
  const dataStatus = node.validator ? validatorDataStatus(node.validator) : null
  return (
    <article
      data-slot="node-card"
      data-tone={tone}
      className={cn(
        // Borderless by design (the surface and its hover ring carry the
        // card): Tailwind's preflight leaves border-style: solid behind, so
        // this has to be stated explicitly.
        'group/node-card relative h-full w-full rounded-md border-none transition-all duration-200',
        SURFACE_CARD_INTERACTIVE,
        'hover:z-1 hover:-translate-y-0.5 hover:shadow-[0_0_20px,0_0_0_1px] hover:shadow-emerald-600/10',
        tone === 'bad' && 'shadow-[0_0_0_1px] shadow-red-600/20',
        tone === 'warn' && 'shadow-[0_0_0_1px] shadow-amber-500/20',
      )}
    >
      <CardX bordered={false} className="bg-transparent" contentClassName="gap-2.5" headerClassName="!block !p-0" header={<div data-node-region="identity"><div data-node-region-content className="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-x-2 gap-y-1.5 px-4 pt-3 pb-2.5">
          <div className="flex min-w-0 items-center gap-2">
            <NodeHealthMarker health={node.health} />
            {/* The 44px target is kept on the anchor; the negative block margin
                keeps it from inflating the identity row, so the name row and
                the Network · Uptime row stay content-driven and can sit 6px
                apart. */}
            <h2 className="min-w-0 text-base font-semibold"><Link to={`/nodes/${node.nodeId}`} aria-label={homeNodeLabel(node)} title={homeNodeLabel(node)} className="flex -my-2.5 min-h-11 min-w-0 items-center after:absolute after:inset-0 after:rounded-md focus-visible:outline-none focus-visible:after:ring-[3px] focus-visible:after:ring-ring/50"><span className="truncate">{homeNodeLabel(node)}</span></Link></h2>
          </div>
          <ValidatorActivityBadge validator={node.validator} identityReason={node.validatorIdentityReason} />
          <div data-slot="node-identity-meta" className="col-span-2 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
            <p className="min-w-0 flex-1"><span className="[overflow-wrap:anywhere]">{network.displayName}</span> · <span className="whitespace-nowrap">Uptime {formatDuration(node.processUptimeMs)}</span></p>
            {/* One unified top status position: an abnormal Node-health summary,
                a live resync percentage and an abnormal Provider data state sit
                here as short chips. The normal Data: Current line is gone; the
                full reason for each stays in the explanation and Node detail. */}
            <span data-slot="node-status-slot" className="ml-auto flex min-w-0 flex-wrap items-center justify-end gap-x-1.5 gap-y-1">
              {resyncing && <ResyncCue node={node} />}
              {dataStatus && <ValidatorDataCue status={dataStatus} />}
            {/* State audit of the reference capture's conspicuous frame: the control is
                transparent by default (transparent background, transparent 1px border, no
                outline or shadow), paints the ghost accent on hover, and paints the shared
                3px focus-visible ring while keyboard-focused. The open dialog also gives the
                trigger aria-expanded's accent background. The captured frame is that
                focus-visible ring — required keyboard feedback, so it is kept; the icon
                matches the copy/Details controls at 14px. */}
            <Dialog><DialogTrigger asChild><Button variant="ghost" size="icon" className="relative z-10 -my-3.5 size-11 shrink-0" aria-label="Node identity details"><Info className="size-3.5" /></Button></DialogTrigger>
              <DialogContent className="max-h-[85dvh] overflow-y-auto rounded-md shadow-sm"><DialogTitle className="pr-10 [overflow-wrap:anywhere]">{homeNodeLabel(node)}</DialogTitle><DialogDescription className="[overflow-wrap:anywhere]">Network: {network.displayName} · Uptime {formatDuration(node.processUptimeMs)}. Node role describes the Node’s consensus membership, not its linked Validator’s current staking validity or the freshness of Provider data.</DialogDescription>
                <p className="text-sm text-muted-foreground">Active Nodes are in the latest Agent Inventory, not necessarily online. Healthy reflects successful, fresh RPC, sync and consensus observations. Process errors, a stopped or Unknown process state, or Network Identity Mismatch prevent Healthy; disabled process monitoring does not. Healthy does not mean synchronization is complete; Resyncing is shown independently.</p>
                <p className="text-sm text-muted-foreground">Home Attention counts Active Nodes that are not Healthy, including Unknown. Overview counts and the Peer map follow the selected Network; the search, health, and Validator status filters narrow only the Node list.</p>
                {diagnostic && <p className="text-sm text-muted-foreground">Current health diagnostic: {diagnostic.text}</p>}
                {dataStatus && <p className="text-sm text-muted-foreground">Validator data: {dataStatus.label}. {dataStatus.description}</p>}
              </DialogContent>
            </Dialog>
            </span>
          </div>
          {/* Abnormal reasons remain readable; healthy cards create no empty row. */}
          {diagnostic && <p
            data-slot="node-diagnostic"
            data-tone={diagnostic?.tone}
            className={cn(
              'col-span-2 m-0 text-[11px] [overflow-wrap:anywhere]',
              diagnostic?.tone === 'destructive' ? 'text-destructive' : 'text-amber-500 dark:text-amber-400',
            )}
          >
            {diagnostic.text}
          </p>}
        </div></div>}>
        <div data-node-region="resource"><div data-node-region-content><ResourceRow node={node} /></div></div>
        <div data-node-region="chain"><div data-node-region-content>
        <div data-slot="node-business-metrics" data-wide={businessWide || undefined} className="border-t border-border pt-3">
          <MetricRow layout="compact" label="Head" value={business.head} />
          <ConsensusRow status={consensusStatus} values={business} />
          <div data-slot="node-counts" data-wide={business.txs.length > 12 || business.peers.length > 12 || undefined}>
            <MetricRow layout="compact" label="Txs" value={business.txs} />
            <MetricRow layout="compact" label="Peers" value={business.peers} />
          </div>
          {formatPeerObservation(node) && (
            <small data-slot="metric-row-detail" className="col-span-2 text-[11px] text-muted-foreground">
              {formatPeerObservation(node)}
            </small>
          )}
        </div>
        </div></div>
        <div data-node-region="validator"><div data-node-region-content><LinkedValidatorSection node={node} /></div></div>
        </CardX>
    </article>
  )
}

/** Resync targets the retained Historical High-Water Mark, not Network Head.
 * Keep it independent of health, consensus membership and Validator identity.
 * Only the live percentage stays on the identity row; the retained target, the
 * last progress and its full time remain reachable from the same focusable
 * control, so nothing the removed three-line block carried is deleted. */
function ResyncCue({ node }: { node: PublicNode }) {
  const [, refreshAge] = useState(0)
  useEffect(() => {
    // Display age only: Server-owned health, freshness and resync state stay untouched.
    const timer = window.setInterval(() => refreshAge(tick => tick + 1), 15_000)
    return () => window.clearInterval(timer)
  }, [])
  const current = validHeight(node.currentHead)
  const target = validHeight(node.historicalHighWatermark)
  const percent = current !== null && target !== null && target > 0
    ? (current / target * 100).toFixed(2) + '%'
    : 'Unknown'
  const timestamp = node.resyncLastProgressAt
  const date = validProgressDate(timestamp)
  return (
    <Dialog>
      <DialogTrigger asChild>
        {/* Keep the 44px touch target without reserving its full height in the
            compact metadata line, matching the neighbouring information entry. */}
        <button type="button" data-slot="node-resync-cue"
          className="relative z-10 -my-3.5 inline-flex min-h-11 items-center rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label={`Resync progress: ${percent} toward the Historical High-Water Mark`}>
          <span className="inline-flex items-center gap-1 rounded border border-amber-500/40 px-1.5 py-0.5 text-[11px] font-medium leading-4 text-amber-500 dark:text-amber-400">
            <span>Resyncing</span><span className="tabular-nums">{percent}</span>
          </span>
        </button>
      </DialogTrigger>
      <DialogContent className="max-h-[85dvh] overflow-y-auto rounded-md shadow-sm">
        <DialogTitle>Resync progress</DialogTitle>
        <DialogDescription>The last recorded advance toward the Historical High-Water Mark, not the latest report time. The target is the Node’s retained historical height, not the Network Head.</DialogDescription>
        <p className="m-0 text-sm tabular-nums">{formatNumber(current)} / {formatNumber(target)}</p>
        {date ? <p className="m-0 text-sm">Last progress <time dateTime={date.toISOString()}>{formatRelativeTime(date)}</time> · {formatUtcDateTime(date)}</p> : <p className="m-0 text-sm">Last progress: Unknown</p>}
        {date && timestamp && <p className="m-0 min-w-0 overflow-x-auto whitespace-nowrap font-mono text-xs text-muted-foreground" tabIndex={0} aria-label="Full source timestamp with UTC offset">{timestamp}</p>}
      </DialogContent>
    </Dialog>
  )
}

/** The abnormal Provider data state, kept separate from the online/health
 *  marker and from the freshness of the Node’s own observations. */
function ValidatorDataCue({ status }: { status: ValidatorDataStatus }) {
  return <span data-slot="validator-data-cue" data-tone={status.tone} role="status" title={status.description}
    aria-label={`Validator data: ${status.label}. ${status.description}`}
    className={cn('inline-flex min-w-0 items-center gap-1 rounded border px-1.5 py-0.5 text-[11px] leading-4',
      status.tone === 'destructive'
        ? 'border-destructive/40 text-destructive'
        : 'border-amber-500/40 text-amber-600 dark:text-amber-400')}>
    <span className={cn('inline-block size-1.5 shrink-0 rounded-full', status.tone === 'destructive' ? 'bg-destructive' : 'bg-amber-500')} aria-hidden="true" />
    <span className="whitespace-nowrap">Data: {status.label}</span>
  </span>
}

/** Require a zoned RFC3339 value; Date alone normalizes impossible dates and
 * interprets zone-less strings in the viewer's timezone. Neither is evidence. */
function validProgressDate(value: string | null | undefined): Date | null {
  if (typeof value !== 'string' || !/^\d{4}-(?:0[1-9]|1[0-2])-(?:0[1-9]|[12]\d|3[01])T(?:[01]\d|2[0-3]):[0-5]\d:[0-5]\d(?:\.\d+)?(?:Z|[+-](?:[01]\d|2[0-3]):[0-5]\d)$/i.test(value)) return null
  const date = new Date(value)
  const calendarDate = value.slice(0, 10)
  const calendar = new Date(calendarDate + 'T00:00:00Z')
  if (!Number.isFinite(date.getTime()) || calendar.toISOString().slice(0, 10) !== calendarDate) return null
  return date
}

function validHeight(value: number | null | undefined): number | null {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : null
}

/** PlatPulse owns no occupancy threshold rule for process CPU/Memory or the
 *  Node data directory, so none is invented here: the ordinary fill uses
 *  Emerald's normal-state success token, the same green family as the online
 *  dot and the theme accent, instead of the informational blue or the default
 *  primary (white in the dark theme). A Server-owned warning/error rule would
 *  replace this, and it would still have to map onto the existing tokens. */
const RESOURCE_BAR_STATUS: ProgressStatus = 'success'

function ResourceRow({ node }: { node: PublicNode }) {
  const nodeDataProgressValue = nodeDataProgress(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)
  const nodeDataBytes = formatNodeDataBytes(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)
  // Wide cards fold the used / total caption into the value line; a narrow
  // card keeps it as its own caption under the track. The bytes and the
  // percentage are both always rendered, only regrouped.
  const nodeDataValue = formatPercent(nodeDataProgressValue)
  return (
    <div
      data-slot="node-resource-region"
      className="grid grid-cols-2 gap-x-3 gap-y-1"
      aria-label="Node process and host network resources"
    >
      <MetricRow layout="compact" label="CPU" value={formatPercent(node.processCpuPercent)} progress={node.processCpuPercent ?? null} progressStatus={RESOURCE_BAR_STATUS} />
      <MetricRow layout="compact" label="Memory" value={formatPercent(node.processMemoryPercent)} progress={node.processMemoryPercent ?? null} progressStatus={RESOURCE_BAR_STATUS} />
      <div className="col-span-2" data-slot="node-data-resource">
        <MetricRow
          layout="compact"
          label="Node data"
          value={nodeDataValue}
          wideValue={nodeDataBytes && nodeDataProgressValue != null ? nodeDataBytes + ' · ' + nodeDataValue : undefined}
          detail={nodeDataBytes}
          progress={nodeDataProgressValue}
          progressStatus={RESOURCE_BAR_STATUS}
        />
      </div>
      <div className="col-span-2" data-slot="host-network-speed" role="group" aria-label="Host network speed">
        <MetricRow layout="compact" label="Speed" value={<span className="flex items-center gap-2 whitespace-nowrap">
          <span aria-label={`Upload ${formatRate(node.hostNetworkTxBytesPerSec)}`} className="inline-flex items-center gap-0.5 text-green-600"><ChevronUp className="size-3 shrink-0" aria-hidden="true" />{formatRate(node.hostNetworkTxBytesPerSec)}</span>
          <span aria-label={`Download ${formatRate(node.hostNetworkRxBytesPerSec)}`} className="inline-flex items-center gap-0.5 text-blue-600"><ChevronDown className="size-3 shrink-0" aria-hidden="true" />{formatRate(node.hostNetworkRxBytesPerSec)}</span>
        </span>} />
      </div>
    </div>
  )
}

function formatPercent(value: number | null | undefined) {
  return value == null ? 'Unknown' : `${value.toFixed(1)}%`
}

function formatRate(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value) || value < 0) return 'Unknown'
  const units = ['bps', 'Kbps', 'Mbps', 'Gbps', 'Tbps']
  let scaled = value * 8
  let unit = 0
  while (scaled >= 1000 && unit < units.length - 1) {
    scaled /= 1000
    unit += 1
  }
  let amount = Number(scaled.toPrecision(3))
  // Rounding at a unit boundary should show 1Mbps, not 1000Kbps.
  if (amount >= 1000 && unit < units.length - 1) {
    amount /= 1000
    unit += 1
  }
  return `${amount}${units[unit]}`
}

/**
 * QC, Locked, Committed and the header role badge come from
 * the Node-scoped last-good consensus observation (issue #99). Missing,
 * unsupported, disabled, and never-observed values render Unknown, never zero
 * or No; failed or stale collections keep the last-good values and visibly mark
 * them Stale.
 */
function ConsensusRow({ status, values }: {
  status: 'current' | 'stale' | 'unknown'
  values: { qc: string; locked: string; committed: string }
}) {
  const detail = status === 'stale' ? 'Stale' : undefined
  return (
    <div className="contents" role="group" aria-label="Consensus values">
      <MetricRow layout="compact" label="QC" value={values.qc} detail={detail} />
      <MetricRow layout="compact" label="Locked" value={values.locked} detail={detail} />
      <MetricRow layout="compact" label="Committed" value={values.committed} detail={detail} />
    </div>
  )
}

function consensusValueStatus(insight: PublicConsensusInsight | undefined): 'current' | 'stale' | 'unknown' {
  if (!insight || insight.validator == null) return 'unknown'
  // Starting/Disabled/Unsupported never carry a usable consensus value;
  // only an accepted successful observation provides membership truth.
  if (['starting', 'disabled', 'unsupported'].includes(insight.state)) return 'unknown'
  // Unknown freshness means the Server cannot certify currency; never
  // present a retained value as current (issue #99).
  if (insight.freshness === 'unknown') return 'unknown'
  // A failed collection or an aged value is Stale while the last-good value
  // remains visible; Server state and freshness remain separate dimensions.
  if (insight.state === 'error' || insight.freshness === 'stale') return 'stale'
  return 'current'
}

function formatConsensusBlock(value: number | null | undefined, status: 'current' | 'stale' | 'unknown'): string {
  if (value == null || status === 'unknown') return 'Unknown'
  return value.toLocaleString()
}

type NodeDiagnostic = { text: string; tone: 'destructive' | 'warning' }

/** One sanitized health diagnostic line for exceptional Nodes (issue #97).
 * Resync progress is independent and never hidden by an unhealthy diagnostic. */
function exceptionalDiagnostic(node: PublicNode): NodeDiagnostic | null {
  if (node.health !== 'healthy') {
    const reason = node.healthReason?.trim()
    return {
      text: reason || `Health ${healthLabel(node.health).toLowerCase()}`,
      tone: 'destructive',
    }
  }
  return null
}

function formatPeerCount(node: PublicNode) {
  return formatNumber(node.peers?.peerCount)
}
function formatPeerObservation(node: PublicNode): string | undefined {
  const peer = node.peers
  if (!peer) return 'Unknown observation'

  const collection = peerInsightCollectionStatus(peer)
  const freshness = peerInsightFreshnessStatus(peer)
  const value = peerInsightValueStatus(peer)
  const hasValue = value !== 'Unknown'
  const qualifiers: string[] = []

  if (collection === 'Error') qualifiers.push('Collection failed')
  else if (collection !== 'Current') qualifiers.push('Collection ' + collection.toLowerCase())
  if (freshness === 'Stale') qualifiers.push('Stale')
  else if (freshness !== 'Current') qualifiers.push('Freshness unknown')

  if (!hasValue) {
    if (qualifiers.length === 0) qualifiers.push('Value unknown')
    qualifiers.push('No successful Peer snapshot is available')
    return qualifiers.join('; ')
  }

  if (qualifiers.length > 0) {
    qualifiers.push('Showing last successful snapshot')
    if (value === 'Empty') qualifiers.push('authoritative zero')
    return qualifiers.join('; ')
  }

  if (value === 'Empty') return 'Empty; authoritative zero'
  // A fresh peer count is self-explanatory; exceptional observation details
  // remain visible below the value.
  return undefined
}
/**
 * The list's half of `formatPeerObservation`. A retained Peer count keeps its
 * value, so the cell has to say whose snapshot that value is: the card spells the
 * case out in a sentence, and a table cell carries the two words that matter plus
 * the dimensions that are not current. A count the Server never attested stays
 * Unknown with no cue, and a fully current observation carries no cue either
 * (#223, story 73; design §11.1).
 */
function peerRetentionCue(node: PublicNode): string | undefined {
  const peer = node.peers
  if (!peer || peerInsightValueStatus(peer) === 'Unknown') return undefined
  const reasons: string[] = []
  const collection = peerInsightCollectionStatus(peer)
  const freshness = peerInsightFreshnessStatus(peer)
  if (collection !== 'Current') {
    reasons.push(collection === 'Unknown' ? 'collection unknown' : `collection ${collection.toLowerCase()}`)
  }
  if (freshness !== 'Current') {
    reasons.push(freshness === 'Unknown' ? 'freshness unknown' : `freshness ${freshness.toLowerCase()}`)
  }
  if (reasons.length === 0) return undefined
  return `last good (${reasons.join(', ')})`
}

function healthLabel(value: string): string {
  if (value === 'healthy') return 'Healthy'
  if (value === 'unhealthy') return 'Unhealthy'
  return 'Unknown'
}

function formatNumber(value: number | null | undefined) { return value == null ? 'Unknown' : value.toLocaleString() }

/** How Home names a URL value it refused. It stays a plain, visible sentence:
 *  the reader can see which filter changed and why (design §9, #222). */
const REJECTED_FILTER_LABELS: Record<HomeFilterRejection['parameter'], string> = {
  network: 'Network',
  health: 'health',
  validator: 'Validator status',
  sort: 'sort',
  view: 'view',
}

function rejectedFilterNotice(rejections: HomeFilterRejection[]): string {
  const items = rejections.map(({ parameter, value }) => `${REJECTED_FILTER_LABELS[parameter]} "${value}"`)
  const list = items.length === 1
    ? items[0]
    : `${items.slice(0, -1).join(', ')} and ${items[items.length - 1]}`
  const outcome = rejections.length === 1 ? 'it fell back to its default' : 'they fell back to their defaults'
  return `This Home link asked for ${list}, which this deployment does not offer, so ${outcome}. The rest of the link was applied unchanged.`
}
