/** Which one of the Audit page's loading and failure states applies. Each
 *  query is one filter selection at one cursor; `settled` says whether a
 *  page for the selection has been merged into the list. The three failures
 *  are kept apart so each is reported once, in its own place:
 *  - the first page never loaded (`initial-*`): the page-wide state;
 *  - an older page never loaded (`older-*`): below the kept list;
 *  - the shown query loaded before but its latest refresh failed
 *    (`refresh-*`): above the kept list, which stays as it was.
 *  A query the reader asked to retry keeps its failure row (and the button
 *  holding focus) while it runs; an automatic load of a selection that failed
 *  earlier is an ordinary load. Nothing
 *  here reads the Realtime stream: only this query's own result counts. */
export type AuditQueryState =
  | 'initial-loading'
  | 'initial-failed'
  | 'initial-retrying'
  | 'older-loading'
  | 'older-failed'
  | 'refresh-failed'
  | 'refresh-retrying'
  | 'ready'

export function auditQueryState(query: {
  settled: boolean
  appending: boolean
  hasData: boolean
  isError: boolean
  isFetching: boolean
  /** The reader pressed a retry for this very query since it last
   *  succeeded. A refetch of a query without data clears its error while it
   *  runs, so only this tells a retry apart from an ordinary load. */
  retrying: boolean
}): AuditQueryState {
  if (query.hasData) {
    // A shown query whose refresh failed keeps its error until a refresh
    // succeeds; before that page is merged there is nothing to keep yet.
    if (query.settled && query.isError) return query.isFetching ? 'refresh-retrying' : 'refresh-failed'
    return query.settled ? 'ready' : 'initial-loading'
  }
  if (query.settled && query.appending) {
    if (query.isError && !query.isFetching) return 'older-failed'
    return query.isFetching ? 'older-loading' : 'ready'
  }
  if (query.settled) {
    // The same selection's newest page under a new key (an authorization
    // generation change keeps the filters): the listed events are the ones
    // loaded before, so its failure is a failed refresh, never silence.
    if (query.isError && !query.isFetching) return 'refresh-failed'
    if (query.isFetching && (query.retrying || query.isError)) return 'refresh-retrying'
    return 'ready'
  }
  if (query.isError && !query.isFetching) return 'initial-failed'
  if ((query.retrying || query.isError) && query.isFetching) return 'initial-retrying'
  return 'initial-loading'
}
