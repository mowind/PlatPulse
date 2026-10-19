import { hashKey } from '@tanstack/react-query'
import { useSyncExternalStore } from 'react'
import {
  adminNetworkDetailQueryKey,
  adminQueryClient,
  fetchAdminNetwork,
  onAdminCacheReset,
  updateNetworkEntry,
} from './admin'
import type { AdminNetworkDetail, NetworkResponse } from './generated'
import { TransportError } from './transport'

/**
 * Network tuple updates that outlive the editor that sent them.
 *
 * The update sends the complete identity tuple, so a second update composed
 * from a tuple the page read before the first one landed would put the first
 * update's identity fields back. The editor's own state ends when it
 * unmounts (another Network, the Registry list, and back), so the hold lives
 * here instead: one entry per access generation and Network key, created
 * synchronously before the request is sent, and removed only by evidence that
 * the page can again read what the Server holds.
 *
 * - pending: sent and unanswered. Nothing but the answer ends it.
 * - reconciling: applied; this write's own read-back is in flight.
 * - readBackFailed: applied, but no read of this Network since has succeeded.
 * - uncertain: the answer was lost (no response, or none from the Server), so
 *   the update may or may not have been applied. It is never sent again; it
 *   ends only when a read shows the Server holding exactly that tuple.
 *
 * A read counts only when it began after this write was answered (or lost):
 * every read of the Network still in flight at that moment is cancelled
 * first, so a read that began before the write can never stand for the
 * read-back. A read showing the earlier tuple does not end an uncertain write:
 * the request may still reach the Server, and nothing in the current API can
 * prove it never will. A Server refusal (its typed error envelope; the update
 * handler sends one only when nothing was committed) ends the write at once.
 *
 * The state is memory only, bound to this tab and access generation: a
 * generation change (sign-in, sign-out, or any access re-check) clears it
 * with the Admin cache, and an answer arriving afterwards changes nothing.
 * An update still unanswered at that moment is no longer held in the new
 * generation; that is the existing authorization boundary, not a proof. It holds no request bodies beyond the tuple
 * being compared, and no credential.
 */

export type NetworkTupleUpdate = {
  displayName: string
  genesisHash: string
  chainId: number
  p2pNetworkId: number
  addressHrp: string
}

export type NetworkTupleWrite =
  | { phase: 'pending' }
  | { phase: 'reconciling' }
  | { phase: 'readBackFailed'; checking: boolean }
  | {
      phase: 'uncertain'
      checking: boolean
      /** What the latest read after the lost answer found. */
      lastCheck: 'none' | 'failed' | 'notShown'
    }

export type NetworkTupleWriteOutcome =
  | { kind: 'applied'; response: NetworkResponse }
  | { kind: 'rejected'; error: unknown }
  | { kind: 'uncertain' }
  /** The authorization ended (or the write was otherwise retired) first. */
  | { kind: 'superseded' }

/** One update. The entry object itself is the operation's identity: a late
 *  answer acts only while its own entry is still the one held. */
type Entry = {
  readonly key: string
  readonly generation: number
  readonly networkKey: string
  readonly queryHash: string
  readonly update: NetworkTupleUpdate
  state: NetworkTupleWrite
}

const entries = new Map<string, Entry>()
const listeners = new Set<() => void>()

function entryKey(generation: number, networkKey: string): string {
  return JSON.stringify([generation, networkKey])
}

function emit(): void {
  for (const listener of listeners) listener()
}

function isCurrent(entry: Entry): boolean {
  return entries.get(entry.key) === entry
}

function transition(entry: Entry, state: NetworkTupleWrite): void {
  if (!isCurrent(entry)) return
  entry.state = state
  emit()
}

function endHold(entry: Entry): void {
  if (!isCurrent(entry)) return
  entries.delete(entry.key)
  emit()
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** The write holding this Network's tuple, read synchronously (the guard a
 *  submit handler checks before anything is sent). */
export function networkTupleWrite(generation: number, networkKey: string): NetworkTupleWrite | null {
  return entries.get(entryKey(generation, networkKey))?.state ?? null
}

export function useNetworkTupleWrite(generation: number, networkKey: string): NetworkTupleWrite | null {
  return useSyncExternalStore(subscribe, () => networkTupleWrite(generation, networkKey))
}

/** Whether the Server refused the update. Its handler answers with the typed
 *  error envelope only before its transaction commits, or when the commit
 *  itself fails, so such an answer proves the update was not applied. Any
 *  other failure — no response, an unreadable one, or an intermediary's page
 *  — proves nothing either way. */
function updateRefused(error: unknown): boolean {
  return error instanceof TransportError && error.serverEnvelope
}

function holdsUpdate(detail: AdminNetworkDetail, update: NetworkTupleUpdate): boolean {
  return (
    detail.display_name === update.displayName &&
    detail.genesis_hash === update.genesisHash &&
    detail.chain_id === update.chainId &&
    detail.p2p_network_id === update.p2pNetworkId &&
    detail.address_hrp === update.addressHrp
  )
}

/** A successful read of this Network, begun after the write's barrier. */
function readSucceeded(entry: Entry, detail: AdminNetworkDetail | undefined): void {
  const state = entry.state
  if (state.phase === 'readBackFailed') {
    endHold(entry)
  } else if (state.phase === 'uncertain') {
    if (detail && holdsUpdate(detail, entry.update)) endHold(entry)
    else transition(entry, { ...state, lastCheck: 'notShown' })
  }
}

// Reads of a held Network made elsewhere on the page (a refresh, Try again,
// a realtime refetch) count as well: every one of them began after the
// barrier, because the barrier cancelled each read in flight at that point.
adminQueryClient.getQueryCache().subscribe((event) => {
  if (event.type !== 'updated' || entries.size === 0) return
  const action = event.action
  if (action.type !== 'success' || action.manual) return
  for (const entry of entries.values()) {
    if (entry.queryHash !== event.query.queryHash) continue
    if (entry.state.phase === 'readBackFailed' || entry.state.phase === 'uncertain') {
      readSucceeded(entry, event.query.state.data as AdminNetworkDetail | undefined)
    }
  }
})

onAdminCacheReset(() => {
  if (entries.size === 0) return
  entries.clear()
  emit()
})

/** Cancels every read of this Network in flight: none of them may stand for
 *  a read made after the write was answered. */
function cancelEarlierReads(entry: Entry): void {
  void adminQueryClient.cancelQueries({
    queryKey: adminNetworkDetailQueryKey(entry.generation, entry.networkKey),
    exact: true,
  })
}

/** Reads this Network's detail into the page's query (joining a read that
 *  began after the barrier). Resolves to whether the read succeeded. */
async function readNetwork(entry: Entry): Promise<boolean> {
  if (!isCurrent(entry)) return false
  try {
    await adminQueryClient.fetchQuery({
      queryKey: adminNetworkDetailQueryKey(entry.generation, entry.networkKey),
      queryFn: ({ signal }) => fetchAdminNetwork(entry.networkKey, signal),
      staleTime: 0,
    })
    return true
  } catch {
    return false
  }
}

/** Sends one tuple update, unless this Network already holds a write. The
 *  hold is in place before the request leaves; it returns null (and sends
 *  nothing) while another write holds the Network. */
export function startNetworkTupleWrite(options: {
  generation: number
  networkKey: string
  update: NetworkTupleUpdate
  csrfToken: string
}): Promise<NetworkTupleWriteOutcome> | null {
  const { generation, networkKey, update, csrfToken } = options
  const key = entryKey(generation, networkKey)
  if (entries.has(key)) return null
  const entry: Entry = {
    key,
    generation,
    networkKey,
    queryHash: hashKey(adminNetworkDetailQueryKey(generation, networkKey)),
    update,
    state: { phase: 'pending' },
  }
  entries.set(key, entry)
  emit()
  return sendUpdate(entry, csrfToken)
}

async function sendUpdate(entry: Entry, csrfToken: string): Promise<NetworkTupleWriteOutcome> {
  let response: NetworkResponse
  try {
    response = await updateNetworkEntry(entry.networkKey, entry.update, csrfToken)
  } catch (error) {
    if (!isCurrent(entry)) return { kind: 'superseded' }
    if (updateRefused(error)) {
      endHold(entry)
      return { kind: 'rejected', error }
    }
    cancelEarlierReads(entry)
    transition(entry, { phase: 'uncertain', checking: false, lastCheck: 'none' })
    // One read-only look straight away; the update itself is never re-sent.
    void checkEntry(entry)
    return { kind: 'uncertain' }
  }
  if (!isCurrent(entry)) return { kind: 'superseded' }
  cancelEarlierReads(entry)
  transition(entry, { phase: 'reconciling' })
  const readBack = await readNetwork(entry)
  // Only a reset retires a reconciling write (page reads do not settle it).
  if (!isCurrent(entry)) return { kind: 'superseded' }
  if (readBack) endHold(entry)
  else transition(entry, { phase: 'readBackFailed', checking: false })
  return { kind: 'applied', response }
}

async function checkEntry(entry: Entry): Promise<void> {
  const state = entry.state
  if (state.phase !== 'readBackFailed' && state.phase !== 'uncertain') return
  if (state.checking) return
  transition(entry, { ...state, checking: true })
  const ok = await readNetwork(entry)
  // A successful read has already been applied through the query cache.
  const after = entry.state
  if (after.phase === 'uncertain') {
    transition(entry, { ...after, checking: false, lastCheck: ok ? after.lastCheck : 'failed' })
  } else if (after.phase === 'readBackFailed') {
    transition(entry, { ...after, checking: false })
  }
}

/** Reads the Network again for a write whose read-back failed or whose answer
 *  was lost. It only reads. */
export function checkNetworkTupleWrite(generation: number, networkKey: string): Promise<void> {
  const entry = entries.get(entryKey(generation, networkKey))
  return entry ? checkEntry(entry) : Promise.resolve()
}

/** Test seam: forget every write between in-process UI tests. */
export function resetNetworkTupleWrites(): void {
  entries.clear()
  emit()
}
