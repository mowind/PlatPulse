// Realtime invalidation (webui.md §6.3), shared by Home and Admin.
//
// SSE carries invalidation and reset signals only; REST stays authoritative.
// A channel owns everything both route groups do the same way: parsing an
// event, dropping one older than the last seen for its resource, coalescing
// refetches into one window, asking an in-flight read again once it settles,
// the REST/SSE cursor handoff, reconnect recovery, and clearing all of it on a
// reset. Each route group supplies an adapter: its query cache and namespace,
// the query keys each resource of its closed vocabulary reaches, and whether a
// reset carried on the `invalidation` stream retires its cache.

import type { Query, QueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import { useRealtimeCursor, type RealtimeSurface } from './transport'

export type RealtimeStatus = 'connecting' | 'connected' | 'disconnected'
export type RealtimeState = { status: RealtimeStatus; online: boolean }

/**
 * Report ingestion can publish a change on every Agent report, and one refetch
 * per event cancels the previous read before it completes. Matching queries are
 * refetched at most once per window, and a refetch never cancels a read that
 * is already in flight.
 */
export const INVALIDATION_COALESCE_MS = 250

/** Query meta of a query that its own interval refreshes and no Invalidation
 *  Event may wake (a longer Metric Window, ADR 0012). */
export const REALTIME_EXEMPT_META = { realtimeInvalidation: 'exempt' } as const

/** A query key prefix an event reaches. The generation is always the last key
 *  part; `exact` reaches only the query whose key is the prefix plus it. */
export type InvalidationTarget = { prefix: readonly unknown[]; exact?: boolean }

export type InvalidationAdapter<R extends string> = {
  surface: RealtimeSurface
  queryClient: QueryClient
  /** The route group's namespace: what a malformed event or a reconnect refetches. */
  root: readonly unknown[]
  /** Exhaustive over the Server's closed resource vocabulary for this surface. */
  targets: { readonly [K in R]: (resourceId: string | undefined) => readonly InvalidationTarget[] }
  /** What an `invalidation` carrying `reset: true` does: retire the cache
   *  through the stream's reset callback, or refetch the namespace. */
  inStreamReset: 'reset' | 'refetch'
}

export type InvalidationChannel = {
  readonly surface: RealtimeSurface
  /** Handles one `invalidation` event under the generation its queries carry. */
  receive(data: string, generation: number): 'handled' | 'reset'
  /** Refetches every active query of the namespace. */
  refetchAll(): Promise<void>
  /** Drops queued refetches and the event ids seen so far. */
  clear(): void
  /** Closes every open stream of this channel. */
  closeStreams(): void
  registerStream(close: () => void): () => void
}

type Pending =
  | { kind: 'target'; target: InvalidationTarget; generation: number }
  | { kind: 'query'; queryKey: readonly unknown[] }

function samePrefix(queryKey: readonly unknown[], prefix: readonly unknown[]): boolean {
  return prefix.every((part, index) => queryKey[index] === part)
}

function exempt(meta: Record<string, unknown> | undefined): boolean {
  return meta?.realtimeInvalidation === REALTIME_EXEMPT_META.realtimeInvalidation
}

function reaches(query: Query, target: InvalidationTarget, generation: number): boolean {
  const { queryKey } = query
  return samePrefix(queryKey, target.prefix)
    && queryKey.at(-1) === generation
    && (target.exact !== true || queryKey.length === target.prefix.length + 1)
    && !exempt(query.meta)
}

function parseEvent(data: string): Record<string, unknown> | null {
  try {
    const value: unknown = JSON.parse(data)
    return typeof value === 'object' && value !== null && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : null
  } catch {
    return null
  }
}

export function createInvalidationChannel<R extends string>(adapter: InvalidationAdapter<R>): InvalidationChannel {
  const { queryClient, root, targets } = adapter
  const lastEventIds = new Map<string, number>()
  const pending = new Map<string, Pending>()
  const streams = new Set<() => void>()
  let timer: ReturnType<typeof setTimeout> | null = null

  const isResource = (value: unknown): value is R => typeof value === 'string' && Object.hasOwn(targets, value)

  function schedule(entry: Pending): void {
    const token = entry.kind === 'target'
      ? `target:${entry.generation}:${entry.target.exact === true}:${JSON.stringify(entry.target.prefix)}`
      : `query:${JSON.stringify(entry.queryKey)}`
    pending.set(token, entry)
    if (timer === null) timer = setTimeout(flush, INVALIDATION_COALESCE_MS)
  }

  function scheduleRoot(generation: number): void {
    schedule({ kind: 'target', target: { prefix: root }, generation })
  }

  function flush(): void {
    timer = null
    const entries = [...pending.values()]
    pending.clear()
    const cache = queryClient.getQueryCache()
    const queries = new Map<string, Query>()
    for (const entry of entries) {
      const found = entry.kind === 'query'
        ? [cache.find({ queryKey: entry.queryKey, exact: true })]
        : cache.findAll({ predicate: (query) => reaches(query, entry.target, entry.generation) })
      for (const query of found) if (query !== undefined) queries.set(query.queryHash, query)
    }
    // A read already in flight began before these events, so it may answer
    // from before the change. Invalidating joins that read instead of starting
    // another, and its success clears the invalidation, so each such query is
    // asked again in the next window, after the read has settled. The snapshot
    // is taken before any refetch starts so only those older reads count.
    const inFlight = [...queries.values()]
      .filter(({ state }) => state.fetchStatus === 'fetching')
      .map(({ queryKey }) => queryKey)
    for (const { queryKey } of queries.values()) {
      void queryClient.invalidateQueries({ queryKey, exact: true, refetchType: 'active' }, { cancelRefetch: false })
    }
    for (const queryKey of inFlight) schedule({ kind: 'query', queryKey })
  }

  function clear(): void {
    if (timer !== null) {
      clearTimeout(timer)
      timer = null
    }
    pending.clear()
    lastEventIds.clear()
  }

  function receive(data: string, generation: number): 'handled' | 'reset' {
    const event = parseEvent(data)
    // A malformed signal is never trusted as data: the whole namespace is
    // refetched instead of letting a stale panel persist.
    if (event === null) {
      scheduleRoot(generation)
      return 'handled'
    }
    if (event.reset === true) {
      if (adapter.inStreamReset === 'reset') return 'reset'
      // A reset retires the event ids too: a restarted Server counts from 1 again.
      lastEventIds.clear()
      scheduleRoot(generation)
      return 'handled'
    }
    const { resource, resourceId } = event
    const idValid = resourceId === undefined || resourceId === null || typeof resourceId === 'string'
    if (!isResource(resource) || !idValid) {
      scheduleRoot(generation)
      return 'handled'
    }
    const id = typeof resourceId === 'string' ? resourceId : undefined
    const eventId = typeof event.eventId === 'number'
      ? event.eventId
      : typeof event.revision === 'number'
        ? event.revision
        : undefined
    if (eventId !== undefined) {
      const key = `${resource}:${id ?? ''}`
      if (eventId <= (lastEventIds.get(key) ?? -1)) return 'handled'
      lastEventIds.set(key, eventId)
    }
    for (const target of targets[resource](id)) schedule({ kind: 'target', target, generation })
    return 'handled'
  }

  return {
    surface: adapter.surface,
    receive,
    async refetchAll() {
      await queryClient.invalidateQueries({
        predicate: (query) => samePrefix(query.queryKey, root) && !exempt(query.meta),
        refetchType: 'active',
      })
    },
    clear,
    closeStreams() {
      for (const close of streams) close()
    },
    registerStream(close) {
      streams.add(close)
      return () => {
        streams.delete(close)
      }
    },
  }
}

export type InvalidationStreamOptions = {
  enabled: boolean
  /** A new key closes the stream and opens another. */
  restartKey: string
  /** The generation the route group's queries currently carry. */
  generation: () => number
  /** Runs after the stream closed on a reset and the channel was cleared. */
  onReset: () => void
}

/**
 * One SSE stream of a channel. `event: reset` (and an in-stream reset where
 * the adapter retires its cache) closes the stream, clears the channel and
 * hands over to `onReset`. When EventSource reconnects after an interruption
 * the stream stays `connecting` until the namespace has been refetched, so
 * Current never claims fresher data than the page holds.
 */
export function useInvalidationStream(channel: InvalidationChannel, options: InvalidationStreamOptions): RealtimeState {
  const { enabled, restartKey } = options
  const [status, setStatus] = useState<RealtimeStatus>('connecting')
  const [online, setOnline] = useState(() => typeof navigator === 'undefined' ? true : navigator.onLine)
  const cursor = useRealtimeCursor(channel.surface)
  const cursorRef = useRef(cursor)
  cursorRef.current = cursor
  const hasCursor = cursor !== null
  const latest = useRef(options)
  latest.current = options

  useEffect(() => {
    const handleOnline = () => setOnline(true)
    const handleOffline = () => setOnline(false)
    window.addEventListener('online', handleOnline)
    window.addEventListener('offline', handleOffline)
    return () => {
      window.removeEventListener('online', handleOnline)
      window.removeEventListener('offline', handleOffline)
    }
  }, [])

  useEffect(() => {
    // The first REST response captures the hub cursor before reading its
    // projection. Waiting for it keeps a fresh shell from replaying stale
    // buffered events; events published after that snapshot remain
    // replayable through `after`.
    if (!enabled || !hasCursor || typeof EventSource === 'undefined') return
    const events = new EventSource(`/api/${channel.surface}/v1/events?after=${cursorRef.current ?? 0}`)
    const unregister = channel.registerStream(() => events.close())
    let opened = false
    let retired = false
    // Bumped whenever an outstanding recovery must no longer mark the stream Current.
    let attempt = 0
    const reset = () => {
      attempt += 1
      events.close()
      setStatus('connecting')
      channel.clear()
      latest.current.onReset()
    }
    const onInvalidation = (event: Event) => {
      const data = (event as MessageEvent<string>).data
      if (channel.receive(data, latest.current.generation()) === 'reset') reset()
    }
    events.onopen = () => {
      if (!opened) {
        opened = true
        setStatus('connected')
        return
      }
      const recovery = ++attempt
      setStatus('connecting')
      void channel.refetchAll().then(() => {
        if (!retired && attempt === recovery) setStatus('connected')
      })
    }
    events.onerror = () => {
      attempt += 1
      setStatus('disconnected')
    }
    events.addEventListener('invalidation', onInvalidation)
    events.addEventListener('reset', reset)
    return () => {
      retired = true
      unregister()
      events.removeEventListener('invalidation', onInvalidation)
      events.removeEventListener('reset', reset)
      events.close()
    }
  }, [channel, enabled, hasCursor, restartKey])

  return { status, online }
}
