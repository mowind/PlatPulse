/**
 * The same-tab return from Node detail to Home (#224, design §9).
 *
 * Home publishes two ordinary surfaces: the Home URL, which carries the public
 * filter, sort, and view selection, and the Node detail route. A reader who
 * opens a Node and comes straight back must find the Home they left — the same
 * filters, the same order, and their place in the list — and that is done with
 * two ordinary navigation artifacts and nothing else:
 *
 * 1. Going out, a Home Node link hands Node detail a departure: the Home search
 *    the reader was reading, the Node that link opens, and the place that Node
 *    held in the list on screen. It travels in the same-tab history entry's
 *    navigation state, exactly like the login flow's own `from`, so it is not in
 *    the Node URL, and only the document that wrote it reads it back: a reload, a
 *    new tab, a copied link, and a session the browser restored from disk are all
 *    a different document, and they are handed nothing.
 * 2. Coming back, Node detail rebuilds an ordinary Home URL from that departure
 *    — read into Home's own vocabulary and written back by Home's own writer —
 *    and hands the departure on in navigation state. Home locates the Node it
 *    names by ID first, so a list that was refetched, reordered, or refiltered
 *    while the reader was away still lands on that Node; only when the Node is
 *    gone does the relative place choose the nearest result, and Home then tells
 *    the reader instead of leaving a blank result.
 *
 * Neither direction writes a place to a URL, to storage, or to a cookie. Position
 * is the Node's identity plus its relative place; no pixel scroll offset is
 * recorded anywhere, so a copied link, a new tab, and a fresh or direct Home
 * entry inherit no reading position. The browser's own scroll restoration still
 * serves Back and Forward between two Home entries, which is the browser's
 * business and not this surface's state.
 */
import { homeNodeLabel, readHomeFilters, writeHomeFilters, type HomeNodeRecord } from './homeFilters'

/** The navigation-state key this return uses; the login flow's own
 *  `from`/`sessionExpired` state sits beside it untouched. */
export const HOME_RETURN_STATE_KEY = 'homeReturn'

/** One Node's place in a Home list: its 1-based index and that list's length. */
export type HomeReturnPosition = { index: number; count: number }

/**
 * The Home reading a return restores: the Node the reader opened, the Home
 * search they were reading, and the place that Node held in its list.
 */
export type HomeReturn = {
  nodeId: string
  search: string
  position: HomeReturnPosition | null
  /** The reading token of the document that wrote this departure; see
   *  `homeReturnTab`. */
  tab: string
}

/** The navigation state a Home Node link publishes for Node detail. */
export type HomeReturnState = { homeReturn: HomeReturn }

/**
 * The document that may read a departure back, as a token minted when this
 * document loads and rotated when the reader behind it changes — signing in or
 * out, an expiry or revocation, or a role change — because that is the only
 * event that can put one reader's departure in front of another.
 *
 * History state belongs to the browser, not to this surface: a browser may
 * persist a session to disk and restore it after a restart, and the entry a
 * departure travels in comes back with it. The token is what keeps a departure
 * inside the one document that wrote it, so a restored session — and a reload,
 * which is also a new document — inherits no reading position at all.
 */
let documentToken = createDocumentToken()

/** The reader this document has read a departure for, or null until one is
 *  seen. It lives beside the token because the token is what refuses a
 *  departure, and this is the only reason to rotate it. */
let observedReader: string | null = null

function createDocumentToken(): string {
  const api = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto
  if (api !== undefined && typeof api.randomUUID === 'function') return api.randomUUID()
  // Without Web Crypto the token only has to tell this document apart from the
  // one before it, so a random string is as good as a UUID.
  return Math.random().toString(36).slice(2)
}

/** The reading token of the document this Home is running in. */
export function homeReturnTab(): string {
  return documentToken
}

/** Starts a new reading: the reader behind this document has changed, so no
 *  departure written for the previous one may be honoured after that. A Public
 *  cache reset alone does not start a new reading — the same reader keeps their
 *  own departure through it. */
function rotateHomeReturnTab(): void {
  documentToken = createDocumentToken()
  observedReader = null
}

/**
 * Binds this document's reading token to the reader behind it, at whatever
 * surface outlives the Home shell that reader is leaving from.
 *
 * Home is not the only place a reader can change session: signing out and
 * signing in as somebody else happens at the authentication boundary, and
 * every shell that draws the reading is unmounted in between. Binding the
 * token there is what makes the rotation see that change, and what keeps the
 * rule in one place: the first reader observed adopts the token this document
 * already holds — a reload hands the same reader their own departure back —
 * while any later reader starts a new reading.
 */
export function syncHomeReturnReader(reader: string): void {
  if (observedReader === reader) return
  const adopted = observedReader === null
  if (!adopted) rotateHomeReturnTab()
  observedReader = reader
}

/**
 * The departure for one Node of the list Home is showing, as the navigation
 * state to attach to that Node link: `index` is the row's zero-based place and
 * `count` the list's length, and `tab` the reading token of the document that
 * is doing the leaving. A place Home cannot express — an empty list, an index
 * outside it — is no place at all rather than a wrong one, so a fallback can
 * never be chosen from a number that never named anything.
 */
export function homeReturnState(
  nodeId: string,
  search: URLSearchParams,
  index: number,
  count: number,
  tab: string,
): HomeReturnState {
  return {
    homeReturn: {
      nodeId,
      search: search.toString(),
      position: index >= 0 && index < count ? { index: index + 1, count } : null,
      tab,
    },
  }
}

/**
 * A Node ID a return may carry. The departure is only ever compared with the
 * Node IDs of the current projection, so this bound exists to keep a history
 * entry from spelling arbitrary prose into Home: the ID must be a URL-safe
 * token, and the notice below echoes at most that much of it.
 */
const NODE_ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/

/** The longest Home search a return may restore. */
const MAX_RETURN_SEARCH = 4096

function readPosition(value: unknown): HomeReturnPosition | null {
  if (typeof value !== 'object' || value === null) return null
  const { index, count } = value as { index?: unknown; count?: unknown }
  if (typeof index !== 'number' || typeof count !== 'number') return null
  if (!Number.isInteger(index) || !Number.isInteger(count)) return null
  if (index < 1 || count < 1 || index > count) return null
  return { index, count }
}

/**
 * The return a history entry carries, or null when it carries none this surface
 * can honour. Navigation state is not this surface's own memory — whoever
 * navigated here wrote it — so every field is checked rather than trusted, and a
 * field that fails its check is dropped rather than obeyed. A Node ID that fails
 * drops the whole return, because there is nothing left to locate; a place that
 * fails leaves the Node to be located by ID alone, which is the safe reading.
 *
 * A departure another document wrote — another tab, a session the browser
 * restored, a reload — is not honoured at all: its reading token is not this
 * document's, so there is no reading position to inherit.
 */
export function readHomeReturn(state: unknown, tab: string): HomeReturn | null {
  if (typeof state !== 'object' || state === null) return null
  const candidate = (state as Record<string, unknown>)[HOME_RETURN_STATE_KEY]
  if (typeof candidate !== 'object' || candidate === null) return null
  const { nodeId, search, position, tab: written } = candidate as Record<string, unknown>
  if (typeof nodeId !== 'string' || !NODE_ID_PATTERN.test(nodeId)) return null
  if (typeof search !== 'string' || search.length > MAX_RETURN_SEARCH) return null
  if (written !== tab) return null
  return { nodeId, search, position: readPosition(position), tab }
}

/**
 * The ordinary Home URL a return goes to. The departure's own reading is read
 * into Home's filter vocabulary and written back by Home's own writer, so the
 * return can neither be pointed at a URL the departure spells out itself nor
 * carry a parameter or a value Home would refuse: what the reader left with is
 * what Home validates again on arrival, and a stale or unsupported selection
 * falls back there exactly as it did for the reader who left, notice and all. No
 * place is written to the URL in either direction.
 */
export function homeReturnHref(departure: HomeReturn | null): string {
  if (departure === null) return '/'
  const { filters, rejected } = readHomeFilters(new URLSearchParams(departure.search))
  const query = writeHomeFilters(filters)
  // A refused selection is carried back verbatim rather than dropped, so the
  // reader meets the same notice on arrival that they left behind.
  for (const { parameter, value } of rejected) query.set(parameter, value)
  const written = query.toString()
  return written.length > 0 ? `/?${written}` : '/'
}

/**
 * Why a return could not land on its own Node. `filtered` is a Node the Network
 * selection still holds but the restored filters now hide; `missing` is a Node
 * this Home cannot show at all.
 */
export type HomeReturnFallback = 'filtered' | 'missing'

/** Where a return landed, and what Home owes the reader when it did not land on
 *  the Node itself. */
export type HomeReturnLanding = {
  /** The row to reveal, or null when this Home has no row to reveal. */
  index: number | null
  /** What Home tells the reader, or null when the return landed on its Node. */
  notice: string | null
}

/**
 * The row a stale place means in a list of this length: the place the Node held,
 * clamped to the list Home has now, so a shorter list can never be scrolled past
 * its own end and no blank space is left below it. No place at all is no row
 * rather than the first one.
 */
export function homeReturnFallbackIndex(
  count: number,
  position: HomeReturnPosition | null,
): number | null {
  if (count <= 0 || position === null) return null
  return Math.min(position.index - 1, count - 1)
}

/**
 * The notice a fallback owes the reader: which Node is not here, and — when a
 * row stood in for it — that the nearest one is. With no row to show, the first
 * sentence stands alone, because the empty state below already explains itself.
 */
export function homeReturnNotice(
  reason: HomeReturnFallback,
  label: string,
  hasFallback: boolean,
): string {
  const gone =
    reason === 'filtered'
      ? `${label} is no longer shown by these filters.`
      : `${label} is no longer in this view.`
  if (!hasFallback) return gone
  return `${gone} Showing the nearest ${reason === 'filtered' ? 'matching ' : ''}Active Node instead.`
}

/**
 * Where the return lands. The Node the reader opened is looked up by ID first
 * and wins whatever place the departure recorded, because the list can have been
 * refetched and reordered while the reader was away — that is the risk this
 * ticket names. The relative place is the fallback for a Node that is gone,
 * never the primary key.
 */
export function landHomeReturn<T extends HomeNodeRecord>(
  scoped: readonly T[],
  matching: readonly T[],
  target: HomeReturn,
): HomeReturnLanding {
  const departed = scoped.find((entry) => entry.node.nodeId === target.nodeId)
  if (departed !== undefined) {
    const index = matching.findIndex((entry) => entry.node.nodeId === target.nodeId)
    if (index >= 0) return { index, notice: null }
    const fallback = homeReturnFallbackIndex(matching.length, target.position)
    return {
      index: fallback,
      notice: homeReturnNotice('filtered', homeNodeLabel(departed.node), fallback !== null),
    }
  }
  const fallback = homeReturnFallbackIndex(matching.length, target.position)
  return { index: fallback, notice: homeReturnNotice('missing', target.nodeId, fallback !== null) }
}
