/**
 * Home list search, filters, sort, view, and their ordinary navigation URL
 * (#222, #223, design §9). The Server already answers Home with the complete
 * Public projection of every Active Node, so Home searches, filters, sorts and
 * views that collection locally: no pagination, no new Server query, and no
 * field the Public projection does not carry.
 *
 * Two scopes are deliberately kept apart here. The Network selection scopes the
 * whole Home reading. Search, health, and Validator-status filters narrow the
 * result list, and once they do, the four attributable overview counters and
 * the Peer country map read that list selection too, through the Node Selection
 * Geo Aggregate (#233, design §9); the two cumulative Validator cells keep the
 * Network selection and say so on their face. `selectHomeRecords` returns both
 * Node sets, so the band and the map can never disagree with the list about
 * which Nodes they read. While nothing is narrowed the two sets are equal and
 * every reading is the Network-level one.
 *
 * Every value the URL carries is validated against the vocabulary Home supports
 * and against the projection Home actually has (`readHomeFilters`,
 * `homeNetworkScope`). An unsupported or stale value falls back to the
 * documented default and is reported, never silently obeyed and never silently
 * dropped: the fallback is visible in the UI. A Home URL stays an ordinary
 * navigation URL — it is not a protected sharing link and carries no authority.
 */
import type { PublicNetwork, PublicNode } from './api/generated'
import { nodeDataProgress } from './nodeData'

/** One Public Node together with the Network that projected it. */
export type HomeNodeRecord = {
  network: Pick<PublicNetwork, 'networkKey' | 'displayName'>
  node: PublicNode
}

/**
 * The nine list sorts. Health, Name and Current Head were the three the delivered
 * toolbar already offered, kept in the same place so an existing Home link still
 * means what it meant; Peers, Process CPU and Process memory are the metric
 * columns #223 adds beside them; Disk, Uptime and Attention First are the three
 * #242 adds. Attention First is its own order over the same Server health
 * classification, not a second direction of the Health key (see
 * `ATTENTION_SORT_ORDER`).
 */
export const HOME_SORTS = [
  'health',
  'name',
  'head',
  'peers',
  'process_cpu',
  'process_memory',
  'disk',
  'uptime',
  'attention',
] as const
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
 * parameter Home does not own (another surface's) is left untouched for its
 * owner: the same-tab return of issue #224 is a departure the tab remembers,
 * not a Home parameter this reader would have to carry in the address bar.
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

/** One of the three health words Home reads, never the `all` filter value. */
type HomeHealthWord = Exclude<HomeHealthFilter, 'all'>

/**
 * The two orders Home ranks the Server's health value by, in the three words
 * `healthCategory` reads it in (#223, #242): the delivered Health key's order —
 * an unhealthy Node leads, a healthy Node follows, and a health Home can only
 * read as Unknown sorts after both — and Attention First's, which leads with what
 * the Server has not confirmed Healthy and puts the Unknown health before the
 * confirmed one, the order a reader investigates in. Every sort key shares one
 * further rule — a value the Server never attested sorts last — so an unattested
 * health never outranks an attested one in either order. Nothing else decides a
 * place in these orders: Activity, consensus membership, a resource percentage, a
 * height lag and Resyncing are not health verdicts.
 */
const HEALTH_SORT_ORDER: readonly HomeHealthWord[] = ['unhealthy', 'healthy', 'unknown']
const ATTENTION_SORT_ORDER: readonly HomeHealthWord[] = ['unhealthy', 'unknown', 'healthy']

/** Where one Node's Server-reported health sits in the order a key ranks by. */
function healthRank(value: string, order: readonly HomeHealthWord[]): number {
  const rank = order.indexOf(healthCategory(value))
  return rank === -1 ? order.length : rank
}

/**
 * The Node data ratio behind the Home card's Disk column (#242): the Node data
 * directory's size as a share of the capacity of the filesystem that holds it.
 * The sort reads the two public fields through the same `nodeDataProgress` the
 * card and the Node Detail hero read, so the order and the column can never
 * disagree about which Nodes have a ratio at all, and the Host's own storage
 * percentage — a different observation — is never read here.
 */
function nodeDataRatio(node: PublicNode): number | null {
  return attestedValue(nodeDataProgress(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes))
}

/**
 * A value the Server never attested (`null`/`undefined`), or one it published as
 * something no browser can rank (a non-finite number): both are Unknown to an
 * order and sort last. A measured zero is a value like any other and keeps its
 * place in the ranking.
 */
function attestedValue(value: number | null | undefined): number | null {
  return value !== null && value !== undefined && Number.isFinite(value) ? value : null
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
 * keeps a measured zero as a value: the six metric sorts compare the metric
 * itself (Current Head, Peers, Process CPU, Process memory, the Node data ratio,
 * process uptime), the Health sort compares the three-value health grade, and
 * Attention First compares the same grade in the order a reader investigates in
 * (Unhealthy, Unknown, Healthy) — a Node's health classification decides its
 * place there, and its Activity, its resources and its Resyncing state do not.
 * Equal values fall back to the Node identity, so the order is deterministic:
 * the same set sorts the same way whatever order the projection happened to
 * arrive in, and a refetch cannot make the list jump.
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
      return compareUnknownLast(attestedValue(left.currentHead), attestedValue(right.currentHead))
    case 'peers':
      return compareUnknownLast(attestedValue(left.peers?.peerCount), attestedValue(right.peers?.peerCount))
    case 'process_cpu':
      return compareUnknownLast(attestedValue(left.processCpuPercent), attestedValue(right.processCpuPercent))
    case 'process_memory':
      return compareUnknownLast(attestedValue(left.processMemoryPercent), attestedValue(right.processMemoryPercent))
    case 'disk':
      return compareUnknownLast(nodeDataRatio(left), nodeDataRatio(right))
    case 'uptime':
      return compareUnknownLast(attestedValue(left.processUptimeMs), attestedValue(right.processUptimeMs))
    case 'health':
      return healthRank(left.health, HEALTH_SORT_ORDER) - healthRank(right.health, HEALTH_SORT_ORDER)
    case 'attention':
      return healthRank(left.health, ATTENTION_SORT_ORDER) - healthRank(right.health, ATTENTION_SORT_ORDER)
  }
}

/**
 * The Network scope and the matching list, both read at once. Network selection
 * scopes both; search, health, and Validator-status filters narrow `matching`,
 * which while it is narrower than `scoped` is also what the overview counters
 * and the Peer country map read (#233, design §9). `scoped` stays the scope of
 * the cumulative Validator cells and of the unfiltered readings.
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
