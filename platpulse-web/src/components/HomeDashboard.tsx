import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useLocation, useSearchParams } from 'react-router'
import { Server, HeartPulse, TriangleAlert } from 'lucide-react'
import type { PublicNetwork } from '../api/generated'
import { realtimeStreamLabel } from './RealtimeNotice'
import GeoMapBoundary from './GeoMapBoundary'
import GeoWorldMap from './GeoWorldMap'
import { geoMapStatus, homeGeoOverview, homeNodeSelectionGeoOverview } from '../homeGeo'
import { Alert, AlertDescription } from './ui/alert'
import { Empty } from './ui/empty'
import {
  healthCategory,
  homeNetworkScope,
  readHomeFilters,
  selectHomeRecords,
  writeHomeFilters,
  type HomeFilterRejection,
  type HomeFilters,
} from '../homeFilters'
import {
  homeReturnState,
  homeReturnTab,
  landHomeReturn,
  readHomeReturn,
  type HomeReturn,
} from '../homeReturn'
import { ValidatorTotalCard } from './ValidatorTotals'
import { SummaryMetricCard } from './SummaryMetricCard'
import { RewardWalletCard, type RewardWalletReading } from './RewardWalletCard'
import { OverviewBand } from './OverviewBand'
import { HomeToolbar } from './HomeToolbar'
import { HomeNodeCard } from './HomeNodeCard'
import { HomeNodeList, type NodeRecord, type HomeReturnLinkState } from './HomeNodeList'

export { RATE_DIRECTION_CLASSES, HomeResourceMetrics } from './HomeResourceMetrics'
export { HomeToolbar } from './HomeToolbar'
export { HomeNodeCard } from './HomeNodeCard'
export { HomeNodeList } from './HomeNodeList'

export type HomeDashboardProps = {
  networks: PublicNetwork[]
  realtimeStatus: 'connecting' | 'connected' | 'disconnected'
  online: boolean
  resetting: boolean
  error: string | null
  hasLastGood?: boolean
  loading: boolean
  /** True while the projection on screen is being refetched: the reading the
   *  arrival places the reader in is then still provisional (webui.md §15.25). */
  refreshing?: boolean
  /** The Reward Wallet Balance query (#246). It is independent of the Network
   *  projection above, so it is handed over as its own reading. */
  rewardWallet?: RewardWalletReading
  /** Search input debounce in milliseconds before syncing to URL / filter pipeline. */
  searchDebounceMs?: number
}

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
  rewardWallet = { status: 'loading' },
  searchDebounceMs = 150,
}: HomeDashboardProps) {
  const [search, setSearch] = useSearchParams()
  // An ordinary Home URL is the whole filter state (design §9.1, #222): the
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
  // at its new place when the settled order moved it.
  const pendingReturn = useRef<{ departure: HomeReturn; search: string } | null>(null)

  // The landing this arrival has already been given: the row it placed and the
  // notice it showed, with whether the projection it was read from was still
  // being refetched.
  const landedReturn = useRef<{ index: number | null; notice: string | null; provisional: boolean } | null>(null)

  // Rows are located through React's own refs, keyed by Node ID: the returned
  // row is the one this render put on screen, never a private DOM lookup.
  const nodeRows = useRef(new Map<string, HTMLElement>())
  const registerNodeRow = (nodeId: string, element: HTMLElement | null) => {
    if (element === null) nodeRows.current.delete(nodeId)
    else nodeRows.current.set(nodeId, element)
  }
  const nodeLinkState: HomeReturnLinkState = (nodeId, index, count) =>
    homeReturnState(nodeId, search, index, count, homeReturnTab())

  // One arrival, and every landing it is still owed.
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

  /** The arrival's notice has been answered and the departure behind it is no
   *  longer owed a landing: the reader has arrived at a reading of their own. */
  const finishReturn = useCallback(() => {
    pendingReturn.current = null
    landedReturn.current = null
    setReturnNotice(null)
  }, [])

  useEffect(() => {
    const takeOver = () => {
      finishReturn()
    }
    window.addEventListener('popstate', takeOver)
    return () => window.removeEventListener('popstate', takeOver)
  }, [finishReturn])

  // One reader action can reach this toolbar twice: a Network pill reports its
  // value on focus and again on press, and asking the address bar for the same
  // URL twice would leave a Back step that appears to do nothing.
  const issuedSearch = useRef<string | null>(null)
  const renderedSearch = search.toString()
  useEffect(() => {
    issuedSearch.current = null
  }, [renderedSearch])

  /** A discrete choice is a step the reader can undo; typing in the search box
   *  replaces the current entry so history does not grow per keystroke. */
  const updateFilters = useCallback(
    (patch: Partial<HomeFilters>, replace = false) => {
      const next = writeHomeFilters({ ...filters, network: scope.network, ...patch }, search)
      const nextSearch = next.toString()
      if (nextSearch === renderedSearch || nextSearch === issuedSearch.current) return
      issuedSearch.current = nextSearch
      finishReturn()
      setSearch(next, { replace })
    },
    [filters, finishReturn, renderedSearch, scope.network, search, setSearch],
  )

  const scopeName =
    scope.network === 'all'
      ? null
      : networks.find((network) => network.networkKey === scope.network)?.displayName ?? scope.network

  // The Network selection's own Networks: the scope of the two cumulative
  // Validator cells, which never follow a narrowed list (#233).
  const scopedNetworks = useMemo(
    () => (scope.network === 'all' ? networks : networks.filter((network) => network.networkKey === scope.network)),
    [networks, scope.network],
  )

  // The list selection is what a filter narrows. While it holds every Active
  // Node of the Network selection nothing is narrowed, and every reading stays
  // the one Home has always shown.
  const selectionNarrowed = matching.length !== scoped.length
  const matchedNodes = useMemo(() => matching.map(({ node }) => node), [matching])
  const healthyCount = hasProjection
    ? matching.filter(({ node }) => healthCategory(node.health) === 'healthy').length
    : null

  // A narrowed selection that matches no Node leaves the whole band in an
  // explicit empty state (#233, decision 8): the three counters that follow the
  // list read no figure at all — never a zero, and never a failure tone — while
  // the list states the reason in words instead of a count.
  const emptySelection = hasProjection && selectionNarrowed && matching.length === 0
  const emptyLabel = emptySelection ? 'No match' : undefined
  const activeNodeCount = emptySelection ? null : hasProjection ? matching.length : null
  const selectionHealthyCount = emptySelection ? null : healthyCount
  const attentionCount =
    activeNodeCount === null || selectionHealthyCount === null ? null : activeNodeCount - selectionHealthyCount

  // The map receives a projection, never raw Network input.
  const selectionLabel = scopeName ? 'the current list selection in ' + scopeName : 'the current list selection'
  const geoOverview = useMemo(
    () =>
      selectionNarrowed
        ? homeNodeSelectionGeoOverview(matchedNodes, scopedNetworks, selectionLabel, scope.network)
        : homeGeoOverview(networks, scope.network),
    [matchedNodes, networks, scopedNetworks, scope.network, selectionLabel, selectionNarrowed],
  )
  const geoSelectionBasis = selectionNarrowed ? 'node-selection' : 'network'
  const geoStatus = geoMapStatus(geoOverview, { loading, hasProjection })
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

      {/* Home's overview band */}
      <OverviewBand
        className="p-4"
        dataSlot="home-overview"
        mapSlot="home-map"
        metricsLabel="Home summary"
        mapFirst
        metrics={
          <>
            <SummaryCard
              label="Active Nodes"
              value={activeNodeCount}
              tone="green"
              icon="server"
              emptyLabel={emptyLabel}
            />
            <SummaryCard
              label="Healthy Nodes"
              value={selectionHealthyCount}
              tone="green"
              icon="heart"
              emptyLabel={emptyLabel}
            />
            <SummaryCard
              label="Attention"
              value={attentionCount}
              tone={attentionCount === null || attentionCount === 0 ? 'green' : 'red'}
              icon="alert"
              emptyLabel={emptyLabel}
              caption={ATTENTION_CAPTION}
              hint={ATTENTION_HINT}
            />
            <RewardWalletCard reading={rewardWallet} />
            <ValidatorTotalCard
              networks={scopedNetworks}
              metric="blocks"
              listNarrowed={selectionNarrowed}
              availability={loading ? 'loading' : hasProjection ? 'ready' : 'unavailable'}
            />
            <ValidatorTotalCard
              networks={scopedNetworks}
              metric="rewards"
              listNarrowed={selectionNarrowed}
              availability={loading ? 'loading' : hasProjection ? 'ready' : 'unavailable'}
            />
          </>
        }
        map={
          <GeoMapBoundary>
            <GeoWorldMap overview={geoOverview} status={geoStatus} selectionBasis={geoSelectionBasis} />
          </GeoMapBoundary>
        }
      />

      <div className="relative p-4 pt-0 md:static">
        {/* Modular HomeToolbar */}
        <HomeToolbar
          networks={networks}
          filters={filters}
          scope={scope}
          updateFilters={updateFilters}
          searchDebounceMs={searchDebounceMs}
        />

        <div className="mt-4">
          {/* Node grid or table list */}
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
                    <span className="tabular-nums">{scoped.length.toLocaleString()}</span> Active Nodes in this Network
                    selection.
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

const ATTENTION_CAPTION = 'Unhealthy or Unknown'
const ATTENTION_HINT =
  'Home Attention counts Active Nodes the Server has not confirmed Healthy: Unhealthy or Unknown. A chain-height difference never counts, and Observing is not an abnormal state.'

const SUMMARY_ICONS = {
  server: Server,
  heart: HeartPulse,
  alert: TriangleAlert,
} as const

function SummaryCard({
  label,
  value,
  tone,
  icon,
  emptyLabel,
  caption,
  hint,
}: {
  label: string
  value: number | null
  tone: 'green' | 'red'
  icon: keyof typeof SUMMARY_ICONS
  emptyLabel?: string
  caption?: string
  hint?: string
}) {
  return (
    <SummaryMetricCard
      label={label}
      value={value === null ? emptyLabel ?? 'Unknown' : value.toLocaleString()}
      tone={value !== null && value > 0 ? tone : 'green'}
      icon={SUMMARY_ICONS[icon]}
      caption={caption}
      hint={hint}
    />
  )
}

const REJECTED_FILTER_LABELS: Record<HomeFilterRejection['parameter'], string> = {
  network: 'Network',
  health: 'health',
  validator: 'Validator status',
  sort: 'sort',
  view: 'view',
}

function rejectedFilterNotice(rejections: HomeFilterRejection[]): string {
  const items = rejections.map(({ parameter, value }) => `${REJECTED_FILTER_LABELS[parameter]} "${value}"`)
  const list =
    items.length === 1 ? items[0] : `${items.slice(0, -1).join(', ')} and ${items[items.length - 1]}`
  const outcome = rejections.length === 1 ? 'it fell back to its default' : 'they fell back to their defaults'
  return `This Home link asked for ${list}, which this deployment does not offer, so ${outcome}. The rest of the link was applied unchanged.`
}
