/**
 * Return from an Admin Node detail page to the Nodes list reading it was
 * opened from (webui.md §8.7). The list's own URL already holds that reading —
 * its filters and its sort — so the departure carries only that search, in the
 * tab's react-router navigation state under `ADMIN_NODES_RETURN_KEY`. Nothing
 * is added to the detail URL, so its `?section=` stays the detail's own
 * parameter and a copied or bookmarked detail link carries no list state.
 *
 * The return target is never taken from the state as a location: the path is
 * always the Nodes list, and only the list's own parameters survive, so a
 * hand-written history state cannot send the reader anywhere else.
 */

export const ADMIN_NODES_PATH = '/admin/nodes'
export const ADMIN_NODES_RETURN_KEY = 'adminNodesReturn'

/** The Nodes list's filter parameters; the list reads its filters from these
 *  keys, so a new filter is carried back here without a second list. */
export const ADMIN_NODES_FILTER_PARAMS = ['network', 'health', 'visibility', 'lifecycle'] as const
export const ADMIN_NODES_SORT_PARAM = 'sort'

/** The Nodes list's URL parameters: its filters and its sort. */
const ADMIN_NODES_LIST_PARAMS: readonly string[] = [...ADMIN_NODES_FILTER_PARAMS, ADMIN_NODES_SORT_PARAM]

/** Navigation state written by the Nodes list's links into a Node detail. */
export type AdminNodesReturnState = { [ADMIN_NODES_RETURN_KEY]: { search: string } }

/** Upper bounds that keep a hand-written state from growing the link. */
const MAX_RETURN_SEARCH = 2048
const MAX_PARAM_VALUE = 256

/** Keep only the list's own parameters, in their original order. */
function listSearch(search: string): string {
  if (search.length > MAX_RETURN_SEARCH) return ''
  const source = new URLSearchParams(search)
  const kept = new URLSearchParams()
  for (const [key, value] of source) {
    if (ADMIN_NODES_LIST_PARAMS.includes(key) && value.length <= MAX_PARAM_VALUE && !kept.has(key)) {
      kept.append(key, value)
    }
  }
  const text = kept.toString()
  return text ? '?' + text : ''
}

/** Navigation state for a link from the Nodes list into a Node detail. */
export function adminNodesDeparture(search: string): AdminNodesReturnState {
  return { [ADMIN_NODES_RETURN_KEY]: { search: listSearch(search) } }
}

/** The Nodes list location a detail page was opened from, or null when the
 *  page was opened any other way (a direct link, a new tab, another Admin
 *  page). Unlike the Home return (homeReturn.ts) this deliberately survives a
 *  same-tab reload or a restored session of that entry: it carries only the
 *  list's own non-sensitive parameters, never a position or another reader's
 *  context, so the restored link is still the list the reader came from. */
export function readAdminNodesReturn(state: unknown): string | null {
  if (typeof state !== 'object' || state === null) return null
  const entry = (state as Record<string, unknown>)[ADMIN_NODES_RETURN_KEY]
  if (typeof entry !== 'object' || entry === null) return null
  const search = (entry as Record<string, unknown>).search
  if (typeof search !== 'string') return null
  return ADMIN_NODES_PATH + listSearch(search)
}
