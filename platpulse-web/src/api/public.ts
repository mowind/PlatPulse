import { QueryClient, useQuery } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import {
  publicAccessSettings,
  publicNetworks,
  publicNodeDetail,
  publicNodeHistory,
  publicNodeMetrics,
  publicRewardWallet,
  type PublicBlockHistoryItem,
  type PublicNetwork,
  type PublicNodeDetail,
  type PublicMetricWindow,
  type PublicNodeMetricHistory,
  type PublicRewardWallet,
} from './generated'
import { METRIC_WINDOW_REFRESH_MS } from '../metricWindow'
import {
  requestGenerated,
  requestHeaders,
  setActiveAccessGeneration,
  useRealtimeCursor,
} from './transport'

export type SiteAccessMode = 'public' | 'private'
export type SiteAccessSettings = {
  mode: SiteAccessMode
  authorizationGeneration: number
}

export type PublicRequestContext = {
  signal?: AbortSignal
  generation?: number
  revision?: number
}

const DEFAULT_GENERATION = 0

/** Public cache is intentionally a different object from Admin's cache. */
export const publicQueryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 0,
      refetchOnWindowFocus: false,
      staleTime: 30_000,
    },
  },
})

export const publicKeys = {
  all: ['public'] as const,
  networks: ['public', 'networks'] as const,
  nodes: ['public', 'node'] as const,
  node: (nodeId: string) => ['public', 'node', nodeId] as const,
  history: (nodeId: string) => ['public', 'node', nodeId, 'history'] as const,
  metrics: (nodeId: string) => ['public', 'node', nodeId, 'metrics'] as const,
  /** A longer Metric Window (ADR 0012). Live keeps the `metrics` key itself, so
   *  the Invalidation Events that refetch Live are exactly the ones they were. */
  metricsWindow: (nodeId: string, window: Exclude<PublicMetricWindow, 'live'>) => ['public', 'node', nodeId, 'metrics', window] as const,
  validators: ['public', 'validator'] as const,
  rewardWallet: ['public', 'reward-wallet'] as const,
} as const

function contextOf(
  contextOrSignal?: PublicRequestContext | AbortSignal,
  generation?: number,
): PublicRequestContext {
  if (contextOrSignal instanceof AbortSignal) return { signal: contextOrSignal, generation }
  return { ...contextOrSignal, generation: contextOrSignal?.generation ?? generation }
}

function generationOf(context?: PublicRequestContext): number {
  return context?.generation ?? getSiteAccessGeneration() ?? DEFAULT_GENERATION
}

function headersOf(context?: PublicRequestContext): Record<string, string> {
  return requestHeaders(generationOf(context), context?.revision)
}

export async function fetchNetworks(signal?: AbortSignal, generation?: number): Promise<PublicNetwork[]> {
  const context = contextOf(signal, generation)
  return requestGenerated(
    () => publicNetworks({ signal: context.signal, headers: headersOf(context) }),
    'Unable to load Active Nodes',
  )
}

/** The Reward Wallet Balance (#246): one Network-independent reading, so it is
 *  its own request rather than a field of the Network projection. */
export async function fetchRewardWallet(signal?: AbortSignal, generation?: number): Promise<PublicRewardWallet> {
  const context = contextOf(signal, generation)
  return requestGenerated(
    () => publicRewardWallet({ signal: context.signal, headers: headersOf(context) }),
    'Unable to load the Reward Wallet Balance',
  )
}

export async function fetchNode(nodeId: string, signal?: AbortSignal, generation?: number): Promise<PublicNodeDetail> {
  const context = contextOf(signal, generation)
  return requestGenerated(
    () => publicNodeDetail({ path: { node_id: nodeId }, signal: context.signal, headers: headersOf(context) }),
    'Unable to load Node',
  )
}

export async function fetchAccessSettings(signal?: AbortSignal): Promise<SiteAccessSettings> {
  const data = await requestGenerated(
    () => publicAccessSettings({ signal, headers: requestHeaders(DEFAULT_GENERATION) }),
    'Unable to load access settings',
  )
  return {
    mode: data.mode === 'public' || data.mode === 'private' ? data.mode : 'private',
    authorizationGeneration: data.authorizationGeneration,
  }
}

let siteAccessModeCache: SiteAccessMode | null = null
let siteAccessGenerationCache: number | null = null
const siteAccessModeListeners = new Set<(mode: SiteAccessMode) => void>()
const siteAccessGenerationListeners = new Set<(generation: number) => void>()

export function getSiteAccessMode(): SiteAccessMode | null {
  return siteAccessModeCache
}

export function getSiteAccessGeneration(): number | null {
  return siteAccessGenerationCache
}

export function subscribeSiteAccessMode(listener: (mode: SiteAccessMode) => void): () => void {
  siteAccessModeListeners.add(listener)
  return () => siteAccessModeListeners.delete(listener)
}

export function subscribeSiteAccessGeneration(listener: (generation: number) => void): () => void {
  siteAccessGenerationListeners.add(listener)
  return () => siteAccessGenerationListeners.delete(listener)
}

/** Apply an authoritative Site Access response received through another
 * surface, such as the Owner Admin mutation. */
export function applySiteAccessSettings(settings: SiteAccessSettings): void {
  siteAccessModeCache = settings.mode
  siteAccessGenerationCache = settings.authorizationGeneration
  setActiveAccessGeneration(settings.authorizationGeneration)
  for (const listener of siteAccessModeListeners) listener(settings.mode)
  for (const listener of siteAccessGenerationListeners) listener(settings.authorizationGeneration)
}

export async function refreshSiteAccessSettings(signal?: AbortSignal): Promise<SiteAccessSettings> {
  try {
    return await revalidateSiteAccessSettings(signal)
  } catch (caught) {
    if (caught instanceof DOMException && caught.name === 'AbortError') throw caught
    siteAccessModeCache = 'private'
    for (const listener of siteAccessModeListeners) listener(siteAccessModeCache)
    return { mode: siteAccessModeCache, authorizationGeneration: siteAccessGenerationCache ?? DEFAULT_GENERATION }
  }
}

/** Strict access revalidation used by reset transitions. A transport failure
 * must keep the old stream closed rather than becoming synthetic authority. */
export async function revalidateSiteAccessSettings(signal?: AbortSignal): Promise<SiteAccessSettings> {
  const settings = await fetchAccessSettings(signal)
  applySiteAccessSettings(settings)
  return settings
}

export async function refreshSiteAccessMode(signal?: AbortSignal): Promise<SiteAccessMode> {
  return (await refreshSiteAccessSettings(signal)).mode
}

export async function ensureSiteAccessModeKnown(): Promise<SiteAccessMode> {
  if (siteAccessModeCache === null) await refreshSiteAccessSettings()
  return siteAccessModeCache ?? 'private'
}

/** One Metric Window of a Node. Live sends no `window` parameter, so its
 *  request is exactly the one it was before the Metric Window existed. */
export async function fetchNodeMetrics(nodeId: string, signal?: AbortSignal, generation?: number, window: PublicMetricWindow = 'live'): Promise<PublicNodeMetricHistory> {
  const context = contextOf(signal, generation)
  return requestGenerated(
    () => publicNodeMetrics({
      path: { node_id: nodeId },
      ...(window === 'live' ? {} : { query: { window } }),
      signal: context.signal,
      headers: headersOf(context),
    }),
    'Unable to load metric history',
  )
}

export async function fetchNodeHistory(nodeId: string, signal?: AbortSignal, generation?: number): Promise<PublicBlockHistoryItem[]> {
  const context = contextOf(signal, generation)
  return requestGenerated(
    () => publicNodeHistory({ path: { node_id: nodeId }, query: { limit: 2 }, signal: context.signal, headers: headersOf(context) }),
    'Unable to load block history',
  )
}

export function usePublicNetworks(generation: number, enabled = true) {
  return useQuery({ queryKey: [...publicKeys.networks, generation], queryFn: ({ signal }) => fetchNetworks(signal, generation), enabled })
}

/** Independent of the Network query: neither its loading nor its failure, nor
 *  any Home selection, changes what this one reads. */
export function usePublicRewardWallet(generation: number, enabled = true) {
  return useQuery({
    queryKey: [...publicKeys.rewardWallet, generation],
    queryFn: ({ signal }) => fetchRewardWallet(signal, generation),
    enabled,
  })
}

export function usePublicNode(nodeId: string, generation: number) {
  return useQuery({
    queryKey: [...publicKeys.node(nodeId), generation],
    queryFn: ({ signal }) => fetchNode(nodeId, signal, generation),
    enabled: nodeId.length > 0,
  })
}

/** Marks a query refreshed by the clock rather than by Invalidation Events. */
const CLOCK_REFRESHED = 'clock'

/**
 * The metric history of one Metric Window.
 *
 * Live is refetched by Invalidation Events, as it always was. A longer window
 * is refetched by the clock at most once per grain (METRIC_WINDOW_REFRESH_MS)
 * and is never woken by an invalidation, so a busy stream cannot turn a 7-day
 * read into one request per Report.
 *
 * No window keeps a cache once nothing reads it (`gcTime: 0`): a window the
 * reader switches back to is fetched again and shows its loading state, never
 * the answer it held minutes ago.
 */
export function usePublicNodeMetrics(nodeId: string, generation: number, window: PublicMetricWindow = 'live') {
  const queryKey = window === 'live'
    ? [...publicKeys.metrics(nodeId), generation]
    : [...publicKeys.metricsWindow(nodeId, window), generation]
  return useQuery({
    queryKey,
    queryFn: ({ signal }) => fetchNodeMetrics(nodeId, signal, generation, window),
    enabled: nodeId.length > 0,
    gcTime: 0,
    refetchInterval: METRIC_WINDOW_REFRESH_MS[window],
    meta: window === 'live' ? undefined : { refresh: CLOCK_REFRESHED },
  })
}

function clockRefreshed(meta: Record<string, unknown> | undefined): boolean {
  return meta?.refresh === CLOCK_REFRESHED
}

export function usePublicNodeHistory(nodeId: string, generation: number) {
  return useQuery({
    queryKey: [...publicKeys.history(nodeId), generation],
    queryFn: ({ signal }) => fetchNodeHistory(nodeId, signal, generation),
    enabled: nodeId.length > 0,
  })
}

const publicRevisions = new Map<string, number>()

export function resetPublicCache(generation: number): void {
  setActiveAccessGeneration(generation)
  // A reset retires the whole namespace, so a queued invalidation for the
  // previous generation must never refetch a query the reset just cleared.
  if (publicInvalidationTimer !== null) {
    clearTimeout(publicInvalidationTimer)
    publicInvalidationTimer = null
  }
  pendingPublicInvalidations.clear()
  void publicQueryClient.cancelQueries({ queryKey: publicKeys.all })
  publicQueryClient.clear()
  publicRevisions.clear()
}

function acceptEvent(resource: string, resourceId: string | undefined, eventId: number | undefined): boolean {
  if (eventId === undefined) return true
  const key = `${resource}:${resourceId ?? ''}`
  const previous = publicRevisions.get(key) ?? -1
  if (eventId <= previous) return false
  publicRevisions.set(key, eventId)
  return true
}

function activePublicGeneration(): number {
  // Query keys use the Server's Site Access generation. AuthContext's local
  // session generation is intentionally separate and must never select which
  // Public query generation receives an SSE refetch.
  return getSiteAccessGeneration() ?? DEFAULT_GENERATION
}

function samePrefix(queryKey: readonly unknown[], prefix: readonly unknown[]): boolean {
  return prefix.every((part, index) => queryKey[index] === part)
}

/**
 * High-frequency invalidations are coalesced (webui.md §6.3): the Public
 * stream can publish a Network change on every Agent report, and one refetch
 * per event cancels the previous in-flight REST read before it can complete.
 * Pending keys are flushed at most once per window, and the flush never cancels
 * a request that is already in flight, so the projection still converges while
 * the request rate stays bounded.
 */
export const PUBLIC_INVALIDATION_COALESCE_MS = 250

type PendingPublicInvalidation =
  | { kind: 'exact'; queryKey: readonly unknown[]; generation: number }
  | { kind: 'namespace'; namespace: readonly unknown[]; generation: number }

const pendingPublicInvalidations = new Map<string, PendingPublicInvalidation>()
let publicInvalidationTimer: ReturnType<typeof setTimeout> | null = null

function pendingToken(entry: PendingPublicInvalidation): string {
  const parts = entry.kind === 'exact' ? entry.queryKey : entry.namespace
  return entry.kind + ':' + entry.generation + ':' + JSON.stringify(parts)
}

function schedulePublicInvalidation(entry: PendingPublicInvalidation): void {
  pendingPublicInvalidations.set(pendingToken(entry), entry)
  if (publicInvalidationTimer !== null) return
  publicInvalidationTimer = setTimeout(flushPublicInvalidations, PUBLIC_INVALIDATION_COALESCE_MS)
}

function flushPublicInvalidations(): void {
  publicInvalidationTimer = null
  const entries = [...pendingPublicInvalidations.values()]
  pendingPublicInvalidations.clear()
  for (const entry of entries) {
    if (entry.kind === 'exact') {
      void publicQueryClient.invalidateQueries(
        { queryKey: [...entry.queryKey, entry.generation], exact: true, refetchType: 'active' },
        { cancelRefetch: false },
      )
      continue
    }
    void publicQueryClient.invalidateQueries(
      {
        // A clock-refreshed Metric Window answers to its own interval only.
        predicate: ({ queryKey, meta }) =>
          samePrefix(queryKey, entry.namespace) && queryKey.at(-1) === entry.generation && !clockRefreshed(meta),
        refetchType: 'active',
      },
      { cancelRefetch: false },
    )
  }
}

function invalidatePublicNamespace(namespace: readonly unknown[], generation: number): void {
  schedulePublicInvalidation({ kind: 'namespace', namespace, generation })
}

function invalidatePublicExact(queryKey: readonly unknown[], generation: number): void {
  schedulePublicInvalidation({ kind: 'exact', queryKey, generation })
}

export function invalidatePublicResource(resource: string, resourceId?: string, eventId?: number): void {
  if (!acceptEvent(resource, resourceId, eventId)) return
  const generation = activePublicGeneration()
  if (resource === 'geo') {
    // A completed background resolution or a provider change updates the
    // country counts inside the Network list.
    invalidatePublicNamespace(publicKeys.networks, generation)
    return
  }
  const keys: Array<readonly unknown[]> = (() => {
    switch (resource) {
      case 'node':
        return resourceId
          ? [publicKeys.node(resourceId), publicKeys.history(resourceId), publicKeys.metrics(resourceId)]
          : [publicKeys.nodes, publicKeys.networks]
      case 'network':
        return [publicKeys.networks]
      case 'validator':
        return [publicKeys.validators, publicKeys.networks]
      case 'reward_wallet':
        return [publicKeys.rewardWallet]
      case 'collection':
        return [publicKeys.all]
      default:
        return [publicKeys.all]
    }
  })()
  for (const queryKey of keys) {
    if (resourceId !== undefined || queryKey === publicKeys.networks || queryKey === publicKeys.rewardWallet) {
      invalidatePublicExact(queryKey, generation)
    } else {
      invalidatePublicNamespace(queryKey, generation)
    }
  }
}

export type RealtimeStatus = 'connecting' | 'connected' | 'disconnected'
export type RealtimeState = { status: RealtimeStatus; online: boolean }

type PublicInvalidation = { eventId?: unknown; resource?: unknown; resourceId?: unknown; revision?: unknown; reset?: unknown }

export function usePublicRealtime(onReset?: () => void, enabled = true, accessGeneration?: number): RealtimeState {
  const [status, setStatus] = useState<RealtimeStatus>('connecting')
  const [online, setOnline] = useState(() => typeof navigator === 'undefined' ? true : navigator.onLine)
  const [streamKey, setStreamKey] = useState(0)
  const realtimeCursor = useRealtimeCursor('public')
  const realtimeCursorRef = useRef(realtimeCursor)
  realtimeCursorRef.current = realtimeCursor
  const hasRealtimeCursor = realtimeCursor !== null
  const resetCallback = useRef(onReset)
  resetCallback.current = onReset

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
    // projection. Wait for that snapshot before opening the first stream so
    // a fresh shell does not replay stale buffered reset events. Events
    // published after the REST snapshot remain replayable through `after`.
    if (!enabled || !hasRealtimeCursor || typeof EventSource === 'undefined') return
    const events = new EventSource(`/api/public/v1/events?after=${realtimeCursorRef.current ?? 0}`)
    const reset = () => {
      events.close()
      setStatus('connecting')
      resetPublicCache((getSiteAccessGeneration() ?? DEFAULT_GENERATION) + 1)
      setStreamKey((value) => value + 1)
      resetCallback.current?.()
    }
    const invalidate = (event: Event) => {
      try {
        const data = JSON.parse((event as MessageEvent<string>).data) as PublicInvalidation
        if (data.reset === true) {
          reset()
          return
        }
        invalidatePublicResource(
          typeof data.resource === 'string' ? data.resource : 'collection',
          typeof data.resourceId === 'string' ? data.resourceId : undefined,
          typeof data.eventId === 'number'
            ? data.eventId
            : typeof data.revision === 'number'
              ? data.revision
              : undefined,
        )
      } catch {
        reset()
      }
    }
    events.onopen = () => setStatus('connected')
    events.onerror = () => setStatus('disconnected')
    events.addEventListener('invalidation', invalidate)
    events.addEventListener('reset', reset)
    return () => {
      events.removeEventListener('invalidation', invalidate)
      events.removeEventListener('reset', reset)
      events.close()
    }
  }, [accessGeneration, enabled, hasRealtimeCursor, streamKey])

  return { status, online }
}
