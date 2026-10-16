import { useCallback, useEffect, useId, useMemo, useRef, useState, type ChangeEvent, type KeyboardEvent } from 'react'
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
import { Server, HeartPulse, TriangleAlert, Network, ChevronUp, ChevronDown, Info, Search, SlidersHorizontal, X } from 'lucide-react'
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
import { LinkedValidatorSection, validatorDataStatus, validatorIdentityStatement, type ValidatorDataStatus } from './LinkedValidator'
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

/** The one sort control's keys, in `HOME_SORTS` order (#223, #242): the six the
 *  delivered toolbar already offered, then Disk, Uptime and Attention First. No
 *  direction control is added — each key's direction is the key itself, stated
 *  once in `order` so the entry's hint never restates a direction of its own.
 *  `short` is the word the entry prints beside its own name while a key other
 *  than the default is in force, and `label` stays the full name the select and
 *  the description use (#246). */
const sortOptions: Array<{ value: HomeSort; label: string; short: string; order: string }> = [
  { value: 'health', label: 'Health', short: 'Health', order: 'Unhealthy first, then Healthy, then Unknown' },
  { value: 'name', label: 'Name', short: 'Name', order: 'Node name, A to Z' },
  { value: 'head', label: 'Current Head', short: 'Head', order: 'highest Block Height first, never-observed Nodes last' },
  { value: 'peers', label: 'Peers', short: 'Peers', order: 'most Peers first, never-observed Nodes last' },
  { value: 'process_cpu', label: 'Process CPU', short: 'CPU', order: 'highest process CPU first, never-observed Nodes last' },
  { value: 'process_memory', label: 'Process memory', short: 'Memory', order: 'highest process memory first, never-observed Nodes last' },
  { value: 'disk', label: 'Disk', short: 'Disk', order: 'largest Node data directory share first, never-observed Nodes last' },
  { value: 'uptime', label: 'Uptime', short: 'Uptime', order: 'longest process uptime first, never-observed Nodes last' },
  { value: 'attention', label: 'Attention First', short: 'Attention', order: 'Unhealthy first, then Unknown, then Healthy' },
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
    // The draft may outlive the address bar by one commit and no more: a keystroke
    // writes the URL a frame later, and a blur that lands before that frame would
    // otherwise leave the typed word owned by nobody. Once the address bar agrees
    // with the draft, or empties, the draft has nothing left to carry. The setter
    // reads the draft as it stands when the address bar moves — this effect fires
    // on that move and not on the reader's keystroke, so a word whose URL has not
    // landed yet keeps its owner instead of being dropped mid-word.
    setQueryDraft((draft) => (filters.query === '' || filters.query === draft ? null : draft))
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
  // The order is the entry's second state, and it is stated twice for the same
  // reason the count is: the entry's own text track names it while the row has the
  // width for a word, and the description names it — with the direction the key
  // sorts in — at every width, so a reader who sees neither the track nor the
  // toolbar still knows what the list is ordered by. The default key is the
  // Health order the card has always led with, so the track keeps the entry's own
  // name and no description is added: an entry describing its own default is
  // noise (#246).
  const sortKey = sortOptions.find((option) => option.value === filters.sort) ?? sortOptions[0]
  const sortStateId = 'home-sort-state'
  const sortInForce = filters.sort !== 'health'
  const sortDescription = `Sort is ${sortKey.label}: ${sortKey.order}.`
  const filtersTriggerDescribedBy = sortInForce ? `${statusFilterCountId} ${sortStateId}` : statusFilterCountId
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
    placeholder: 'Search nodes...',
    value: queryText,
    onChange: (event: ChangeEvent<HTMLInputElement>) => {
      setQueryDraft(event.target.value)
      updateFilters({ query: event.target.value }, true)
    },
    onBlur: () => {
      // The commit a keystroke asks for is painted a frame after the keystroke,
      // so dropping the draft the moment the field loses focus would read an
      // address bar that does not carry the text yet: for that one frame the
      // query would be empty, the field would show the reader's text gone and
      // the two marks that state an active search would leave the row — right
      // as the reader's Tab is on its way to the Clear search they stand
      // beside, which then has no control to land on. The draft stands until
      // the address bar says the same thing, which is the moment the text stops
      // being the box's and becomes the URL's.
      if (filters.query === queryDraft) setQueryDraft(null)
    },
    onKeyDown: (event: KeyboardEvent<HTMLInputElement>) => {
      if (event.key === 'Escape') event.preventDefault()
    },
  } as const

  // The accessible explanation of the one sort key whose name does not say what
  // it leads with; it travels with the control that selects it.
  const attentionSortHintId = useId()
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
  // the list states the reason in words instead of a count. Without a
  // projection the counters keep reading 'Unknown', which is a
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
          {/* The one tile whose figure is a health verdict rather than a plain
              count, so it is the one tile that states what it counts (#236 round
              2). The caption names the two Server verdicts and the title states
              the boundary: chain-height differences and Observing never enter it. */}
          <SummaryCard label="Attention" value={attentionCount}
            tone={attentionCount === null || attentionCount === 0 ? 'green' : 'red'} icon="alert" emptyLabel={emptyLabel}
            caption={ATTENTION_CAPTION} hint={ATTENTION_HINT} />
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
            className="flex flex-wrap items-center justify-end gap-2 md:flex-nowrap"
          >
            {/* The Network selection is the row's left-hand group and the three
                controls a reader reaches for after it are its right-hand one, so the
                two ends of the row are held by the two ends of the toolbar instead of
                one pile of controls with the rest of the row left empty. The Network
                group grows into whatever the right-hand group does not use, and a
                Network name longer than the room it has scrolls inside this group
                rather than pushing the controls beside it off the row. */}
            <div
              data-slot="network-group"
              className="relative z-10 min-w-0 grow overflow-x-auto rounded-sm py-1.5 -my-1.5"
            >
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
            {/* One flex item holds the search control, the Sort & filter entry and the
                card/list choice, in the order they are read and tabbed, and the row
                right-aligns the group as a whole (justify-end above) rather than
                giving each control its own ml-auto: the three stay one block with one
                set of gaps, beside the Network group on a desktop row, on a line of
                their own under it where the row cannot hold both, and at the row's
                right edge in either case. */}
            {/* The group is a grid, and not a flex row, for one reason: its own width
                has to be the 17rem search slot plus the two controls beside it, while
                the smallest width it can be given has to stay the three controls'
                own minimum — 44px for the slot and the two entries at their own
                widths. A first track of minmax(44px, 17rem) is exactly that pair: the
                track's maximum keeps the group's width at the slot plus the entries,
                so a row with the room gives the reader the field, and the track's
                44px minimum keeps the group's floor at the controls' minimum, so a
                row without the room — a long Network list pressing in — hands the
                shortage to the Network group, which scrolls its pills, instead of
                drawing a control outside this box and over the pills. A definite
                width on the slot cannot say both: it would also be the slot's
                smallest contribution, which lifts the group's floor to 17rem plus the
                two entries and leaves the Network group to pay for every shortfall. */}
            <div
              data-slot="home-toolbar-actions"
              className="grid grid-flow-col grid-cols-[minmax(2.75rem,17rem)_auto_auto] items-center justify-end gap-2"
            >
              {/* The search control is one slot that asks for 17rem — the field's
                  readable 13rem, the 44px Clear search action inside the field's own
                  box, the 4px between them, and the 1rem that keeps the field off the
                  edge of that readable width — and gives that room back to the
                  controls beside it when the row is narrower than the three of them
                  need. The first track of the grid above holds both ends of that ask,
                  17rem as its maximum and 44px as its minimum, and this slot is given
                  the track's width; its own min-w-11 restates that floor for the one
                  control that can shrink. The slot's width never depends on which of
                  its two shapes is displayed, which is what lets the width query in
                  emerald.css ask the row's one question — is there room for a compact
                  field beside the controls that are already here? — and get the same
                  answer whichever shape a reader is shown. */}
              <div
                ref={searchSlotRef}
                data-slot="home-search-slot"
                className="flex min-w-11 items-center"
              >
                {/* The compact field fills the slot, so the field a reader is given is
                    never a squeezed version of this one, and this shape is only
                    displayed where the slot holds all of it. Its Search icon and its
                    Clear search action sit inside the field's own box, and the clear
                    action is drawn only while there is a word to clear: the field
                    carries one clear action, never two (the browser's own search-input
                    clear button is switched off in emerald.css for the same reason). */}
                <div
                  data-slot="home-search-inline"
                  className="w-full items-center"
                >
                  <div className="relative flex w-full items-center">
                    <Search aria-hidden="true" className="pointer-events-none absolute left-2.5 size-4 text-muted-foreground" />
                    <Input
                      ref={searchFieldRef}
                      data-slot="home-search-field"
                      data-search-state={searchActive ? 'on' : 'off'}
                      aria-describedby={searchActive ? searchStateId : undefined}
                      className={cn('w-full pl-8', searchActive ? 'pr-11 border-emerald-600 dark:border-emerald-500' : 'pr-2')}
                      {...searchFieldProps}
                    />
                    {searchActive && (
                      <Button
                        {...clearSearchProps}
                        data-slot="home-search-clear"
                        className="absolute top-1/2 right-0 -translate-y-1/2"
                      >
                        <X aria-hidden="true" />
                      </Button>
                    )}
                  </div>
                </div>
                {/* The other shape of that one control: the icon-only entry that opens
                    the same field. It is drawn by the same line-icon set as the
                    card/list choice beside it, and it is the only shape whose search
                    sits behind a control. */}
                <div
                  data-slot="home-search-collapsed"
                  className={cn('relative', searchOpen ? 'z-30' : 'z-10')}
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
                    <div className="relative flex w-full items-center">
                      <Search aria-hidden="true" className="pointer-events-none absolute left-2.5 size-4 text-muted-foreground" />
                      <Input
                        ref={searchInputRef}
                        data-slot="home-search-panel-field"
                        className={cn('w-full pl-8', searchActive ? 'pr-11' : 'pr-2')}
                        {...searchFieldProps}
                      />
                      {searchActive && (
                        <Button
                          {...clearSearchProps}
                          className="absolute top-1/2 right-0 -translate-y-1/2"
                        >
                          <X aria-hidden="true" />
                        </Button>
                      )}
                    </div>
                  </ToolbarPopover>
                </div>
              </div>
              {/* The sort order and the two status filters share one toolbar control
                  (issue #232): the row keeps the Network group, this entry, the
                  card/list choice and the search control, and every control this entry
                  hides leaves the tab walk with it. The badge counts the status
                  filters, the only choices this entry represents that the row could
                  not otherwise show, and it is drawn only while there is one: a zero
                  badge would be a chip that says nothing. The count itself is still
                  stated for a reader that cannot see the chip
                  (statusFilterCountId), and the order in force is stated the same way
                  while it is not the default one (sortStateId, #246). */}
              <div data-slot="sort-filter-entry" className={cn('relative', filtersOpen ? 'z-30' : 'z-10')}>
                <ToolbarPopover
                  open={filtersOpen}
                  onClose={closePanel}
                  returnFocusRef={filtersTriggerRef}
                  label="Sort and filters"
                  align="end"
                  className="flex w-72 flex-col gap-2.5 p-2.5"
                  trigger={
                    <Button
                      ref={filtersTriggerRef}
                      variant="outline"
                      aria-expanded={filtersOpen}
                      aria-haspopup="dialog"
                      aria-describedby={filtersTriggerDescribedBy}
                      aria-label="Sort & filter"
                      data-slot="home-filter-trigger"
                      className={cn('gap-1.5 rounded-md px-2.5 text-xs shadow-none md:bg-background', SURFACE_TOOLBAR)}
                      onClick={() => togglePanel('filters')}
                    >
                      <SlidersHorizontal aria-hidden="true" className="size-3.5" />
                      {/* One fixed text track carries the entry's own name, and the
                          order in force while there is one (#246), so every state of
                          this entry is the same width: changing the order - from the
                          default, back to it, or between two others - moves neither
                          the entry nor the search control and the pills beside it,
                          which is what keeps the row from jumping while the surface
                          above it is open. The words are decorative: the entry names
                          itself (aria-label), and the order reaches a reader who
                          cannot see the track through the description below
                          (sortStateId), which states the key and the direction it
                          sorts in at every width. Below lg the track is not drawn and
                          the entry keeps its name alone, because a row that narrow
                          has no width to spare for a second word. The surface is
                          anchored to this entry's end edge (align="end"), the edge
                          the toolbar pins, so a track stating something new cannot
                          slide the surface out from under the reader. */}
                      <span aria-hidden="true" className="lg:hidden">Sort &amp; filter</span>
                      <span
                        data-slot="home-filter-sort"
                        aria-hidden="true"
                        className="hidden w-[5.5rem] shrink-0 truncate text-left lg:inline-block"
                      >
                        {sortInForce
                          ? <span className="text-[0.6875rem] font-medium text-muted-foreground">Sort: {sortKey.short}</span>
                          : 'Sort & filter'}
                      </span>
                      {statusFilterCount > 0 && (
                        <span
                          data-slot="home-filter-count"
                          aria-hidden="true"
                          className="inline-flex h-4 min-w-4 items-center justify-center rounded-full bg-emerald-600 px-1 text-[0.625rem] font-semibold leading-none tabular-nums text-white dark:bg-emerald-500 dark:text-emerald-950"
                        >
                          {statusFilterCount}
                        </span>
                      )}
                    </Button>
                  }
                >
                    {/* Three rows rather than three stacked labels (#246): a label
                        and its select share one line, so the surface is about half
                        its former height and hides far less of the cards below it,
                        and the three selects share one left edge because the label
                        column is one fixed 5.5rem track. Each select keeps its own
                        accessible name, its 44px target and its keyboard behaviour;
                        only the layout around them changed. The rows are written in
                        the order a reader works in — the order the list is in, then
                        the two filters that narrow it — and one low-contrast rule
                        separates the two concerns. */}
                    <label className="flex items-center gap-2.5 text-xs font-medium text-muted-foreground">
                      <span className="w-[5.5rem] shrink-0 whitespace-nowrap">Sort by</span>
                      <Select
                        aria-label="Sort"
                        aria-describedby={filters.sort === 'attention' ? attentionSortHintId : undefined}
                        className="min-h-11 flex-1 text-xs"
                        value={filters.sort}
                        onChange={(event) => updateFilters({ sort: event.target.value as HomeSort })}
                      >
                        {sortOptions.map((option) => (
                          <option key={option.value} value={option.value}>{option.label}</option>
                        ))}
                      </Select>
                    </label>
                    {/* Attention First is the one key whose name does not say what it
                        leads with, so its order is described where it is chosen. The
                        note is a sibling of the label rather than a child of it, so it
                        never joins the control's label text; it is sr-only, so the
                        panel's visible layout is untouched. */}
                    {filters.sort === 'attention' && (
                      <span id={attentionSortHintId} className="sr-only">
                        Attention First leads with the Active Nodes the Server has not confirmed Healthy, Unhealthy before Unknown, and lists the Healthy ones last.
                      </span>
                    )}
                    <span aria-hidden="true" data-slot="home-filter-separator" className="w-full border-t border-border/60" />
                    <label className="flex items-center gap-2.5 text-xs font-medium text-muted-foreground">
                      <span className="w-[5.5rem] shrink-0 whitespace-nowrap">Health</span>
                      <Select
                        aria-label="Health filter"
                        className="min-h-11 flex-1 text-xs"
                        value={filters.health}
                        onChange={(event) => updateFilters({ health: event.target.value as HomeHealthFilter })}
                      >
                        <option value="all">All health</option>
                        <option value="healthy">Healthy</option>
                        <option value="unhealthy">Unhealthy</option>
                        <option value="unknown">Unknown</option>
                      </Select>
                    </label>
                    <label className="flex items-center gap-2.5 text-xs font-medium text-muted-foreground">
                      <span className="w-[5.5rem] shrink-0 whitespace-nowrap">Validator</span>
                      <Select
                        aria-label="Validator status filter"
                        className="min-h-11 flex-1 text-xs"
                        value={filters.validator}
                        onChange={(event) => updateFilters({ validator: event.target.value as HomeValidatorFilter })}
                      >
                        <option value="all">All Validator status</option>
                        <option value="validator">Validator</option>
                        <option value="not_validator">Not a Validator</option>
                        <option value="unknown">Unknown</option>
                      </Select>
                    </label>
                    {/* The footer exists only while it has something to do (#246):
                        the surface prints the count and its clear action while a
                        status filter is on, and says nothing at all while both
                        filters are at their default — no sentence about an absent
                        filter, and no action drawn disabled for a reader to wonder
                        about. */}
                    {statusFilterCount > 0 && (
                      <div
                        data-slot="home-filter-footer"
                        className="flex items-center justify-between gap-2 border-t border-border/60 pt-2"
                      >
                        <p className="text-xs text-muted-foreground">{statusFilterSummary}</p>
                        <Button variant="ghost" size="sm" className="px-2 text-xs" onClick={clearStatusFilters}>
                          Clear filters
                        </Button>
                      </div>
                    )}
                </ToolbarPopover>
                <span id={statusFilterCountId} className="sr-only">{statusFilterSummary}</span>
                {sortInForce && (
                  <span id={sortStateId} data-slot="home-sort-state" className="sr-only">{sortDescription}</span>
                )}
              </div>
              {/* The card/list choice is part of the same ordinary Home URL contract as
                  the filters and the sort (design §9). It is the last control of the
                  right-hand group, so the row's own right edge is this choice's edge
                  at every width. The two choices keep their words rather than gaining
                  icons: a phone row leaves 268px for this control and the Sort &
                  filter entry together, and a pair of line icons would spend 40px of
                  it on a segmented control whose labels already say which shape it
                  selects. */}
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

/** Home Attention's visible reading (#236 round 2): the tile counts exactly the
 * two Server verdicts that are not Healthy, so the caption names them instead
 * of leaving the figure to be guessed at. */
const ATTENTION_CAPTION = 'Unhealthy or Unknown'
/** The tile's longer reading, exposed as its title: what the figure counts, and
 * what it deliberately does not — a chain-height difference never counts, and
 * an Observing or otherwise Unknown observation is not an abnormal state. */
const ATTENTION_HINT = 'Home Attention counts Active Nodes the Server has not confirmed Healthy: Unhealthy or Unknown. A chain-height difference never counts, and Observing is not an abnormal state.'

const SUMMARY_ICONS = {
  server: Server,
  heart: HeartPulse,
  alert: TriangleAlert,
  network: Network,
} as const

function SummaryCard({ label, value, tone, icon, emptyLabel, caption, hint }: {
  label: string; value: number | null; tone: 'green' | 'red'; icon: keyof typeof SUMMARY_ICONS
  /** Copy for an explicit empty state; without it a null value reads 'Unknown'. */
  emptyLabel?: string
  /** Short visible reading under the value; the slot is reserved on every tile,
   *  so adding one changes no tile's height. */
  caption?: string
  /** The longer reading, kept as the tile's title. */
  hint?: string
}) {
  return <SummaryMetricCard label={label} value={value === null ? emptyLabel ?? 'Unknown' : value.toLocaleString()}
    tone={value !== null && value > 0 ? tone : 'green'} icon={SUMMARY_ICONS[icon]} caption={caption} hint={hint} />
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
  // The identity line the card shows when the Server does not confirm a
  // Validator, and the same statement the identity entry keeps in full — so the
  // short face and the explanation can never disagree (#238 round 2).
  const identityStatement = node.validator ? validatorIdentityStatement(node.validator) : null
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
                <p className="text-sm text-muted-foreground">Home Attention counts Active Nodes that are not Healthy, including Unknown. It counts Active Nodes and nothing else: a chain-height difference never enters it, and Observing is not an abnormal state. While nothing is narrowed, these counters and the Peer map read the whole Network selection. Once search, health, or Validator status narrows the list, they read the Active Nodes that matched instead, so a Node's countries count only where that Node is listed — while the two cumulative Validator cells keep the Network selection, and the list still reports how many Nodes matched.</p>
                <p data-slot="node-head-explanation" className="text-sm text-muted-foreground">Block Height: {headComparisonExplanation(node)}</p>
                <p className="text-sm text-muted-foreground">CPU and Memory are the monitored PlatON process's own percentages, not the Host's. Disk is the Node data directory's size as a share of the capacity of the filesystem that holds it, not whole-filesystem fullness. Speed is the Agent Host's own interface throughput, drawn on every Node card that Host reports for, so it is not a rate attributable to this Node alone.{nodeDataBytes ? ' Node data: ' + nodeDataBytes + '.' : ''}</p>
                {peerObservation && <p className="text-sm text-muted-foreground">Peer observation: {peerObservation}.</p>}
                {diagnostic && <p data-slot="node-diagnostic-explanation" className="text-sm text-muted-foreground">Current health diagnostic: {diagnostic.detail}</p>}
                {identityStatement && <p data-slot="node-validator-identity-explanation" className="text-sm text-muted-foreground">Validator identity: {identityStatement.label} — {identityStatement.detail}</p>}
                {dataStatus && <p className="text-sm text-muted-foreground">Validator data: {dataStatus.label}. {dataStatus.description}</p>}
              </DialogContent>
            </Dialog>
            </span>
          </div>
          {/* Abnormal reasons remain readable in one short line, and the Server's
              own sentence stays reachable as that line's title and in the identity
              entry. Healthy cards create no empty row. */}
          {diagnostic && <p
            data-slot="node-diagnostic"
            data-tone={diagnostic.tone}
            title={diagnostic.detail}
            className={cn(
              'col-span-2 m-0 text-[11px] [overflow-wrap:anywhere]',
              diagnostic.tone === 'destructive' ? 'text-destructive' : 'text-warning-foreground dark:text-warning',
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

/** The share of the retained target a Resyncing Node has reached, in the forms
 *  the cue needs. Two decimals keep the chip one width, but a Node at 99.996%
 *  would then print `100.00%` — a finished sync the Server has not reported — so
 *  a share that is below the target while rounding to `100.00%` is printed as
 *  `≈100%` instead, and the dialog keeps both figures. The Server's own
 *  `resyncState` stays the authority for the state: a share of exactly 100% is
 *  not "rounded away" but shown as recorded beside the state that disagrees
 *  with it, and neither number ever ends a Resync the Server still reports. */
type ResyncProgress = {
  /** The share with two decimals, the form the chip prints. */
  rounded: string
  /** True while the share is short of the target but rounds to `100.00%`. */
  approximate: boolean
  /** The share in a reading that stays below the target, or null where no
   *  reading this component can print does. */
  share: string | null
}

const APPROXIMATE_TARGET_LABEL = '≈100%'

/** What the chip prints when the rounded share is short of the target but would
 *  read as a finished sync: the approximation mark, not the misleading number. */
function resyncProgress(current: number | null, target: number | null): ResyncProgress | null {
  if (current === null || target === null || target <= 0) return null
  const share = current / target * 100
  const rounded = share.toFixed(2) + '%'
  return { rounded, approximate: share < 100 && rounded === '100.00%', share: readableShare(share) }
}

/** The share with enough decimals to still read as short of the target: every
 *  place two decimals drop is restored, up to a point — a share so close to the
 *  target that ten decimals still print `100.00…` is stated as below the target
 *  or, where no reading this helper can offer stays below it, in the dialog's
 *  own words rather than as a number that claims it was reached. */
function readableShare(share: number): string | null {
  for (const digits of [4, 6, 8, 10]) {
    const text = share.toFixed(digits).replace(/0+$/, '').replace(/\.$/, '')
    if (Number(text) < 100) return text + '%'
  }
  return null
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
  const progress = resyncProgress(current, target)
  // The rounded share is what the chip prints, except where it would read as a
  // finished sync the Server has not reported: there the chip prints the
  // approximation and the dialog states both figures.
  const percent = progress === null ? null : progress.approximate ? APPROXIMATE_TARGET_LABEL : progress.rounded
  const timestamp = node.resyncLastProgressAt
  const date = validProgressDate(timestamp)
  return (
    <Dialog>
      <DialogTrigger asChild>
        {/* Keep the 44px touch target without reserving its full height in the
            compact metadata line, matching the neighbouring information entry. */}
        <button type="button" data-slot="node-resync-cue"
          className="relative z-10 -my-3.5 inline-flex min-h-11 items-center rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label={progress === null
            ? `Resync state: ${node.resyncState || 'unknown'}; no progress percentage is asserted`
            : progress.approximate
              ? 'Resync progress: approximately 100% toward the Historical High-Water Mark, short of the target; the share is stated in the dialog'
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
        {/* The cue cannot print a share and its own rounding in one chip, so the
            dialog prints both: the closest reading this card prints of a share that
            stays below the target, the figure two decimals produce, and the reason
            the state outranks the number. Where even that closest reading prints
            100, the dialog says so in words rather than setting a number beside the
            word Resyncing that claims the target was reached (#246). */}
        {progress !== null && progress.approximate && (
          <p data-slot="resync-progress-rounding" className="m-0 text-sm tabular-nums">
            {progress.share === null
              ? `Below the Historical High-Water Mark by less than any decimal this card prints, where two decimal places print ${progress.rounded}`
              : `Below the Historical High-Water Mark: the closest reading this card prints is ${progress.share}, where two decimal places print ${progress.rounded}`}
            {`, which is why the cue reads ${APPROXIMATE_TARGET_LABEL} rather than a finished percentage. The Resyncing state stays the Server's own: a rounded number neither reaches the target nor ends the Resync.`}
          </p>
        )}
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

/** What the Speed line reads (#246): the rate is the Agent Host's own interface
 *  throughput, collected once per Host and drawn on every Node card that Host
 *  reports for, so it is a shared Host observation and never this Node's own
 *  traffic — one Node's card cannot separate its share of it, and the label says
 *  so rather than letting the word "Speed" imply a per-Node meter. The card keeps
 *  its height: the same sentence is the label's tooltip, and the identity entry
 *  repeats it beside the CPU, Memory and Disk definitions. */
const HOST_NETWORK_SPEED_NAME = "Host network speed: this Agent Host's own interface throughput, shared with every Node the Host reports for — not a rate attributable to this Node alone"

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
        <MetricRow layout="compact" label="Speed" labelTitle={HOST_NETWORK_SPEED_NAME} value={<span className="flex items-center gap-2 whitespace-nowrap">
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

type NodeDiagnostic = {
  /** The one short line the card header shows. */
  text: string
  /** The Server's own reason, unshortened: the card line's title and the
   *  identity entry carry it, so nothing the Server said is lost. */
  detail: string
  tone: 'destructive' | 'warning'
}

/** How long a Server reason may run before the card line shortens it; the
 * Server's own short sentences ('RPC observation failed', 'node is retired')
 * stay untouched below it. */
const DIAGNOSTIC_MAX_CHARS = 40

/** Server reasons whose own wording is long enough to crowd the card header,
 * each with the shorter reading that says the same thing. */
const DIAGNOSTIC_SHORT_FORMS: ReadonlyArray<readonly [RegExp, string]> = [
  [/stale or unknown/i, 'Observations stale / unknown'],
  [/unknown or unsupported/i, 'Observations unknown or unsupported'],
]

/** The card header shows one short line, so a long Server sentence becomes its
 * own short form or is cut at a word boundary. The full reason stays in the
 * line's title and in the identity entry, so the compaction never hides why a
 * Node is exceptional (#97 round 2). */
function shortDiagnostic(detail: string): string {
  const short = DIAGNOSTIC_SHORT_FORMS.find(([pattern]) => pattern.test(detail))?.[1]
  if (short) return short
  if (detail.length <= DIAGNOSTIC_MAX_CHARS) return detail
  const cut = detail.slice(0, DIAGNOSTIC_MAX_CHARS)
  const boundary = cut.lastIndexOf(' ')
  return (boundary > 0 ? cut.slice(0, boundary) : cut).trimEnd() + '…'
}

/** One sanitized health diagnostic line for exceptional Nodes (issue #97).
 * Resync progress is independent and never hidden by an unhealthy diagnostic.
 * The tone follows the Server's own verdict: Unhealthy is a confirmed failure
 * and takes the error tone, while Unknown is missing, stale, or never-observed
 * evidence — a warning, never a confirmed failure and never Offline. */
function exceptionalDiagnostic(node: PublicNode): NodeDiagnostic | null {
  if (node.health !== 'healthy') {
    const detail = node.healthReason?.trim() || `Health ${healthLabel(node.health).toLowerCase()}`
    return {
      text: shortDiagnostic(detail),
      detail,
      tone: node.health === 'unhealthy' ? 'destructive' : 'warning',
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
