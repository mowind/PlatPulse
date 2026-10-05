/**
 * Home list search, filters, sort, view, and their ordinary navigation URL
 * (#222, #223, design §9). The Server already answers Home with the complete
 * Public projection of every Active Node, so Home searches, filters, sorts and
 * views that collection locally: no pagination, no new Server query, and no
 * field the Public projection does not carry.
 *
 * Two scopes are deliberately kept apart here. The Network selection scopes the
 * whole Home reading — overview statistics, the Peer country map, and the
 * Validator totals. Search, health, and Validator-status filters narrow the
 * result list only (#222, design §9). `selectHomeRecords` returns both, so the
 * overview and the list can never disagree about which Node set they read.
 *
 * Every value the URL carries is validated against the vocabulary Home supports
 * and against the projection Home actually has (`readHomeFilters`,
 * `homeNetworkScope`). An unsupported or stale value falls back to the
 * documented default and is reported, never silently obeyed and never silently
 * dropped: the fallback is visible in the UI. A Home URL stays an ordinary
 * navigation URL — it is not a protected sharing link and carries no authority.
 */
import type { PublicNetwork, PublicNode } from './api/generated'

/** One Public Node together with the Network that projected it. */
export type HomeNodeRecord = {
  network: Pick<PublicNetwork, 'networkKey' | 'displayName'>
  node: PublicNode
}

/**
 * The six list sorts. Health, Name and Current Head were the three the delivered
 * toolbar already offered, kept in the same place so an existing Home link still
 * means what it meant; Peers, Process CPU and Process memory are the metric
 * columns #223 adds beside them.
 */
export const HOME_SORTS = ['health', 'name', 'head', 'peers', 'process_cpu', 'process_memory'] as const
export type HomeSort = (typeof HOME_SORTS)[number]

/** The two Node list views: the delivered card grid, and the compact list (#223). */
export const HOME_VIEWS = ['card', 'list'] as const
export type HomeView = (typeof HOME_VIEWS)[number]

export const HOME_HEALTH_FILTERS = ['all', 'healthy', 'unhealthy', 'unknown'] as const
export type HomeHealthFilter = (typeof HOME_HEALTH_FILTERS)[number]

export const HOME_VALIDATOR_FILTERS = ['all', 'validator', 'not_validator', 'unknown'] as const
export type HomeValidatorFilter = (typeof HOME_VALIDATOR_FILTERS)[number]

export type HomeFilters = {
  /** `all`, or one Network key of the current projection. */
  network: string
  /** The raw search text the reader typed; matching trims it. */
  query: string
  health: HomeHealthFilter
  validator: HomeValidatorFilter
  sort: HomeSort
  /** The list view the reader chose; the card grid is the default. */
  view: HomeView
}

export const DEFAULT_HOME_FILTERS: HomeFilters = {
  network: 'all',
  query: '',
  health: 'all',
  validator: 'all',
  sort: 'health',
  view: 'card',
}

/** The Home filter parameters, in the order Home writes them back. */
const HOME_PARAMETERS = ['network', 'q', 'health', 'validator', 'sort', 'view'] as const

/** A value the URL carried that Home could not honour. A search string is
 *  whatever the reader typed, so it is never rejected. */
export type HomeFilterRejection = {
  parameter: 'network' | 'health' | 'validator' | 'sort' | 'view'
  value: string
}

/** The Network scope Home resolved, plus what it had to refuse. */
export type HomeNetworkScope = {
  network: string
  rejected: HomeFilterRejection | null
}

/** The two Node sets Home reads: the Network scope, and the matching list. */
export type HomeSelection<T extends HomeNodeRecord = HomeNodeRecord> = {
  scoped: T[]
  matching: T[]
}

function oneOf<T extends string>(allowed: readonly T[], value: string | null): T | null {
  return value !== null && (allowed as readonly string[]).includes(value) ? (value as T) : null
}

/**
 * One owned parameter of the Home URL: its supported value, or the default it
 * falls back to while naming the refused value to the reader. An empty value is
 * absent rather than wrong, so it falls back silently.
 */
function readChoice<P extends HomeFilterRejection['parameter'], T extends string>(
  search: URLSearchParams,
  parameter: P,
  allowed: readonly T[],
  fallback: T,
  rejected: HomeFilterRejection[],
): T {
  const raw = search.get(parameter)
  if (!raw) return fallback
  const value = oneOf(allowed, raw)
  if (value === null) {
    rejected.push({ parameter, value: raw })
    return fallback
  }
  return value
}

/**
 * The filters an ordinary Home URL asks for. An unsupported value is reported
 * and replaced by the default; an empty value is absent, not invalid; a
 * parameter Home does not own (another surface's, such as the same-tab return
 * ticket's `to`) is left untouched for its owner.
 */
export function readHomeFilters(search: URLSearchParams): {
  filters: HomeFilters
  rejected: HomeFilterRejection[]
} {
  const rejected: HomeFilterRejection[] = []
  return {
    filters: {
      network: search.get('network') || DEFAULT_HOME_FILTERS.network,
      query: search.get('q') ?? DEFAULT_HOME_FILTERS.query,
      health: readChoice(search, 'health', HOME_HEALTH_FILTERS, DEFAULT_HOME_FILTERS.health, rejected),
      validator: readChoice(search, 'validator', HOME_VALIDATOR_FILTERS, DEFAULT_HOME_FILTERS.validator, rejected),
      sort: readChoice(search, 'sort', HOME_SORTS, DEFAULT_HOME_FILTERS.sort, rejected),
      view: readChoice(search, 'view', HOME_VIEWS, DEFAULT_HOME_FILTERS.view, rejected),
    },
    rejected,
  }
}

/**
 * The ordinary Home URL for these filters: a default selection is omitted
 * rather than spelled out, and parameters another surface owns survive.
 */
export function writeHomeFilters(filters: HomeFilters, base?: URLSearchParams): URLSearchParams {
  const next = new URLSearchParams(base)
  for (const parameter of HOME_PARAMETERS) next.delete(parameter)
  if (filters.network !== DEFAULT_HOME_FILTERS.network && filters.network !== '') {
    next.set('network', filters.network)
  }
  if (filters.query.length > 0) next.set('q', filters.query)
  if (filters.health !== DEFAULT_HOME_FILTERS.health) next.set('health', filters.health)
  if (filters.validator !== DEFAULT_HOME_FILTERS.validator) next.set('validator', filters.validator)
  if (filters.sort !== DEFAULT_HOME_FILTERS.sort) next.set('sort', filters.sort)
  if (filters.view !== DEFAULT_HOME_FILTERS.view) next.set('view', filters.view)
  return next
}

/**
 * The Network a URL selection can still mean. A selection Home cannot resolve is
 * refused here rather than obeyed: filtering by a Network the projection no
 * longer reports would show an empty Home that looks like an empty deployment.
 * Until the projection is available (`networkKeys` is null) nothing is known
 * about the selection, so it is honoured provisionally instead of being
 * declared stale.
 */
export function homeNetworkScope(
  network: string,
  networkKeys: readonly string[] | null,
): HomeNetworkScope {
  if (network === '' || network === 'all') return { network: 'all', rejected: null }
  if (networkKeys === null || networkKeys.includes(network)) return { network, rejected: null }
  return { network: 'all', rejected: { parameter: 'network', value: network } }
}

/**
 * The public values Home searches: the public Node display name, the public Node
 * ID, and the Network display name (design §9). Nothing else is reachable from a
 * Home URL — not Agent notes or names, not Host paths, not endpoints or
 * credentials, and not a linked Validator's identity.
 */
export function homeNodeSearchValues(entry: HomeNodeRecord): string[] {
  return [entry.node.displayName ?? '', entry.node.nodeId, entry.network.displayName]
}

/** An ordinary case-insensitive substring match over those public values. */
export function homeNodeMatchesQuery(entry: HomeNodeRecord, query: string): boolean {
  const needle = query.trim().toLowerCase()
  if (needle.length === 0) return true
  return homeNodeSearchValues(entry).join(' ').toLowerCase().includes(needle)
}

/** The Server's own Node health value, read as one of its three answers. */
export function healthCategory(value: string): Exclude<HomeHealthFilter, 'all'> {
  const normalized = value.trim().toLowerCase()
  if (normalized === 'healthy') return 'healthy'
  if (normalized === 'unhealthy') return 'unhealthy'
  return 'unknown'
}

/**
 * The Server's own Current Validator Status, read verbatim. A Node with no
 * effective Link exposes no status at all, and that absence is Unknown rather
 * than Not a Validator (#173).
 */
export function validatorCategory(node: PublicNode): Exclude<HomeValidatorFilter, 'all'> {
  switch ((node.validator?.currentValidatorStatus ?? '').toLowerCase()) {
    case 'validator':
      return 'validator'
    case 'not_validator':
      return 'not_validator'
    default:
      return 'unknown'
  }
}

export function homeNodeLabel(node: PublicNode): string {
  return node.displayName ?? node.nodeId
}

/**
 * The existing Home health interpretation: one shared tone for the health value
 * the Server publishes, never derived from arbitrary metric zeros.
 */
export function healthTone(value: string): 'good' | 'warn' | 'bad' | 'neutral' {
  const normalized = value.toLowerCase()
  if (/(error|failed|unhealthy|offline|unavailable)/.test(normalized)) return 'bad'
  if (normalized === 'live' || /(healthy|current|connected|ready|synced|active|running|ok|fresh)/.test(normalized)) {
    return 'good'
  }
  if (/(starting|unknown|unsupported|disabled|empty|stale|resync|degraded|connecting)/.test(normalized)) return 'warn'
  return 'neutral'
}

/**
 * The Health sort rank. Every sort key shares one rule — a value the Server never
 * attested sorts last — so Health is graded in the same three words the Health
 * filter and the Health cell already read (#223, stories 73 and 74): an
 * unhealthy Node leads, a healthy Node follows, and a health Home can only read
 * as Unknown sorts after both. Attention still leads, and an unattested health
 * never outranks an attested one.
 */
function healthSortRank(value: string): number {
  const category = healthCategory(value)
  if (category === 'unhealthy') return 0
  return category === 'healthy' ? 1 : 2
}

/**
 * A metric column ordered largest first. A value the Server never attested is
 * Unknown and sorts last; a measured zero is a real value and keeps its place in
 * the order. The two are never conflated (#223, stories 73 and 74).
 */
function compareUnknownLast(left: number | null, right: number | null): number {
  if (left === null) return right === null ? 0 : 1
  if (right === null) return -1
  return right - left
}

/**
 * The requested order. Every key puts a value the Server never attested last and
 * keeps a measured zero as a value: the four metric sorts compare the metric
 * itself (Current Head, Peers, Process CPU, Process memory) and the Health sort
 * compares the three-value health grade. Equal values fall back to the Node
 * identity, so the order is deterministic: the same set sorts the same way
 * whatever order the projection happened to arrive in, and a refetch cannot make
 * the list jump.
 */
export function sortHomeRecords<T extends HomeNodeRecord>(records: readonly T[], sort: HomeSort): T[] {
  return [...records].sort((left, right) => {
    const order = compareHomeNodes(left.node, right.node, sort)
    return order !== 0 ? order : left.node.nodeId.localeCompare(right.node.nodeId)
  })
}

function compareHomeNodes(left: PublicNode, right: PublicNode, sort: HomeSort): number {
  switch (sort) {
    case 'name':
      return homeNodeLabel(left).localeCompare(homeNodeLabel(right))
    case 'head':
      return compareUnknownLast(left.currentHead ?? null, right.currentHead ?? null)
    case 'peers':
      return compareUnknownLast(left.peers?.peerCount ?? null, right.peers?.peerCount ?? null)
    case 'process_cpu':
      return compareUnknownLast(left.processCpuPercent ?? null, right.processCpuPercent ?? null)
    case 'process_memory':
      return compareUnknownLast(left.processMemoryPercent ?? null, right.processMemoryPercent ?? null)
    case 'health':
      return healthSortRank(left.health) - healthSortRank(right.health)
  }
}

/**
 * The Network scope and the matching list, both read at once: Network selection
 * alone scopes the overview and the map, while search, health, and
 * Validator-status filters narrow only the list (design §9, #222).
 */
export function selectHomeRecords<T extends HomeNodeRecord>(
  records: readonly T[],
  filters: HomeFilters,
): HomeSelection<T> {
  const scoped =
    filters.network === 'all'
      ? [...records]
      : records.filter((entry) => entry.network.networkKey === filters.network)
  const matching = scoped
    .filter((entry) => homeNodeMatchesQuery(entry, filters.query))
    .filter((entry) => filters.health === 'all' || healthCategory(entry.node.health) === filters.health)
    .filter((entry) => filters.validator === 'all' || validatorCategory(entry.node) === filters.validator)
  return { scoped, matching: sortHomeRecords(matching, filters.sort) }
}
