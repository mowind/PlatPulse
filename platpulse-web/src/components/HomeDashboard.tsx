import { useCallback, useEffect, useMemo, useRef, useState, type ChangeEvent, type KeyboardEvent } from 'react'
import { Link, useLocation, useSearchParams } from 'react-router'
import type { PublicNetwork, PublicNode } from '../api/generated'
import { realtimeStreamLabel } from './RealtimeNotice'
import { peerInsightCollectionStatus, peerInsightFreshnessStatus, peerInsightValueStatus } from './PeerInsight'
import { NodeHealthMarker, formatRelativeTime, formatUtcDateTime } from './StatusBadge'
import {
  HEAD_COMPARISON_TONE_CLASS,
  OBSERVED_NETWORK_HEAD,
  networkHeadComparison,
  networkHeadOffset,
  nodeHeadUnconfirmed,
  validHeight,
  type HeadComparisonTone,
} from './NodeDetailObservations'
import GeoMapBoundary from './GeoMapBoundary'
import GeoWorldMap from './GeoWorldMap'
import { geoMapStatus, homeGeoOverview, homeNodeSelectionGeoOverview } from '../homeGeo'
import { formatNodeDataBytes } from '../formatBytes'
import { formatDuration } from '../formatDuration'
import { nodeDataProgress } from '../nodeData'
import { MetricRow } from './MetricRow'
import { ProgressThin, type ProgressStatus } from './ui/progress-thin'
import { CardX } from './ui/card-x'
import { Alert, AlertDescription } from './ui/alert'
import { Empty } from './ui/empty'
import { Input, Select } from './ui/input'
import { Tabs, TabsList, TabsTrigger } from './ui/tabs'
import { Server, HeartPulse, TriangleAlert, Network, ChevronUp, ChevronDown, Info, Search, X } from 'lucide-react'
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
import { homeReturnState, homeReturnTab, landHomeReturn, readHomeReturn, type HomeReturn, type HomeReturnState } from '../homeReturn'
import { LinkedValidatorSection, validatorDataStatus, type ValidatorDataStatus } from './LinkedValidator'
import { ValidatorTotalCard } from './ValidatorTotals'
import { ValidatorActivityBadge } from './ValidatorActivityBadge'
import { SURFACE_CARD_INTERACTIVE } from '../lib/surface'
import { SummaryMetricCard } from './SummaryMetricCard'
import { OverviewBand } from './OverviewBand'
import { Button } from './ui/button'
import { Dialog, DialogTrigger, DialogContent, DialogTitle, DialogDescription } from './ui/dialog'
import { ToolbarPopover } from './ui/toolbar-popover'

type HomeDashboardProps = {
  networks: PublicNetwork[]
  realtimeStatus: 'connecting' | 'connected' | 'disconnected'
  online: boolean
  resetting: boolean
  error: string | null
  hasLastGood?: boolean
  loading: boolean
  /** True while the projection on screen is being refetched: the reading the
   *  arrival places the reader in is then still provisional (design §15.25). */
  refreshing?: boolean
}

type NodeRecord = { network: PublicNetwork; node: PublicNode }

/** Reveals the row a return landed on: the row this render registered for that
 *  Node, brought to where it now is, with the keyboard reader continuing from
 *  the returned Node rather than from the top of a list they never read. */
function revealReturnRow(
  matching: readonly NodeRecord[],
  nodeRows: ReadonlyMap<string, HTMLElement>,
  index: number | null,
): void {
  if (index === null) return
  const nodeId = matching[index]?.node.nodeId
  const row = nodeId === undefined ? undefined : nodeRows.get(nodeId)
  row?.scrollIntoView?.({ block: 'center' })
  row?.querySelector<HTMLElement>('[data-slot="node-link"]')?.focus({ preventScroll: true })
}

/** Every Node link on Home carries the departure the return journey rebuilds
 *  from: the Node, the search it was read in, and the Node's place in the list
 *  on screen. Rows are found again through refs keyed by Node ID. */
type HomeReturnLinkState = (nodeId: string, index: number, count: number) => HomeReturnState
type NodeRowRefs = (nodeId: string, element: HTMLElement | null) => void

/** The one accessible name Home's search control answers to, in both shapes
 *  #241 gives it: the row's own field, and the entry that opens that field. */
const SEARCH_LABEL = 'Search Active Nodes'

/**
 * The room the toolbar's own search shape takes, in CSS pixels (#241): the
 * field's readable 13rem plus its 44px Clear search action and the 4px between
 * them. emerald.css states it once as the 16rem its width query asks for, and
 * the one measurement below reads the same number, so the query and the focus
 * handoff disagree about the reader's toolbar only if this pair drifts.
 */
const SEARCH_FIELD_TRACK = 16 * 16

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
  refreshing = false,
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
  // Two scopes, read once (#233): the Network selection is the whole reading
  // while nothing is narrowed, and once search, health, or Validator status
  // narrows the list, the overview band and the map read the Nodes it holds.
  const { scoped, matching } = useMemo(
    () => selectHomeRecords(records, { ...filters, network: scope.network }),
    [filters, records, scope.network],
  )
  const rejectedFilters = scope.rejected ? [scope.rejected, ...rejected] : rejected
  // The same-tab return (#224): the departure described by every Node link
  // travels in this document's own history entry and never in a URL, and only
  // this document reads it back, so a copied link, a second tab, a restored
  // session, and a reload all inherit no reading position.
  const location = useLocation()
  const [departure, setDeparture] = useState(() => readHomeReturn(location.state, homeReturnTab()))
  const [returnNotice, setReturnNotice] = useState<string | null>(null)
  // The departure this arrival still owes the reader a landing for, and the
  // reading it arrived in. Home holds the projection it fetched before the
  // reader opened the Node, so the authoritative refetch can reorder or remove
  // that Node a moment after the first paint: while the departure is pending,
  // the Node is landed on again — with its notice when it left the reading, and
  // at its new place when the settled order moved it. The reading is remembered
  // with it so that nothing is owed once the reader has asked for a different
  // one by any other route.
  const pendingReturn = useRef<{ departure: HomeReturn; search: string } | null>(null)
  // The landing this arrival has already been given: the row it placed and the
  // notice it showed, with whether the projection it was read from was still
  // being refetched. That reading is provisional, so the Node decides again
  // when the authoritative one arrives — it may have moved the reader's Node
  // without changing what the notice says. Once it has settled, the arrival is
  // done: a live reading the reader is watching never moves them around.
  const landedReturn = useRef<{ index: number | null; notice: string | null; provisional: boolean } | null>(
    null,
  )
  // Rows are located through React's own refs, keyed by Node ID: the returned
  // row is the one this render put on screen, never a private DOM lookup.
  const nodeRows = useRef(new Map<string, HTMLElement>())
  const registerNodeRow = (nodeId: string, element: HTMLElement | null) => {
    if (element === null) nodeRows.current.delete(nodeId)
    else nodeRows.current.set(nodeId, element)
  }
  const nodeLinkState: HomeReturnLinkState = (nodeId, index, count) =>
    homeReturnState(nodeId, search, index, count, homeReturnTab())
  // One arrival, and every landing it is still owed. The Node ID decides where
  // the reader lands, even when the refreshed order has moved it; the departure
  // is then consumed with the history state it travelled in, so this entry can
  // never be inherited again. No offset is ever written down: the browser owns
  // Back and Forward, and the row is revealed where it now is.
  useEffect(() => {
    const reading = search.toString()
    const pending = departure === null ? pendingReturn.current : { departure, search: reading }
    if (pending === null || pending.search !== reading || !hasProjection) return
    const landing = landHomeReturn(scoped, matching, pending.departure)
    if (departure !== null) {
      pendingReturn.current = pending
      setDeparture(null)
      setSearch(search, { replace: true, state: null })
    }
    const owed = landedReturn.current
    const moved = owed !== null && owed.provisional && owed.index !== landing.index
    if (owed !== null && owed.notice === landing.notice && !moved) {
      // The authoritative reading arrived without moving the Node: the landing
      // stands, and this arrival is no longer provisional.
      if (owed.provisional && !refreshing) landedReturn.current = { ...owed, provisional: false }
      return
    }
    landedReturn.current = { index: landing.index, notice: landing.notice, provisional: refreshing }
    setReturnNotice(landing.notice)
    revealReturnRow(matching, nodeRows.current, landing.index)
  }, [departure, hasProjection, matching, refreshing, scoped, search, setSearch])
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
  /** The arrival's notice has been answered and the departure behind it is no
   *  longer owed a landing: the reader has arrived at a reading of their own. */
  const finishReturn = useCallback(() => {
    pendingReturn.current = null
    landedReturn.current = null
    setReturnNotice(null)
  }, [])
  useEffect(() => {
    const takeOver = () => {
      setQueryDraft(null)
      // Another entry's reading, and the reader's own: nothing is owed here.
      finishReturn()
    }
    window.addEventListener('popstate', takeOver)
    return () => window.removeEventListener('popstate', takeOver)
  }, [finishReturn])
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
    // A reader action is a new reading: the arrival's notice has been answered,
    // and the departure behind it is not owed a landing any more.
    finishReturn()
    setSearch(next, { replace })
  }

  // The two toolbar surfaces (#232): the sort order with the two status
  // filters, and the search field. Only one is open at a time, and neither is
  // part of the URL: a copied link never carries an open panel, and Back never
  // closes one.
  const [panel, setPanel] = useState<'none' | 'filters' | 'search'>('none')
  const filtersOpen = panel === 'filters'
  const searchOpen = panel === 'search'
  const closePanel = useCallback(() => setPanel('none'), [])
  const togglePanel = (which: 'filters' | 'search') =>
    setPanel((current) => (current === which ? 'none' : which))
  const filtersTriggerRef = useRef<HTMLButtonElement | null>(null)
  const searchTriggerRef = useRef<HTMLButtonElement | null>(null)
  const searchInputRef = useRef<HTMLInputElement | null>(null)
  useEffect(() => {
    // Typing is the reason the field is there, so it takes focus as it appears.
    if (searchOpen) searchInputRef.current?.focus()
  }, [searchOpen])
  const searchSlotRef = useRef<HTMLDivElement | null>(null)
  const searchFieldRef = useRef<HTMLInputElement | null>(null)
  /** The shape the last measurement found, and whether the reader was inside the
   *  search control when it changed under them. */
  const searchShapeRef = useRef<boolean | null>(null)
  const searchFocusWasInside = useRef(false)
  /**
   * Which of the row's two search shapes the toolbar has room for is a CSS
   * decision on the slot's own width (#241, emerald.css), and this effect adds
   * exactly one thing to it: the reader does not lose the keyboard when the
   * shape they are in is replaced by the other one. It measures the same room
   * against the same 16rem, and only when a resize crosses that line while the
   * reader is inside the search control does it hand the focus on — to the field
   * when the field becomes the row's own control, closing the entry's surface as
   * it goes, so no control stays behind a shape that is no longer displayed, and
   * to the entry when the field goes out of sight. It never decides which shape
   * is displayed: that stays one rule in one place, so the two can never
   * disagree about the reader's toolbar.
   */
  useEffect(() => {
    const slot = searchSlotRef.current
    if (!slot || typeof ResizeObserver === 'undefined') return
    const rememberPlace = (event: FocusEvent) => {
      const target = event.target
      if (!(target instanceof Node)) return
      // A shape going out of sight takes the keyboard from the reader for a
      // moment and hands it to the document itself; that is not the reader
      // moving on, so it does not count as their place changing.
      if (target === document.body || target === document.documentElement) return
      searchFocusWasInside.current = slot.contains(target)
    }
    const reconcile = () => {
      const fits = slot.clientWidth >= SEARCH_FIELD_TRACK
      const previous = searchShapeRef.current
      searchShapeRef.current = fits
      if (previous === null || previous === fits || !searchFocusWasInside.current) return
      if (fits) {
        setPanel((current) => (current === 'search' ? 'none' : current))
        searchFieldRef.current?.focus()
      } else {
        searchTriggerRef.current?.focus()
      }
    }
    document.addEventListener('focusin', rememberPlace)
    const observer = new ResizeObserver(reconcile)
    observer.observe(slot)
    reconcile()
    return () => {
      observer.disconnect()
      document.removeEventListener('focusin', rememberPlace)
    }
  }, [])
  // Only Health and Validator status are counted: they are the two choices that
  // live behind the toolbar entry and would otherwise be invisible, while the
  // Network selection, the sort order, the view, and the search text each show
  // their own state on the surface. The count is stated at zero as well — "no
  // status filter is on" is a fact about the reading, not an absence — and it is
  // decorative: the entry's own name stays "Sort & filter", and a reader who
  // cannot see the count is told it through the entry's description.
  const statusFilterCount = (filters.health === 'all' ? 0 : 1) + (filters.validator === 'all' ? 0 : 1)
  const statusFilterCountId = 'home-status-filter-count'
  const statusFilterSummary = `${statusFilterCount} of 2 status filters on`
  // The search entry states its own condition twice over: a marker drawn inside
  // the control, so an active search is never read through colour alone, and a
  // description naming the text in force, so a reader who sees neither still
  // knows why the list is narrowed while the field itself is out of sight.
  const searchActive = queryText.trim() !== ''
  const searchStateId = 'home-search-state'
  // Only Health and Validator status reset: the sort order, the search text, and
  // the Network selection are the reader's other choices and stay as they are.
  const clearStatusFilters = () => updateFilters({ health: 'all', validator: 'all' })
  /** Clear search drops the query and nothing else (#232): the Network
   *  selection, the two status filters, the sort order and the view are the
   *  reader's other choices and stay exactly where they were. */
  const clearSearch = () => {
    setQueryDraft(null)
    updateFilters({ query: '' }, true)
  }
  /** That one action, in either shape (#241): the two shapes are the same
   *  control, so they carry the same Clear search with the same name, the same
   *  disabled state and the same effect. */
  const clearSearchProps = {
    variant: 'ghost',
    size: 'icon-sm',
    'aria-label': 'Clear search',
    disabled: queryText === '',
    onClick: clearSearch,
  } as const
  /** The one search field, in either of the row's two shapes (#241). Both shapes
   *  are the same labelled control over the same draft, so the row's own field
   *  and the field the entry opens are one query rather than two that could
   *  disagree. Escape is answered here instead of being left to the browser: its
   *  own action on a search field may revert the text, and the reader's filter is
   *  not something a retreat key gets to clear. */
  const searchFieldProps = {
    type: 'search',
    'aria-label': SEARCH_LABEL,
    placeholder: 'Name, Node ID, or Network',
    value: queryText,
    onChange: (event: ChangeEvent<HTMLInputElement>) => {
      setQueryDraft(event.target.value)
      updateFilters({ query: event.target.value }, true)
    },
    onBlur: () => setQueryDraft(null),
    onKeyDown: (event: KeyboardEvent<HTMLInputElement>) => {
      if (event.key === 'Escape') event.preventDefault()
    },
  } as const

  const scopeName = scope.network === 'all'
    ? null
    : networks.find((network) => network.networkKey === scope.network)?.displayName ?? scope.network
  // The Network selection's own Networks: the scope of the two cumulative
  // Validator cells, which never follow a narrowed list (#233).
  const scopedNetworks = useMemo(
    () => (scope.network === 'all'
      ? networks
      : networks.filter((network) => network.networkKey === scope.network)),
    [networks, scope.network],
  )
  // The list selection is what a filter narrows. While it holds every Active
  // Node of the Network selection nothing is narrowed, and every reading stays
  // the one Home has always shown.
  const selectionNarrowed = matching.length !== scoped.length
  const matchedNodes = useMemo(() => matching.map(({ node }) => node), [matching])
  // A narrowed list is counted by the Networks that still hold a matched Node;
  // unfiltered, the counter keeps reading the Network selection itself, so a
  // Network with no Active Node is not silently dropped from it.
  const matchedNetworkCount = useMemo(() => {
    if (!selectionNarrowed) return scopedNetworks.length
    const keys = new Set(matching.map(({ network }) => network.networkKey))
    return scopedNetworks.filter((network) => keys.has(network.networkKey)).length
  }, [matching, scopedNetworks, selectionNarrowed])
  const healthyCount = hasProjection ? matching.filter(({ node }) => healthCategory(node.health) === 'healthy').length : null
  // A narrowed selection that matches no Node leaves the whole band in an
  // explicit empty state (#233, decision 8): the four counters that follow the
  // list read no figure at all — never a zero, and never a failure tone — while
  // the list keeps its "Showing 0 of N" line and the map states the reason in
  // words. Without a projection the counters keep reading 'Unknown', which is a
  // different statement from "no Node matches". The two cumulative cells are
  // not part of the empty state: they keep their Network-level totals.
  const emptySelection = hasProjection && selectionNarrowed && matching.length === 0
  const emptyLabel = emptySelection ? 'No match' : undefined
  const activeNodeCount = emptySelection ? null : hasProjection ? matching.length : null
  const selectionHealthyCount = emptySelection ? null : healthyCount
  const attentionCount = activeNodeCount === null || selectionHealthyCount === null
    ? null
    : activeNodeCount - selectionHealthyCount
  const selectionNetworkCount = emptySelection ? null : hasProjection ? matchedNetworkCount : null
  // The map receives a projection, never raw Network input. While nothing is
  // narrowed that projection is the Network-level Geo Insight, field for field;
  // a narrowed list reads the Node Selection Geo Aggregate over exactly the
  // Nodes the search, health, and Validator status filters left (#233), which
  // the Server ships as compact per-Node buckets in the same response.
  const selectionLabel = scopeName
    ? 'the current list selection in ' + scopeName
    : 'the current list selection'
  const geoOverview = useMemo(
    () => (selectionNarrowed
      ? homeNodeSelectionGeoOverview(matchedNodes, scopedNetworks, selectionLabel, scope.network)
      : homeGeoOverview(networks, scope.network)),
    [matchedNodes, networks, scopedNetworks, scope.network, selectionLabel, selectionNarrowed],
  )
  const geoSelectionBasis = selectionNarrowed ? 'node-selection' : 'network'
  const geoStatus = geoMapStatus(geoOverview, { loading, hasProjection })
  // The two cumulative Validator cells keep the Network selection, so they say
  // which selection they cover as soon as a filter makes that visible.
  const cumulativeScopeNote = selectionNarrowed
    ? 'Whole Network selection, never the filtered list'
    : undefined
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

      {/* A return whose Node the reader can no longer see says which Node is
          gone and what Home showed instead, rather than leaving them nowhere. */}
      {returnNotice && (
        <div className="px-4 pt-4">
          <p
            data-slot="home-return-notice"
            role="status"
            className="rounded-md border-none bg-amber-400/10 px-4 py-3 text-xs [overflow-wrap:anywhere] text-amber-600 dark:text-amber-400"
          >
            {returnNotice}
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
          {/* Four counters read the current list selection (#233); the two
              cumulative Validator cells keep the Network selection and say so
              on their face, because the Server publishes those totals per
              Network and a Node filter must never re-sum them. A selection that
              matches no Node puts these four in their own explicit empty state
              instead of a zero, while the cumulative cells keep counting the
              Network selection. */}
          <SummaryCard label="Active Nodes" value={activeNodeCount} tone="green" icon="server" emptyLabel={emptyLabel} />
          <SummaryCard label="Healthy Nodes" value={selectionHealthyCount} tone="green" icon="heart" emptyLabel={emptyLabel} />
          <ValidatorTotalCard networks={scopedNetworks} metric="blocks" scopeNote={cumulativeScopeNote} availability={loading ? 'loading' : hasProjection ? 'ready' : 'unavailable'} />
          <SummaryCard label="Attention" value={attentionCount}
            tone={attentionCount === null || attentionCount === 0 ? 'green' : 'red'} icon="alert" emptyLabel={emptyLabel} />
          <SummaryCard label="Networks" value={selectionNetworkCount} tone="green" icon="network" emptyLabel={emptyLabel} />
          <ValidatorTotalCard networks={scopedNetworks} metric="rewards" scopeNote={cumulativeScopeNote} availability={loading ? 'loading' : hasProjection ? 'ready' : 'unavailable'} />
        </>}
        map={<GeoMapBoundary>
          <GeoWorldMap overview={geoOverview} status={geoStatus} selectionBasis={geoSelectionBasis} />
        </GeoMapBoundary>}
      />

      <div className="relative p-4 pt-0 md:static">
        <div className="flex flex-col gap-2" aria-label="Node filters and sorting">
          <div
            data-slot="home-toolbar"
            className="flex flex-wrap items-start gap-2 md:flex-nowrap md:items-center"
          >
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
          {/* The sort order and the two status filters share one toolbar
              control (issue #232): the row keeps the Network pills, this entry,
              the card/list choice, and the search control, and every control
              this entry hides leaves the tab walk with it. The badge counts the
              status filters, the only choices this entry represents and the only
              ones the row could not otherwise show; the Network selection, the
              sort order, the view, and the search text each say their own state. */}
          <div className={cn('ml-auto md:relative md:shrink-0', filtersOpen ? 'md:z-30' : 'md:z-10')}>
            <ToolbarPopover
              open={filtersOpen}
              onClose={closePanel}
              returnFocusRef={filtersTriggerRef}
              label="Sort and filters"
              className="flex w-72 flex-col gap-3"
              trigger={
                <Button
                  ref={filtersTriggerRef}
                  variant="outline"
                  aria-expanded={filtersOpen}
                  aria-haspopup="dialog"
                  aria-describedby={statusFilterCountId}
                  data-slot="home-filter-trigger"
                  className={cn('gap-1.5 rounded-md px-2.5 text-xs shadow-none md:bg-background', SURFACE_TOOLBAR)}
                  onClick={() => togglePanel('filters')}
                >
                  Sort &amp; filter
                  {/* The count is always drawn, zero included: it is the one piece
                      of toolbar state this entry represents and the row could not
                      otherwise show. A live count keeps the accent chip; zero is a
                      muted chip, so "nothing is filtering" never reads as an active
                      filter. */}
                  <span
                    data-slot="home-filter-count"
                    aria-hidden="true"
                    className={cn(
                      'rounded-full border px-1.5 text-[0.625rem] font-semibold leading-4',
                      statusFilterCount > 0
                        ? 'border-emerald-600 bg-emerald-600 text-white dark:border-emerald-500 dark:bg-emerald-500 dark:text-emerald-950'
                        : 'border-border text-muted-foreground',
                    )}
                  >
                    {statusFilterCount}
                  </span>
                </Button>
              }
            >
              <label className="flex flex-col gap-1 text-xs font-medium tracking-wider text-muted-foreground">
                Sort
                <Select
                  aria-label="Sort"
                  className="w-full"
                  value={filters.sort}
                  onChange={(event) => updateFilters({ sort: event.target.value as HomeSort })}
                >
                  {sortOptions.map((option) => (
                    <option key={option.value} value={option.value}>{option.label}</option>
                  ))}
                </Select>
              </label>
              <label className="flex flex-col gap-1 text-xs font-medium tracking-wider text-muted-foreground">
                Health
                <Select
                  aria-label="Health filter"
                  className="w-full"
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
                  className="w-full"
                  value={filters.validator}
                  onChange={(event) => updateFilters({ validator: event.target.value as HomeValidatorFilter })}
                >
                  <option value="all">All Validator status</option>
                  <option value="validator">Validator</option>
                  <option value="not_validator">Not a Validator</option>
                  <option value="unknown">Unknown</option>
                </Select>
              </label>
              <div className="flex items-center justify-between gap-2 border-t pt-3">
                <p className="text-xs text-muted-foreground">
                  {statusFilterCount === 0 ? 'No status filter is on' : statusFilterSummary}
                </p>
                <Button
                  variant="ghost"
                  size="sm"
                  className="px-2 text-xs"
                  disabled={statusFilterCount === 0}
                  onClick={clearStatusFilters}
                >
                  Clear filters
                </Button>
              </div>
            </ToolbarPopover>
            <span id={statusFilterCountId} className="sr-only">{statusFilterSummary}</span>
          </div>
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

          {/* The search slot is the room the row has left for the search control
              (#241): one flex item that takes whatever the Network pills, the
              Sort & filter entry and the card/list choice leave over at their
              own widths, so the room is the row's real room rather than a column
              count of the viewport. Its own width never depends on what it
              holds, which is what lets the width query in emerald.css ask the
              row's one question — is there room for a compact field beside the
              controls that are already there? — and get the same answer
              whichever shape is displayed. */}
          <div
            ref={searchSlotRef}
            data-slot="home-search-slot"
            className="flex min-w-11 flex-1 items-center"
          >
            {/* One shape: that compact field, with the Clear search action that
                belongs to it. It is one fixed 16rem — the field's own readable
                13rem plus that action — so the field a reader is given is never
                a squeezed version of this one, and the shape is only displayed
                where the slot holds all of it. */}
            <div
              data-slot="home-search-inline"
              className="w-[16rem] shrink-0 items-center gap-1"
            >
              <Input
                ref={searchFieldRef}
                data-slot="home-search-field"
                data-search-state={searchActive ? 'on' : 'off'}
                aria-describedby={searchActive ? searchStateId : undefined}
                className={cn('w-full', searchActive && 'border-emerald-600 dark:border-emerald-500')}
                {...searchFieldProps}
              />
              <Button {...clearSearchProps}>
                <X aria-hidden="true" />
              </Button>
            </div>
            {/* The other shape: the icon-only entry (#232) that opens that same
                field. It is drawn by the same line-icon set as the card/list
                choice beside it, and its surface is the one place this row's
                search is ever behind a control. */}
            <div
              data-slot="home-search-collapsed"
              className={cn('md:relative md:shrink-0', searchOpen ? 'md:z-30' : 'md:z-10')}
            >
              <ToolbarPopover
                open={searchOpen}
                onClose={closePanel}
                returnFocusRef={searchTriggerRef}
                label={SEARCH_LABEL}
                className="w-72 sm:w-80"
                trigger={
                  <Button
                    ref={searchTriggerRef}
                    variant="outline"
                    size="icon"
                    aria-label={SEARCH_LABEL}
                    aria-expanded={searchOpen}
                    aria-haspopup="dialog"
                    aria-describedby={searchActive ? searchStateId : undefined}
                    data-slot="home-search-trigger"
                    data-search-state={searchActive ? 'on' : 'off'}
                    className={cn(
                      'relative rounded-md shadow-none md:bg-background',
                      searchActive && 'border-emerald-600 text-emerald-700 dark:border-emerald-500 dark:text-emerald-400',
                    )}
                    onClick={() => togglePanel('search')}
                  >
                    <Search aria-hidden="true" />
                    {searchActive && (
                      <span
                        data-slot="home-search-active"
                        aria-hidden="true"
                        className="absolute top-1 right-1 size-1.5 rounded-full bg-emerald-600 dark:bg-emerald-400"
                      />
                    )}
                  </Button>
                }
              >
                <div className="flex items-center gap-2">
                  <Input ref={searchInputRef} className="w-full" {...searchFieldProps} />
                  <Button {...clearSearchProps}>
                    <X aria-hidden="true" />
                  </Button>
                </div>
              </ToolbarPopover>
            </div>
          </div>
          {/* The described-by sentence lives outside both shapes: each shape
              describes itself with it, so whichever one is displayed carries the
              flag in words and neither hides the other's description. */}
          {searchActive && (
            <span id={searchStateId} className="sr-only">{`Search is on: ${queryText}`}</span>
          )}
          </div>
        </div>

        <div className="mt-4">
          {/* The grid's minimum track is the card's own 22.5rem content switch
              (see the node-card rules in emerald.css), so auto-fill resolves on
              the measured container instead of a fixed column count: one column
              on a phone, two from the tablet widths, three once the 1280px shell
              holds three of those tracks, and four only if a container ever
              genuinely holds four. No row is equalized with another (#238). */}
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
                {scopeName ? <> in <span className="[overflow-wrap:anywhere]">{scopeName}</span></> : null}.
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
                <HomeNodeList records={matching} linkState={nodeLinkState} registerRow={registerNodeRow} />
              ) : (
                <div
                  className="grid grid-cols-1 gap-3 sm:grid-cols-[repeat(auto-fill,minmax(min(100%,22.5rem),1fr))]"
                  data-slot="node-grid"
                  aria-label="Active Nodes"
                >
                  {matching.map(({ network, node }, index) => (
                    <div
                      data-slot="node-card-frame"
                      className="min-w-0"
                      key={node.nodeId}
                      ref={(element) => registerNodeRow(node.nodeId, element)}
                    >
                      <HomeNodeCard
                        network={network}
                        node={node}
                        linkState={nodeLinkState(node.nodeId, index, matching.length)}
                      />
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

function SummaryCard({ label, value, tone, icon, emptyLabel }: {
  label: string; value: number | null; tone: 'green' | 'red'; icon: keyof typeof SUMMARY_ICONS
  /** Copy for an explicit empty state; without it a null value reads 'Unknown'. */
  emptyLabel?: string
}) {
  return <SummaryMetricCard label={label} value={value === null ? emptyLabel ?? 'Unknown' : value.toLocaleString()}
    tone={value !== null && value > 0 ? tone : 'green'} icon={SUMMARY_ICONS[icon]} />
}

/** The compact list view (stories 72, 73, 79): one row per matching Node with
 * the seven public columns this ticket names, inside one local horizontal
 * scroller whose Node name and health word stay pinned (emerald.css owns those
 * two columns and their offsets). Every value comes from the same public field
 * and the same formatter the card uses, so a metric the Server never attested
 * still reads Unknown here and a real zero still reads 0. QC, Locked and
 * Committed stay card-only columns, exactly as the design says. */
function HomeNodeList({ records, linkState, registerRow }: {
  records: readonly NodeRecord[]
  linkState: HomeReturnLinkState
  registerRow: NodeRowRefs
}) {
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
          {records.map(({ network, node }, index) => (
            <tr
              key={node.nodeId}
              data-slot="node-list-row"
              ref={(element) => registerRow(node.nodeId, element)}
            >
              <td data-column="name">
                <Link
                  to={'/nodes/' + node.nodeId}
                  state={linkState(node.nodeId, index, records.length)}
                  data-slot="node-link"
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

/**
 * Compact Home card, styled as Emerald's NodeCard: a stretched semantic title
 * link to Node Detail, an emerald status dot with a ping ring, a secondary
 * identity line carrying Network, process uptime and Peers, a three-column
 * resource row with thin progress bars, one shared Host network line, the
 * current Block Height with the evidence-backed Observed Network Head comparison
 * beside it (#237, #239), and the three-metric Validator summary or the short
 * identity line a confirmed status allows (#238). Copy, disclosure and
 * identity controls sit outside the anchor so they never trigger navigation.
 * Healthy Nodes carry no routine prose. Exceptional health keeps a short
 * diagnostic line (issue #97); resync progress is a separate, lightweight status
 * area, and every explanation the compaction removed from the face of the card
 * stays in the identity entry.
 */
function HomeNodeCard({ network, node, linkState }: NodeRecord & { linkState: HomeReturnState }) {
  const tone = healthTone(node.health)
  const diagnostic = exceptionalDiagnostic(node)
  const resyncing = (node.resyncState ?? '').toLowerCase() === 'resyncing'
  // The compact Peer cue the identity line can carry, and the explanations the
  // identity entry keeps in full: nothing the card compresses is lost.
  const peerCue = peerCardCue(node)
  const peerObservation = formatPeerObservation(node)
  const nodeDataBytes = formatNodeDataBytes(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)
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
      <CardX bordered={false} className="bg-transparent" contentClassName="gap-2.5" headerClassName="!block !p-0" header={<div data-node-region="identity" className="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-x-2 gap-y-1.5 px-4 pt-3 pb-2.5">
          <div className="flex min-w-0 items-center gap-2">
            <NodeHealthMarker health={node.health} />
            {/* The 44px target is kept on the anchor; the negative block margin
                keeps it from inflating the identity row, so the name row and
                the Network · Uptime row stay content-driven and can sit 6px
                apart. */}
            <h2 className="min-w-0 text-base font-semibold"><Link to={`/nodes/${node.nodeId}`} state={linkState} data-slot="node-link" aria-label={homeNodeLabel(node)} title={homeNodeLabel(node)} className="flex -my-2.5 min-h-11 min-w-0 items-center after:absolute after:inset-0 after:rounded-md focus-visible:outline-none focus-visible:after:ring-[3px] focus-visible:after:ring-ring/50"><span className="truncate">{homeNodeLabel(node)}</span></Link></h2>
          </div>
          <ValidatorActivityBadge validator={node.validator} identityReason={node.validatorIdentityReason} />
          <div data-slot="node-identity-meta" className="col-span-2 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
            {/* Second line (#237): the Node's environment at secondary weight —
                Network, process uptime and the Peer count. A long Network name
                wraps here instead of squeezing the count or the status cues, and
                the count itself never breaks: the identity explanation carries
                the whole name. */}
            <p className="min-w-0 flex-1">
              <span className="[overflow-wrap:anywhere]">{network.displayName}</span> · <span className="whitespace-nowrap">Uptime {formatDuration(node.processUptimeMs)}</span> · <span data-slot="node-peers-meta" className="whitespace-nowrap tabular-nums">Peers {formatPeerCount(node)}</span>
              {/* One short last-good/freshness cue: a retained count must never
                  read as a current one, and the full observation sentence stays
                  in the identity explanation this row already opens. */}
              {peerCue && <span data-slot="node-peers-cue" className="ml-1.5">{peerCue}</span>}
            </p>
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
                <p className="text-sm text-muted-foreground">Home Attention counts Active Nodes that are not Healthy, including Unknown. While nothing is narrowed, these counters and the Peer map read the whole Network selection. Once search, health, or Validator status narrows the list, they read the Active Nodes that matched instead, so a Node's countries count only where that Node is listed — while the two cumulative Validator cells keep the Network selection, and the list still reports how many Nodes matched.</p>
                <p data-slot="node-head-explanation" className="text-sm text-muted-foreground">Block Height: {headComparisonExplanation(node)}</p>
                <p className="text-sm text-muted-foreground">CPU and Memory are the monitored PlatON process's own percentages, not the Host's. Disk is the Node data directory's size as a share of the capacity of the filesystem that holds it, not whole-filesystem fullness.{nodeDataBytes ? ' Node data: ' + nodeDataBytes + '.' : ''}</p>
                {peerObservation && <p className="text-sm text-muted-foreground">Peer observation: {peerObservation}.</p>}
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
        </div>}>
        <div data-node-region="resource"><ResourceMetrics node={node} /></div>
        {/* Chain (#237, #239): Home keeps the current Block Height alone as one
            compact key-value line, and names where that height sits against the
            deployment-local Observed Network Head only while the Server's own
            evidence supports the comparison. QC, Locked, Committed and Txs stay
            on Node Detail, which reads the same public fields — no field,
            observation or history is deleted. */}
        <div data-node-region="chain">
        <div data-slot="node-business-metrics" className="border-t border-border pt-3">
          {/* One head line: the height row is the flexible track and the short
              comparison the fixed one, so the comparison shares the line while
              the card can hold both and drops to its own right-aligned line when
              the tightest track cannot — never overflowing, clipping or
              shrinking the reading itself. */}
          <div data-slot="node-head-line" className="flex flex-wrap items-baseline justify-end gap-x-2 gap-y-0.5">
            {/* An unusable height — absent, negative, fractional or non-finite —
                is a height the Server never validly observed, so it stays
                explicitly Unknown rather than being restated as a reading the
                reader could trust, and it supports no comparison either. */}
            <MetricRow className="min-w-0 flex-[1_1_auto]" layout="compact" label="Head" value={validHeight(node.currentHead) ? formatNumber(node.currentHead) : 'Unknown'} />
            <HeadComparison node={node} />
            <HeadRetainedMark node={node} />
          </div>
        </div>
        </div>
        <div data-node-region="validator"><LinkedValidatorSection node={node} /></div>
        </CardX>
    </article>
  )
}

/** Home's short form of the evidence-backed comparison (#239): the gated offset
 *  and its display tone are the Node Detail page's own (`networkHeadOffset`),
 *  and only this wording is Home's. An exact match is a state rather than a
 *  measurement, and a Node ahead of the reference is a leading state — never a
 *  clamped zero and never written as synchronized. */
function homeHeadComparison(node: PublicNode): { label: string; tone: HeadComparisonTone } | null {
  const offset = networkHeadOffset(node)
  if (offset === null) return null
  if (offset.blocks === 0) return { label: 'At observed head', tone: offset.tone }
  const blocks = Math.abs(offset.blocks)
  return {
    label: (offset.blocks > 0 ? 'Behind ' : 'Ahead ') + blocks.toLocaleString() + ' block' + (blocks === 1 ? '' : 's'),
    tone: offset.tone,
  }
}

/** The same retained-value mark Node Detail prints beside a value a failed
 *  collection kept (#239). Home's Block Height borrows both the word and its
 *  warning colour, so one vocabulary covers every retained reading. */
const HEAD_RETAINED_LABEL = 'Last-good'

/** What the deployment-local reference is, in the one sentence every surface
 *  that shows a comparison owes the reader: Home's hint title and the card's own
 *  explanation entry both state it, so neither can call it a canonical, public or
 *  explorer height, and neither claims synchronization from it. */
const OBSERVED_NETWORK_HEAD_NOTE =
  'The ' + OBSERVED_NETWORK_HEAD + " is this deployment's Server-side cross-Node reference, not the public chain head, and the comparison does not claim the Node is synchronized."

/** The short comparison beside the Block Height, drawn only while the Server's
 *  own evidence supports one. Its absence is the whole statement when it is not:
 *  the absolute height stays, and the card's explanation entry names what the
 *  comparison would have needed. */
function HeadComparison({ node }: { node: PublicNode }) {
  const comparison = homeHeadComparison(node)
  if (comparison === null) return null
  return (
    <small
      data-slot="node-head-comparison"
      data-tone={comparison.tone}
      title={'Compared with the ' + OBSERVED_NETWORK_HEAD + ': ' + comparison.label + '. ' + OBSERVED_NETWORK_HEAD_NOTE}
      className={cn('min-w-0 text-right text-[11px] leading-[1.375rem] [overflow-wrap:anywhere]', HEAD_COMPARISON_TONE_CLASS[comparison.tone])}
    >
      {comparison.label}
    </small>
  )
}

/** The short mark on a Block Height the Server is not confirming as current
 *  (#239, design §11.1 item 6): the value it publishes is the last one it
 *  retained, so the card must not let it read as an ordinary current reading.
 *  The mark can never share the line with a comparison — the Node's own
 *  observation is exactly what a comparison needs — and it borrows the detail
 *  page's own retained-value wording and colour. Its reason stays in the title
 *  and in the card's explanation entry; Home invents no freshness of its own. */
function HeadRetainedMark({ node }: { node: PublicNode }) {
  const reason = nodeHeadUnconfirmed(node)
  if (!reason) return null
  return (
    <small
      data-slot="node-head-retained"
      title={HEAD_RETAINED_LABEL + ' value: ' + reason + '. The Server is not confirming this as the Node’s current Block Height, so no comparison is asserted.'}
      className="min-w-0 text-right text-[11px] leading-[1.375rem] text-warning-foreground dark:text-warning [overflow-wrap:anywhere]"
    >
      {HEAD_RETAINED_LABEL}
    </small>
  )
}

/** The card's accessible account of the Block Height line (#239). It states what
 *  the absolute value is, which reference a comparison reads, why none is
 *  asserted when the evidence cannot support one, and that the comparison's
 *  colour is a display level rather than a new health, alert or Attention rule. */
function headComparisonExplanation(node: PublicNode): string {
  const retained = nodeHeadUnconfirmed(node)
  const absolute = retained
    ? 'the last Block Height the Server retained for this Node, marked ' + HEAD_RETAINED_LABEL + ' because ' + retained + '.'
    : 'the current head the Server published for this Node; a height the Server never observed stays Unknown rather than a fabricated zero.'
  const tone = 'Its colour is a display level only — an exact match uses the success colour, a lag of 1 to 10 blocks the warning colour, a longer lag the error colour, and a Node ahead of the reference stays neutral — never a health, alert or Attention rule.'
  const comparison = homeHeadComparison(node)
  const boundary = comparison !== null
    ? ' Shown here: ' + comparison.label + '.'
    : retained
      ? ' No comparison is asserted from a retained height.'
      : ' No comparison is asserted here: ' + (networkHeadComparison(node).reason ?? 'the observation evidence does not support one') + '.'
  return absolute + boundary + ' ' + OBSERVED_NETWORK_HEAD_NOTE + ' ' + tone
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
  // The shared guard only says whether a height is usable; the retained value is
  // what the percentage reads.
  const current = validHeight(node.currentHead) ? node.currentHead : null
  const target = validHeight(node.historicalHighWatermark) ? node.historicalHighWatermark : null
  // A percentage needs both operands: a valid current height and a valid,
  // computable Historical High-Water Mark target. Without them the cue states
  // the Server's own resync state alone — never an "Unknown" progress area and
  // never a fabricated zero — while the retained target and the real last
  // progress time stay in the same cue's explanation.
  const percent = current !== null && target !== null && target > 0
    ? (current / target * 100).toFixed(2) + '%'
    : null
  const timestamp = node.resyncLastProgressAt
  const date = validProgressDate(timestamp)
  return (
    <Dialog>
      <DialogTrigger asChild>
        {/* Keep the 44px touch target without reserving its full height in the
            compact metadata line, matching the neighbouring information entry. */}
        <button type="button" data-slot="node-resync-cue"
          className="relative z-10 -my-3.5 inline-flex min-h-11 items-center rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label={percent === null
            ? `Resync state: ${node.resyncState || 'unknown'}; no progress percentage is asserted`
            : `Resync progress: ${percent} toward the Historical High-Water Mark`}>
          <span className="inline-flex items-center gap-1 rounded border border-amber-500/40 px-1.5 py-0.5 text-[11px] font-medium leading-4 text-amber-500 dark:text-amber-400">
            <span>Resyncing</span>{percent !== null && <span className="tabular-nums">{percent}</span>}
          </span>
        </button>
      </DialogTrigger>
      <DialogContent className="max-h-[85dvh] overflow-y-auto rounded-md shadow-sm">
        <DialogTitle>Resync progress</DialogTitle>
        <DialogDescription>The last recorded advance toward the Historical High-Water Mark, not the latest report time. The target is the Node’s retained historical height, not the Network Head.</DialogDescription>
        <p className="m-0 text-sm tabular-nums">{formatNumber(current)} / {formatNumber(target)}</p>
        {percent === null && <p data-slot="resync-progress-unavailable" className="m-0 text-sm">No percentage is asserted: a percentage needs a valid current Block Height and a valid, computable Historical High-Water Mark target.</p>}
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

/** PlatPulse owns no occupancy threshold rule for process CPU/Memory or the
 *  Node data directory, so none is invented here: the ordinary fill uses
 *  Emerald's normal-state success token, the same green family as the online
 *  dot and the theme accent, instead of the informational blue or the default
 *  primary (white in the dark theme). A Server-owned warning/error rule would
 *  replace this, and it would still have to map onto the existing tokens: the
 *  ordinary resource bar keeps the ticket's own success green (#237). */
const RESOURCE_BAR_STATUS: ProgressStatus = 'success'

/** What the Disk bar measures (#237). The Node data directory's size over the
 *  capacity of the filesystem that holds it is not whole-filesystem fullness —
 *  the Host's own storage percentage is a different observation — so the bar's
 *  accessible name names the source it reads, and the identity explanation
 *  repeats the sentence beside the used / capacity bytes. */
/* The three bars share one Emerald success-green fill: it colours a reading the
 * Server published, never a verdict, so no occupancy threshold is invented and
 * the theme's default primary (white in the dark theme) is left for real
 * emphasis. A Server-owned rule can replace the constant without touching the
 * columns. */
const DISK_BAR_NAME = 'Disk: Node data directory size as a share of the filesystem capacity holding it, not whole-filesystem fullness'

type ResourceMetric = {
  metric: 'process-cpu' | 'process-memory' | 'node-data'
  /** The short name the three-column row can hold; the full source name stays
   *  the bar's accessible name and the label's title. */
  label: string
  name: string
  value: string
  progress: number | null
}

/** Home's three resource columns (#237): CPU and Memory are the monitored
 *  PlatON process's own percentages and Disk is the Node data directory's share
 *  of the filesystem capacity that holds it. Every column keeps the Emerald
 *  anatomy — muted label at the left, tabular value flush right on the same
 *  line, one thin success bar underneath — and the three bars share the row
 *  below the values, so a value that cannot share its label's line (an
 *  unattested one, or a very long ratio) drops to its own right-aligned line
 *  instead of overflowing, being clipped, or becoming a zero it never was. */
function ResourceMetrics({ node }: { node: PublicNode }) {
  const nodeDataRatio = nodeDataProgress(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)
  const metrics: ResourceMetric[] = [
    { metric: 'process-cpu', label: 'CPU', name: 'Process CPU', value: formatPercent(node.processCpuPercent), progress: node.processCpuPercent ?? null },
    // 'Mem' is the shortest honest form of the longest of the three labels: the
    // card's own minimum track must hold the label and its right-aligned
    // percentage on one line, and the full 'Process memory' name stays the
    // label's title and the bar's accessible name (the short-label form the
    // Linked Validator parameters already use).
    { metric: 'process-memory', label: 'Mem', name: 'Process memory', value: formatPercent(node.processMemoryPercent), progress: node.processMemoryPercent ?? null },
    { metric: 'node-data', label: 'Disk', name: DISK_BAR_NAME, value: formatPercent(nodeDataRatio), progress: nodeDataRatio },
  ]
  return (
    <div
      data-slot="node-resource-region"
      className="grid grid-cols-3 gap-x-2.5 gap-y-1"
      aria-label="Node process and host network resources"
    >
      {metrics.map(({ metric, label, name, value }) => (
        <div
          key={metric}
          data-slot="resource-cell"
          data-metric={metric}
          className="flex min-w-0 flex-wrap items-baseline gap-x-1 self-end"
        >
          <span data-slot="resource-label" title={name} className="text-[11px] leading-4 text-muted-foreground">{label}</span>
          <strong data-slot="resource-value" className="ml-auto whitespace-nowrap text-[13px] font-medium tabular-nums">{value}</strong>
        </div>
      ))}
      {metrics.map(({ metric, name, progress }) => (
        <ProgressThin key={metric} percentage={progress} status={RESOURCE_BAR_STATUS} label={name} />
      ))}
      <div className="col-span-3" data-slot="host-network-speed" role="group" aria-label="Host network speed">
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

/** The card's short half of `formatPeerObservation` (#237): the count itself is
 *  already on the identity line, so this says only what a reader needs in order
 *  not to misread it — a retained count is last good and why, a successful
 *  empty snapshot is an authoritative zero, and a count the Server never
 *  attested has no successful snapshot behind it. A fully current count says
 *  nothing extra; the full dimension sentence stays in the identity
 *  explanation. */
function peerCardCue(node: PublicNode): string | undefined {
  const value = peerInsightValueStatus(node.peers)
  if (value === 'Unknown') return 'no successful snapshot'
  const retention = peerRetentionCue(node)
  if (retention) return retention
  return value === 'Empty' ? 'Empty; authoritative zero' : undefined
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
