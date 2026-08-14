// Typed Admin adapter and query/realtime layer (design §6.1–§6.3).
//
// The generated OpenAPI client is imported through the project client
// singleton; this module owns request normalization, the dedicated Admin
// query cache (structurally isolated from Public queries), and the
// Admin-specific SSE invalidation stream. REST remains authoritative: SSE
// carries invalidation/reset signals only, and every invalidation refetches
// the matching Admin REST resource through the cache.

import { QueryClient, keepPreviousData, useQuery } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import {
  diagnostics,
  overview,
  setVisibility,
  type AdminOverview,
  type AgentDiagnostic,
  type ApiErrorBody,
  type VisibilityRequest,
  type VisibilityResponse,
} from './generated'

/**
 * Dedicated Admin query cache. Public queries never share this object or
 * its keys, so authorization boundaries cannot cross through runtime cache
 * state (design §6.2, §7 `PATTERN-AUTH-GENERATION`).
 */
export const adminQueryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 0,
      refetchOnWindowFocus: false,
      staleTime: 30_000,
    },
  },
})

const adminKeys = {
  all: ['admin'] as const,
  overview: ['admin', 'overview'] as const,
  diagnostics: ['admin', 'diagnostics'] as const,
}

/**
 * The access generation the Admin cache currently belongs to. Cleared and
 * re-tagged on every authorization transition so a shell mount can tell
 * whether cached values may come from an older session.
 */
let adminCacheGeneration: number | null = null

/** A failed Admin call, keyed by the Server's stable error `code`. */
export class AdminApiError extends Error {
  readonly code: string

  constructor(code: string, message: string) {
    super(message)
    this.name = 'AdminApiError'
    this.code = code
  }
}

/**
 * Access-generation reset bus. The Admin surface subscribes once per shell;
 * REST 401s (`auth_required`, e.g. an expired or revoked session) notify the
 * bus so the shell can close streams, clear the Admin cache, and re-check
 * the session before any older-generation response can render.
 */
type AccessResetListener = () => void
const accessResetListeners = new Set<AccessResetListener>()

export function subscribeAdminAccessReset(listener: AccessResetListener): () => void {
  accessResetListeners.add(listener)
  return () => accessResetListeners.delete(listener)
}

function notifyAdminAccessReset(): void {
  for (const listener of accessResetListeners) listener()
}

async function requestAdmin<T>(
  run: () => Promise<{ data?: T; error?: unknown }>,
  fallbackMessage: string,
): Promise<T> {
  const { data, error } = await run()
  if (error || !data) {
    const body = error as ApiErrorBody | undefined
    if (body?.error?.code === 'auth_required') notifyAdminAccessReset()
    throw new AdminApiError(
      body?.error?.code ?? 'unavailable',
      body?.error?.message ?? fallbackMessage,
    )
  }
  return data
}

/** Server-owned attention overview (REST-authoritative). */
export async function fetchAdminOverview(signal?: AbortSignal): Promise<AdminOverview> {
  return requestAdmin(
    () => overview({ signal }),
    'Unable to load the Admin overview',
  )
}

/** Full Agent/Node diagnostics projected by the Server. */
export async function fetchAdminDiagnostics(signal?: AbortSignal): Promise<AgentDiagnostic[]> {
  return requestAdmin(
    () => diagnostics({ signal }),
    'Unable to load Admin diagnostics',
  )
}

export function useAdminOverview(generation: number) {
  return useQuery({
    // The generation is part of the key: data cached under an older access
    // generation is never rendered under the current one.
    queryKey: [...adminKeys.overview, generation],
    queryFn: ({ signal }) => fetchAdminOverview(signal),
    // Keep the last authoritative value visible while an SSE-triggered
    // refetch runs; a failed refetch never blanks the panel.
    placeholderData: keepPreviousData,
  })
}

export function useAdminDiagnostics(generation: number) {
  return useQuery({
    queryKey: [...adminKeys.diagnostics, generation],
    queryFn: ({ signal }) => fetchAdminDiagnostics(signal),
    placeholderData: keepPreviousData,
  })
}

/**
 * Mutation seam: no optimistic state, no automatic retry. Success invalidates
 * the Admin namespace immediately so the panels refetch authoritative REST.
 */
export async function updateNodeVisibility(
  nodeId: string,
  visibility: VisibilityRequest['visibility'],
  csrfToken: string,
): Promise<VisibilityResponse> {
  const response = await requestAdmin(
    () =>
      setVisibility({
        path: { node_id: nodeId },
        body: { visibility },
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to update Node visibility',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

/** Abort in-flight Admin requests and drop every cached Admin value that
 * belongs to an older access generation. No-op when the cache is already
 * valid for the current generation. */
export function resetAdminCache(generation: number): void {
  if (adminCacheGeneration === generation) return
  adminCacheGeneration = generation
  void adminQueryClient.cancelQueries({ queryKey: adminKeys.all })
  adminQueryClient.clear()
}

export type RealtimeStatus = 'connecting' | 'connected' | 'disconnected'
export type RealtimeState = { status: RealtimeStatus; online: boolean }

/**
 * One Admin SSE stream per shell per tab. Invalidations refetch the exact
 * Admin REST namespace (coalesced by the query cache); the Server's
 * access `reset` event (session expired/revoked/role-changed) or an
 * `auth_required` REST response triggers the access-generation sequence
 * instead. Reconnects re-open the stream under the current generation.
 */
export function useAdminRealtime(
  generation: number,
  onAccessReset: () => void,
): RealtimeState {
  const [status, setStatus] = useState<RealtimeStatus>('connecting')
  const [online, setOnline] = useState(() =>
    typeof navigator === 'undefined' ? true : navigator.onLine,
  )
  const accessReset = useRef(onAccessReset)
  accessReset.current = onAccessReset

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
    if (typeof EventSource === 'undefined') return
    const events = new EventSource('/api/admin/v1/events')
    // The Server sends `event: reset` only when the session is no longer
    // current (expired, revoked, role-changed); that is an access-generation
    // transition. Every `invalidation` (including buffered replay resets)
    // only refetches the authoritative Admin REST namespace.
    const onReset = () => accessReset.current()
    const onInvalidation = () => {
      void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    }
    events.onopen = () => setStatus('connected')
    events.onerror = () => setStatus('disconnected')
    events.addEventListener('invalidation', onInvalidation)
    events.addEventListener('reset', onReset)
    return () => {
      events.removeEventListener('invalidation', onInvalidation)
      events.removeEventListener('reset', onReset)
      events.close()
    }
  }, [generation])

  return { status, online }
}
