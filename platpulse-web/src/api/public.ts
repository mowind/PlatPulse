import { useEffect, useRef, useState } from 'react'
import {
  publicAccessSettings,
  publicNetwork,
  publicNetworks,
  publicNodeDetail,
  publicNodePeerHistory,
  publicValidatorAnalytics,
  publicValidatorHistory,
  type PublicValidatorAnalyticsResponse,
  type PublicValidatorHistoryResponse,
  type PublicNetwork,
  type PublicNode,
  type PublicPeerHistory,
} from './generated'

export type SiteAccessMode = 'public' | 'private'
export type SiteAccessSettings = {
  mode: SiteAccessMode
  authorizationGeneration: number
}

export async function fetchNetworks(signal?: AbortSignal): Promise<PublicNetwork[]> {
  const { data, error } = await publicNetworks({ signal })
  if (error || !data) throw new Error((error as { error?: { message?: string } } | undefined)?.error?.message ?? 'Unable to load published Nodes')
  return data
}

export async function fetchNetwork(networkKey: string, signal?: AbortSignal): Promise<PublicNetwork> {
  const { data, error } = await publicNetwork({ path: { network_key: networkKey }, signal })
  if (error || !data) throw new Error(error?.error?.message ?? 'Unable to load Network')
  return data
}

export async function fetchNode(nodeId: string, signal?: AbortSignal): Promise<PublicNode> {
  const { data, error } = await publicNodeDetail({ path: { node_id: nodeId }, signal })
  if (error || !data) throw new Error(error?.error?.message ?? 'Unable to load Node')
  return data
}

/**
 * Non-sensitive Public access probe for the Server-wide Site Access Mode.
 * Reachable without a Session in both modes so the WebUI can decide whether
 * an anonymous visitor may render Home or must sign in.
 */
export async function fetchAccessSettings(signal?: AbortSignal): Promise<SiteAccessSettings> {
  const { data, error } = await publicAccessSettings({ signal })
  if (error || !data) {
    throw new Error(
      (error as { error?: { message?: string } } | undefined)?.error?.message ??
        'Unable to load access settings',
    )
  }
  return {
    mode: data.mode === 'public' || data.mode === 'private' ? data.mode : 'private',
    authorizationGeneration: data.authorizationGeneration,
  }
}

/**
 * Cached Site Access Mode with a tiny subscriber set. The value is
 * public and non-sensitive; it only decides whether anonymous visitors may
 * render Home (the Server still enforces every read). A failed refresh keeps
 * the last value; the initial safe default is Private.
 */
let siteAccessModeCache: SiteAccessMode | null = null
const siteAccessModeListeners = new Set<(mode: SiteAccessMode) => void>()

export function getSiteAccessMode(): SiteAccessMode | null {
  return siteAccessModeCache
}

export function subscribeSiteAccessMode(listener: (mode: SiteAccessMode) => void): () => void {
  siteAccessModeListeners.add(listener)
  return () => siteAccessModeListeners.delete(listener)
}

/** Re-read the Site Access Mode and notify subscribers. */
export async function refreshSiteAccessMode(signal?: AbortSignal): Promise<SiteAccessMode> {
  try {
    const settings = await fetchAccessSettings(signal)
    siteAccessModeCache = settings.mode
  } catch (caught) {
    if (caught instanceof DOMException && caught.name === 'AbortError') throw caught
    siteAccessModeCache = 'private'
  }
  for (const listener of siteAccessModeListeners) listener(siteAccessModeCache)
  return siteAccessModeCache
}

export async function ensureSiteAccessModeKnown(): Promise<SiteAccessMode> {
  if (siteAccessModeCache === null) await refreshSiteAccessMode()
  return siteAccessModeCache ?? 'private'
}

export async function fetchNodePeerHistory(nodeId: string, signal?: AbortSignal): Promise<PublicPeerHistory> {
  const { data, error } = await publicNodePeerHistory({ path: { node_id: nodeId }, signal })
  if (error || !data) throw new Error(error?.error?.message ?? 'Unable to load Peer history')
  return data
}

export async function fetchValidatorHistory(
  validatorId: string,
  limit = 50,
  signal?: AbortSignal,
): Promise<PublicValidatorHistoryResponse> {
  const { data, error } = await publicValidatorHistory({ path: { validator_id: validatorId }, query: { limit }, signal })
  if (error || !data) throw new Error(error?.error?.message ?? 'Unable to load Validator history')
  return data
}

export async function fetchValidatorAnalytics(
  validatorId: string,
  limit = 31,
  signal?: AbortSignal,
): Promise<PublicValidatorAnalyticsResponse> {
  const { data, error } = await publicValidatorAnalytics({ path: { validator_id: validatorId }, query: { limit }, signal })
  if (error || !data) throw new Error(error?.error?.message ?? 'Unable to load Validator analytics')
  return data
}


 export type PublicHistoryItem = { nodeId: string; height: number | null; blockTimeMs: number | null; transactionCount: number | null; source: string | null; coinbase: string | null; sealSignerMatch: string | null; protocolProposer: string | null; observedAt: string | null; freshness: string | null; gapFromHeight: number | null; gapToHeight: number | null; gapKind: string | null; gapReason: string | null; divergenceKind: string | null; divergenceReason: string | null }

export async function fetchNodeHistory(nodeId: string, signal?: AbortSignal): Promise<PublicHistoryItem[]> {
  const response = await fetch(`/api/public/v1/nodes/${encodeURIComponent(nodeId)}/history`, { signal })
  if (!response.ok) throw new Error('Unable to load block history')
  return response.json() as Promise<PublicHistoryItem[]>
}

export async function fetchNodeHistoryExport(nodeId: string, signal?: AbortSignal): Promise<PublicHistoryItem[]> {
  const response = await fetch(`/api/public/v1/nodes/${encodeURIComponent(nodeId)}/history/export`, { signal })
  if (!response.ok) throw new Error('Unable to export block history')
  return response.json() as Promise<PublicHistoryItem[]>
}

export type RealtimeStatus = 'connecting' | 'connected' | 'disconnected'

export function usePublicRealtime(
  onInvalidate: () => void,
  onReset?: () => void,
): RealtimeStatus {
  const [status, setStatus] = useState<RealtimeStatus>('connecting')
  const [streamKey, setStreamKey] = useState(0)
  const callback = useRef(onInvalidate)
  callback.current = onInvalidate
  const resetCallback = useRef(onReset)
  resetCallback.current = onReset
  useEffect(() => {
    if (typeof EventSource === 'undefined') return
    const events = new EventSource('/api/public/v1/events')
    const restartAfterReset = () => {
      // Do not let the browser keep an old authorization-bound connection
      // alive while the shell rechecks access. A new stream is opened only
      // after this hook has rendered its next generation.
      events.close()
      setStreamKey((value) => value + 1)
      resetCallback.current?.()
    }
    const invalidate = (event: Event) => {
      try {
        const data = JSON.parse((event as MessageEvent<string>).data) as { reset?: unknown }
        if (data.reset === true) {
          restartAfterReset()
          return
        }
      } catch {
        // A malformed invalidation cannot be trusted; clear and re-resolve
        // authorization before allowing the old Public projection to render.
        restartAfterReset()
        return
      }
      callback.current()
    }
    const reset = () => restartAfterReset()
    events.onopen = () => setStatus('connected')
    events.onerror = () => setStatus('disconnected')
    events.addEventListener('invalidation', invalidate)
    // The Server sends `event: reset` on authorization transitions: Guest
    // access disabled, Session revoked/expired/role-changed. Collection
    // privacy resets arrive as an `invalidation` with reset=true and take the
    // same clear/recheck path above.
    events.addEventListener('reset', reset)
    return () => {
      events.removeEventListener('invalidation', invalidate)
      events.removeEventListener('reset', reset)
      events.close()
    }
  }, [streamKey])
  return status
}
