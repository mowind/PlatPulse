import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type ChangeEvent,
  type KeyboardEvent,
} from 'react'
import { Search, SlidersHorizontal, X } from 'lucide-react'
import type { PublicNetwork } from '../api/generated'
import { Button } from './ui/button'
import { Input, Select } from './ui/input'
import { Tabs, TabsList, TabsTrigger } from './ui/tabs'
import { ToolbarPopover } from './ui/toolbar-popover'
import { SURFACE_TOOLBAR } from '../lib/surface'
import { cn } from '../lib/utils'
import type {
  HomeFilters,
  HomeHealthFilter,
  HomeNetworkScope,
  HomeSort,
  HomeValidatorFilter,
  HomeView,
} from '../homeFilters'

/** The one accessible name Home's search control answers to, in both shapes
 *  #241 gives it: the row's own field, and the entry that opens that field. */
export const SEARCH_LABEL = 'Search Active Nodes'

/**
 * The room the toolbar's own search shape takes, in CSS pixels (#241): the
 * field's readable 13rem plus its 44px Clear search action and the 4px between
 * them. emerald.css states it once as the 16rem its width query asks for, and
 * the one measurement below reads the same number, so the query and the focus
 * handoff disagree about the reader's toolbar only if this pair drifts.
 */
export const SEARCH_FIELD_TRACK = 16 * 16

/** The one sort control's keys, in `HOME_SORTS` order (#223, #242): the six the
 *  delivered toolbar already offered, then Disk, Uptime and Attention First. No
 *  direction control is added — each key's direction is the key itself, stated
 *  once in `order` so the entry's hint never restates a direction of its own.
 *  `short` is the word the entry prints beside its own name while a key other
 *  than the default is in force, and `label` stays the full name the select and
 *  the description use (#246). */
export const sortOptions: Array<{ value: HomeSort; label: string; short: string; order: string }> = [
  { value: 'health', label: 'Health', short: 'Health', order: 'Unhealthy first, then Healthy, then Unknown' },
  { value: 'name', label: 'Name', short: 'Name', order: 'Node name, A to Z' },
  { value: 'head', label: 'Current Head', short: 'Head', order: 'highest Block Height first, never-observed Nodes last' },
  { value: 'peers', label: 'Peers', short: 'Peers', order: 'most Peers first, never-observed Nodes last' },
  { value: 'process_cpu', label: 'Process CPU', short: 'CPU', order: 'highest process CPU first, never-observed Nodes last' },
  {
    value: 'process_memory',
    label: 'Process memory',
    short: 'Memory',
    order: 'highest process memory first, never-observed Nodes last',
  },
  { value: 'disk', label: 'Disk', short: 'Disk', order: 'largest Node data directory share first, never-observed Nodes last' },
  { value: 'uptime', label: 'Uptime', short: 'Uptime', order: 'longest process uptime first, never-observed Nodes last' },
  {
    value: 'attention',
    label: 'Attention First',
    short: 'Attention',
    order: 'Unhealthy first, then Unknown, then Healthy',
  },
]

export type HomeToolbarProps = {
  networks: PublicNetwork[]
  filters: HomeFilters
  scope: HomeNetworkScope
  updateFilters: (patch: Partial<HomeFilters>, replace?: boolean) => void
  searchDebounceMs?: number
}

export function HomeToolbar({
  networks,
  filters,
  scope,
  updateFilters,
  searchDebounceMs = 150,
}: HomeToolbarProps) {
  // The search box owns its own text while the reader types; the address bar
  // owns it again as soon as it says something the box did not write.
  const [queryDraft, setQueryDraft] = useState<string | null>(null)
  const queryText = queryDraft ?? filters.query
  const queryDraftRef = useRef<string | null>(queryDraft)
  queryDraftRef.current = queryDraft

  const debounceTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const updateFiltersRef = useRef(updateFilters)
  updateFiltersRef.current = updateFilters

  const cancelDebounce = useCallback(() => {
    if (debounceTimerRef.current !== null) {
      clearTimeout(debounceTimerRef.current)
      debounceTimerRef.current = null
    }
  }, [])

  const setDraft = useCallback((value: string | null) => {
    setQueryDraft(value)
    queryDraftRef.current = value
  }, [])

  // Immediately flush any pending draft query when updating other filters (network, sort, health, validator, view)
  const commitWithDraft = useCallback(
    (patch: Partial<HomeFilters>, replace = false) => {
      cancelDebounce()
      if (queryDraftRef.current !== null) {
        const draft = queryDraftRef.current
        setDraft(null)
        updateFiltersRef.current({ query: draft, ...patch }, replace)
      } else {
        updateFiltersRef.current(patch, replace)
      }
    },
    [cancelDebounce, setDraft],
  )

  const handleQueryChange = (value: string) => {
    setDraft(value)
    cancelDebounce()
    if (searchDebounceMs <= 0) {
      updateFiltersRef.current({ query: value }, true)
    } else {
      debounceTimerRef.current = setTimeout(() => {
        debounceTimerRef.current = null
        updateFiltersRef.current({ query: value }, true)
      }, searchDebounceMs)
    }
  }

  // Clear search drops the query and nothing else (#232): skip debounce and clear immediately
  const clearSearch = () => {
    cancelDebounce()
    setDraft(null)
    updateFiltersRef.current({ query: '' }, true)
  }

  // Address bar popstate takeover
  useEffect(() => {
    const onPopState = () => {
      cancelDebounce()
      setDraft(null)
    }
    window.addEventListener('popstate', onPopState)
    return () => {
      window.removeEventListener('popstate', onPopState)
      cancelDebounce()
    }
  }, [cancelDebounce, setDraft])

  // Sync draft when external filters.query changes
  useEffect(() => {
    setQueryDraft((draft) => (filters.query === '' || filters.query === draft ? null : draft))
  }, [filters.query])

  // The two toolbar surfaces (#232): the sort order with the two status
  // filters, and the search field. Only one is open at a time.
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
    if (searchOpen) searchInputRef.current?.focus()
  }, [searchOpen])

  const searchSlotRef = useRef<HTMLDivElement | null>(null)
  const searchFieldRef = useRef<HTMLInputElement | null>(null)
  const searchShapeRef = useRef<boolean | null>(null)
  const searchFocusWasInside = useRef(false)

  // Measure slot width and handle keyboard focus handoff between shapes
  useEffect(() => {
    const slot = searchSlotRef.current
    if (!slot || typeof ResizeObserver === 'undefined') return
    const rememberPlace = (event: FocusEvent) => {
      const target = event.target
      if (!(target instanceof Node)) return
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

  const statusFilterCount = (filters.health === 'all' ? 0 : 1) + (filters.validator === 'all' ? 0 : 1)
  const statusFilterCountId = 'home-status-filter-count'
  const statusFilterSummary = `${statusFilterCount} of 2 status filters on`

  const sortKey = sortOptions.find((option) => option.value === filters.sort) ?? sortOptions[0]
  const sortStateId = 'home-sort-state'
  const sortInForce = filters.sort !== 'health'
  const sortDescription = `Sort is ${sortKey.label}: ${sortKey.order}.`
  const filtersTriggerDescribedBy = sortInForce ? `${statusFilterCountId} ${sortStateId}` : statusFilterCountId

  const searchActive = queryText.trim() !== ''
  const searchStateId = 'home-search-state'

  const clearStatusFilters = () => commitWithDraft({ health: 'all', validator: 'all' })

  const clearSearchProps = {
    variant: 'ghost',
    size: 'icon-sm',
    'aria-label': 'Clear search',
    disabled: queryText === '',
    onClick: clearSearch,
  } as const

  const searchFieldProps = {
    type: 'search',
    'aria-label': SEARCH_LABEL,
    placeholder: 'Search nodes...',
    value: queryText,
    onChange: (event: ChangeEvent<HTMLInputElement>) => {
      handleQueryChange(event.target.value)
    },
    onBlur: () => {
      if (filters.query === queryDraft) {
        setQueryDraft(null)
        queryDraftRef.current = null
      }
    },
    onKeyDown: (event: KeyboardEvent<HTMLInputElement>) => {
      if (event.key === 'Escape') event.preventDefault()
    },
  } as const

  const attentionSortHintId = useId()

  return (
    <div className="flex flex-col gap-2" aria-label="Node filters and sorting">
      <div data-slot="home-toolbar" className="flex flex-wrap items-center justify-end gap-2 md:flex-nowrap">
        {/* Network tabs */}
        <div data-slot="network-group" className="relative z-10 min-w-0 grow overflow-x-auto rounded-sm py-1.5 -my-1.5">
          <Tabs
            value={scope.network}
            onValueChange={(value) => commitWithDraft({ network: value })}
            className="w-full flex-col gap-4"
          >
            <TabsList
              className={cn(
                'compact-tabs min-h-0 group-data-[orientation=horizontal]/tabs:h-8 h-8 w-max rounded-md md:bg-background',
                SURFACE_TOOLBAR,
              )}
              aria-label="Network filter"
            >
              <TabsTrigger
                value="all"
                className="min-h-0 h-6.5 shrink-0 flex-none rounded-sm border-none text-xs shadow-none data-[state=active]:text-emerald-600 dark:data-[state=active]:text-emerald-600"
              >
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

        {/* Toolbar actions */}
        <div
          data-slot="home-toolbar-actions"
          className="grid grid-flow-col grid-cols-[minmax(2.75rem,17rem)_auto_auto] items-center justify-end gap-2"
        >
          {/* Search slot */}
          <div ref={searchSlotRef} data-slot="home-search-slot" className="flex min-w-11 items-center">
            {/* Inline search field */}
            <div data-slot="home-search-inline" className="w-full items-center">
              <div className="relative flex w-full items-center">
                <Search aria-hidden="true" className="pointer-events-none absolute left-2.5 size-4 text-muted-foreground" />
                <Input
                  ref={searchFieldRef}
                  data-slot="home-search-field"
                  data-search-state={searchActive ? 'on' : 'off'}
                  aria-describedby={searchActive ? searchStateId : undefined}
                  className={cn(
                    'w-full pl-8',
                    searchActive ? 'pr-11 border-emerald-600 dark:border-emerald-500' : 'pr-2',
                  )}
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

            {/* Collapsed search popover */}
            <div data-slot="home-search-collapsed" className={cn('relative', searchOpen ? 'z-30' : 'z-10')}>
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
                      searchActive &&
                        'border-emerald-600 text-emerald-700 dark:border-emerald-500 dark:text-emerald-400',
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
                    <Button {...clearSearchProps} className="absolute top-1/2 right-0 -translate-y-1/2">
                      <X aria-hidden="true" />
                    </Button>
                  )}
                </div>
              </ToolbarPopover>
            </div>
          </div>

          {/* Sort & filter popover */}
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
                  <span aria-hidden="true" className="lg:hidden">
                    Sort &amp; filter
                  </span>
                  <span
                    data-slot="home-filter-sort"
                    aria-hidden="true"
                    className="hidden w-[5.5rem] shrink-0 truncate text-left lg:inline-block"
                  >
                    {sortInForce ? (
                      <span className="text-[0.6875rem] font-medium text-muted-foreground">
                        Sort: {sortKey.short}
                      </span>
                    ) : (
                      'Sort & filter'
                    )}
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
              <label className="flex items-center gap-2.5 text-xs font-medium text-muted-foreground">
                <span className="w-[5.5rem] shrink-0 whitespace-nowrap">Sort by</span>
                <Select
                  aria-label="Sort"
                  aria-describedby={filters.sort === 'attention' ? attentionSortHintId : undefined}
                  className="min-h-11 flex-1 text-xs"
                  value={filters.sort}
                  onChange={(event) => commitWithDraft({ sort: event.target.value as HomeSort })}
                >
                  {sortOptions.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </Select>
              </label>
              {filters.sort === 'attention' && (
                <span id={attentionSortHintId} className="sr-only">
                  Attention First leads with the Active Nodes the Server has not confirmed Healthy, Unhealthy before
                  Unknown, and lists the Healthy ones last.
                </span>
              )}
              <span aria-hidden="true" data-slot="home-filter-separator" className="w-full border-t border-border/60" />
              <label className="flex items-center gap-2.5 text-xs font-medium text-muted-foreground">
                <span className="w-[5.5rem] shrink-0 whitespace-nowrap">Health</span>
                <Select
                  aria-label="Health filter"
                  className="min-h-11 flex-1 text-xs"
                  value={filters.health}
                  onChange={(event) => commitWithDraft({ health: event.target.value as HomeHealthFilter })}
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
                  onChange={(event) =>
                    commitWithDraft({ validator: event.target.value as HomeValidatorFilter })
                  }
                >
                  <option value="all">All Validator status</option>
                  <option value="validator">Validator</option>
                  <option value="not_validator">Not a Validator</option>
                  <option value="unknown">Unknown</option>
                </Select>
              </label>
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
            <span id={statusFilterCountId} className="sr-only">
              {statusFilterSummary}
            </span>
            {sortInForce && (
              <span id={sortStateId} data-slot="home-sort-state" className="sr-only">
                {sortDescription}
              </span>
            )}
          </div>

          {/* Cards / List view toggle */}
          <Tabs
            value={filters.view}
            onValueChange={(value) => commitWithDraft({ view: value as HomeView })}
            className="flex-none md:relative md:z-10"
          >
            <TabsList className={cn('flex-none rounded-md md:bg-background', SURFACE_TOOLBAR)} aria-label="View">
              <TabsTrigger value="card" className="flex-none text-xs">
                Cards
              </TabsTrigger>
              <TabsTrigger value="list" className="flex-none text-xs">
                List
              </TabsTrigger>
            </TabsList>
          </Tabs>
        </div>

        {searchActive && <span id={searchStateId} className="sr-only">{`Search is on: ${queryText}`}</span>}
      </div>
    </div>
  )
}
