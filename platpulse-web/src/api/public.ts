import { QueryClient, useQuery } from '@tanstack/react-query'
import { useState } from 'react'
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
  type PublicResource,
  type PublicRewardWallet,
} from './generated'
import { METRIC_WINDOW_REFRESH_MS } from '../metricWindow'
import {
  createInvalidationChannel,
  REALTIME_EXEMPT_META,
  useInvalidationStream,
  type InvalidationTarget,
  type RealtimeState,
} from './realtime'
import { requestGenerated, requestHeaders, setActiveAccessGeneration } from './transport'

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
    meta: window === 'live' ? undefined : REALTIME_EXEMPT_META,
  })
}

export function usePublicNodeHistory(nodeId: string, generation: number) {
  return useQuery({
    queryKey: [...publicKeys.history(nodeId), generation],
    queryFn: ({ signal }) => fetchNodeHistory(nodeId, signal, generation),
    enabled: nodeId.length > 0,
  })
}

export function resetPublicCache(generation: number): void {
  setActiveAccessGeneration(generation)
  // A reset retires the whole namespace, so a queued invalidation for the
  // previous generation must never refetch a query the reset just cleared.
  publicRealtime.clear()
  void publicQueryClient.cancelQueries({ queryKey: publicKeys.all })
  publicQueryClient.clear()
}

function activePublicGeneration(): number {
  // Query keys use the Server's Site Access generation. AuthContext's local
  // session generation is intentionally separate and must never select which
  // Public query generation receives an SSE refetch.
  return getSiteAccessGeneration() ?? DEFAULT_GENERATION
}

const exact = (prefix: readonly unknown[]): InvalidationTarget => ({ prefix, exact: true })

/** Home's realtime channel. A reset on the `invalidation` stream (a purge, a
 *  Site Access Mode change, a Server restart) retires the Public cache. */
export const publicRealtime = createInvalidationChannel<PublicResource>({
  surface: 'public',
  queryClient: publicQueryClient,
  root: publicKeys.all,
  inStreamReset: 'reset',
  targets: {
    node: (nodeId) => nodeId
      ? [exact(publicKeys.node(nodeId)), exact(publicKeys.history(nodeId)), exact(publicKeys.metrics(nodeId))]
      : [{ prefix: publicKeys.nodes }, exact(publicKeys.networks)],
    network: () => [exact(publicKeys.networks)],
    validator: (validatorId) => validatorId
      ? [exact(publicKeys.validators), exact(publicKeys.networks)]
      : [{ prefix: publicKeys.validators }, exact(publicKeys.networks)],
    // A completed background resolution or a provider change updates the
    // country counts inside the Network list.
    geo: () => [{ prefix: publicKeys.networks }],
    reward_wallet: () => [exact(publicKeys.rewardWallet)],
    collection: () => [{ prefix: publicKeys.all }],
  },
})

/** Home's stream. A reset clears the Public cache under the next Site Access
 *  generation, reopens the stream and hands over to `onReset`. */
export function usePublicRealtime(onReset?: () => void, enabled = true, accessGeneration?: number): RealtimeState {
  const [streamKey, setStreamKey] = useState(0)
  return useInvalidationStream(publicRealtime, {
    enabled,
    restartKey: `${accessGeneration ?? ''}:${streamKey}`,
    generation: activePublicGeneration,
    onReset: () => {
      resetPublicCache(activePublicGeneration() + 1)
      setStreamKey((value) => value + 1)
      onReset?.()
    },
  })
}
