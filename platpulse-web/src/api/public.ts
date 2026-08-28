import { QueryClient, useQuery } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import {
  publicAccessSettings,
  publicNetwork,
  publicNetworks,
  publicNodeDetail,
  publicNodeHistory,
  publicNodeMetrics,
  publicNodePeerHistory,
  publicValidatorAnalytics,
  publicValidatorHistory,
  type PublicBlockHistoryItem,
  type PublicValidatorAnalyticsResponse,
  type PublicValidatorHistoryResponse,
  type PublicNetwork,
  type PublicNode,
  type PublicNodeMetricHistory,
  type PublicPeerHistory,
} from './generated'
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
  network: (networkKey: string) => ['public', 'network', networkKey] as const,
  nodes: ['public', 'node'] as const,
  node: (nodeId: string) => ['public', 'node', nodeId] as const,
  history: (nodeId: string) => ['public', 'node', nodeId, 'history'] as const,
  metrics: (nodeId: string) => ['public', 'node', nodeId, 'metrics'] as const,
  peerHistory: (nodeId: string) => ['public', 'node', nodeId, 'peer-history'] as const,
  validators: ['public', 'validator'] as const,
  validatorHistory: (validatorId: string, limit: number) =>
    ['public', 'validator', validatorId, 'history', limit] as const,
  validatorAnalytics: (validatorId: string, limit: number) =>
    ['public', 'validator', validatorId, 'analytics', limit] as const,
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
    'Unable to load published Nodes',
  )
}

export async function fetchNetwork(networkKey: string, signal?: AbortSignal, generation?: number): Promise<PublicNetwork> {
  const context = contextOf(signal, generation)
  const network = await requestGenerated(
    () => publicNetwork({ path: { network_key: networkKey }, signal: context.signal, headers: headersOf(context) }),
    'Unable to load Network',
  )
  return network
}

export async function fetchNode(nodeId: string, signal?: AbortSignal, generation?: number): Promise<PublicNode> {
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

export async function fetchNodePeerHistory(nodeId: string, signal?: AbortSignal, generation?: number): Promise<PublicPeerHistory> {
  const context = contextOf(signal, generation)
  return requestGenerated(
    () => publicNodePeerHistory({ path: { node_id: nodeId }, signal: context.signal, headers: headersOf(context) }),
    'Unable to load Peer history',
  )
}

export async function fetchValidatorHistory(validatorId: string, limit = 50, signal?: AbortSignal, generation?: number): Promise<PublicValidatorHistoryResponse> {
  const context = contextOf(signal, generation)
  return requestGenerated(
    () => publicValidatorHistory({ path: { validator_id: validatorId }, query: { limit }, signal: context.signal, headers: headersOf(context) }),
    'Unable to load Validator history',
  )
}

export async function fetchValidatorAnalytics(validatorId: string, limit = 31, signal?: AbortSignal, generation?: number): Promise<PublicValidatorAnalyticsResponse> {
  const context = contextOf(signal, generation)
  return requestGenerated(
    () => publicValidatorAnalytics({ path: { validator_id: validatorId }, query: { limit }, signal: context.signal, headers: headersOf(context) }),
    'Unable to load Validator analytics',
  )
}

export async function fetchNodeMetrics(nodeId: string, signal?: AbortSignal, generation?: number): Promise<PublicNodeMetricHistory> {
  const context = contextOf(signal, generation)
  return requestGenerated(
    () => publicNodeMetrics({ path: { node_id: nodeId }, signal: context.signal, headers: headersOf(context) }),
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

export function usePublicNetwork(networkKey: string, generation: number) {
  const queryKey = [...publicKeys.network(networkKey), generation] as const
  return useQuery({
    queryKey,
    queryFn: ({ signal }) => fetchNetwork(networkKey, signal, generation),
    enabled: networkKey.length > 0,
  })
}

export function usePublicNode(nodeId: string, generation: number) {
  return useQuery({
    queryKey: [...publicKeys.node(nodeId), generation],
    queryFn: ({ signal }) => fetchNode(nodeId, signal, generation),
    enabled: nodeId.length > 0,
  })
}

export function usePublicNodeMetrics(nodeId: string, generation: number) {
  return useQuery({
    queryKey: [...publicKeys.metrics(nodeId), generation],
    queryFn: ({ signal }) => fetchNodeMetrics(nodeId, signal, generation),
    enabled: nodeId.length > 0,
  })
}

export function usePublicNodeHistory(nodeId: string, generation: number) {
  return useQuery({
    queryKey: [...publicKeys.history(nodeId), generation],
    queryFn: ({ signal }) => fetchNodeHistory(nodeId, signal, generation),
    enabled: nodeId.length > 0,
  })
}

export function usePublicNodePeerHistory(nodeId: string, generation: number) {
  return useQuery({
    queryKey: [...publicKeys.peerHistory(nodeId), generation],
    queryFn: ({ signal }) => fetchNodePeerHistory(nodeId, signal, generation),
    enabled: nodeId.length > 0,
  })
}

export function usePublicValidatorHistory(validatorId: string, limit: number, generation: number) {
  return useQuery({
    queryKey: [...publicKeys.validatorHistory(validatorId, limit), generation],
    queryFn: ({ signal }) => fetchValidatorHistory(validatorId, limit, signal, generation),
    enabled: validatorId.length > 0,
  })
}

export function usePublicValidatorAnalytics(validatorId: string, limit: number, generation: number) {
  return useQuery({
    queryKey: [...publicKeys.validatorAnalytics(validatorId, limit), generation],
    queryFn: ({ signal }) => fetchValidatorAnalytics(validatorId, limit, signal, generation),
    enabled: validatorId.length > 0,
  })
}

const publicRevisions = new Map<string, number>()

export function resetPublicCache(generation: number): void {
  setActiveAccessGeneration(generation)
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

function invalidatePublicNamespace(namespace: readonly unknown[], generation: number): void {
  void publicQueryClient.invalidateQueries({
    predicate: ({ queryKey }) =>
      samePrefix(queryKey, namespace) && queryKey.at(-1) === generation,
    refetchType: 'active',
  })
}

function invalidatePublicExact(queryKey: readonly unknown[], generation: number): void {
  void publicQueryClient.invalidateQueries({
    queryKey: [...queryKey, generation],
    exact: true,
    refetchType: 'active',
  })
}

export function invalidatePublicResource(resource: string, resourceId?: string, eventId?: number): void {
  if (!acceptEvent(resource, resourceId, eventId)) return
  const generation = activePublicGeneration()
  const keys: Array<readonly unknown[]> = (() => {
    switch (resource) {
      case 'node':
        return resourceId
          ? [publicKeys.node(resourceId), publicKeys.history(resourceId), publicKeys.metrics(resourceId), publicKeys.peerHistory(resourceId)]
          : [publicKeys.nodes, publicKeys.networks]
      case 'network':
        return [publicKeys.networks, ...(resourceId ? [publicKeys.network(resourceId)] : [])]
      case 'validator':
        return resourceId
          ? [publicKeys.validatorHistory(resourceId, 20), publicKeys.validatorAnalytics(resourceId, 31)]
          : [publicKeys.validators, publicKeys.networks]
      case 'collection':
        return [publicKeys.all]
      default:
        return [publicKeys.all]
    }
  })()
  for (const queryKey of keys) {
    if (resourceId !== undefined || queryKey === publicKeys.networks || queryKey === publicKeys.network(resourceId ?? '')) {
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
