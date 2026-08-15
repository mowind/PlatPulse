import { useEffect, useRef, useState } from 'react'
import {
  publicAccessSettings,
  publicNetwork,
  publicNetworks,
  publicNodeDetail,
  type PublicAccessSettings,
  type PublicNetwork,
  type PublicNode,
} from './generated'

export async function fetchNetworks(): Promise<PublicNetwork[]> {
  const { data, error } = await publicNetworks()
  if (error || !data) throw new Error((error as { error?: { message?: string } } | undefined)?.error?.message ?? 'Unable to load published Nodes')
  return data
}

export async function fetchNetwork(networkKey: string): Promise<PublicNetwork> {
  const { data, error } = await publicNetwork({ path: { network_key: networkKey } })
  if (error || !data) throw new Error(error?.error?.message ?? 'Unable to load Network')
  return data
}

export async function fetchNode(nodeId: string): Promise<PublicNode> {
  const { data, error } = await publicNodeDetail({ path: { node_id: nodeId } })
  if (error || !data) throw new Error(error?.error?.message ?? 'Unable to load Node')
  return data
}

/**
 * Non-sensitive Public access probe: whether anonymous Home (Guest access)
 * is enabled (design §12.1). Reachable without a Session in both modes so
 * the WebUI can decide whether a Guest may render Home or must sign in.
 */
export async function fetchAccessSettings(): Promise<PublicAccessSettings> {
  const { data, error } = await publicAccessSettings()
  if (error || !data) {
    throw new Error(
      (error as { error?: { message?: string } } | undefined)?.error?.message ??
        'Unable to load access settings',
    )
  }
  return data
}

/**
 * Cached anonymous-Home setting with a tiny subscriber set. The value is
 * public and non-sensitive; it only decides whether Guests may render Home
 * (the Server still enforces every read). A failed refresh keeps the last
 * value; the initial safe default is `false` (Home is private by default).
 */
let guestEnabledCache: boolean | null = null
const guestEnabledListeners = new Set<(enabled: boolean) => void>()

export function getGuestEnabled(): boolean | null {
  return guestEnabledCache
}

export function subscribeGuestEnabled(listener: (enabled: boolean) => void): () => void {
  guestEnabledListeners.add(listener)
  return () => guestEnabledListeners.delete(listener)
}

/** Re-read the anonymous-Home setting and notify subscribers. Returns the
 * new value, or `false` when the probe fails (private-by-default fallback). */
export async function refreshGuestEnabled(): Promise<boolean> {
  try {
    const settings = await fetchAccessSettings()
    guestEnabledCache = settings.guestEnabled
  } catch {
    guestEnabledCache = false
  }
  for (const listener of guestEnabledListeners) listener(guestEnabledCache)
  return guestEnabledCache
}

export async function ensureGuestEnabledKnown(): Promise<boolean> {
  if (guestEnabledCache === null) await refreshGuestEnabled()
  return guestEnabledCache ?? false
}

export type PublicHistoryItem = { nodeId: string; height: number | null; blockTimeMs: number | null; transactionCount: number | null; source: string | null; coinbase: string | null; sealSignerMatch: string | null; protocolProposer: string | null; observedAt: string | null; freshness: string | null; gapFromHeight: number | null; gapToHeight: number | null; gapKind: string | null; gapReason: string | null; divergenceKind: string | null; divergenceReason: string | null }

export async function fetchNodeHistory(nodeId: string): Promise<PublicHistoryItem[]> {
  const response = await fetch(`/api/public/v1/nodes/${encodeURIComponent(nodeId)}/history`)
  if (!response.ok) throw new Error('Unable to load block history')
  return response.json() as Promise<PublicHistoryItem[]>
}

export async function fetchNodeHistoryExport(nodeId: string): Promise<PublicHistoryItem[]> {
  const response = await fetch(`/api/public/v1/nodes/${encodeURIComponent(nodeId)}/history/export`)
  if (!response.ok) throw new Error('Unable to export block history')
  return response.json() as Promise<PublicHistoryItem[]>
}

export type RealtimeStatus = 'connecting' | 'connected' | 'disconnected'

export function usePublicRealtime(
  onInvalidate: () => void,
  onReset?: () => void,
): RealtimeStatus {
  const [status, setStatus] = useState<RealtimeStatus>('connecting')
  const callback = useRef(onInvalidate)
  callback.current = onInvalidate
  const resetCallback = useRef(onReset)
  resetCallback.current = onReset
  useEffect(() => {
    if (typeof EventSource === 'undefined') return
    const events = new EventSource('/api/public/v1/events')
    const invalidate = () => callback.current()
    const reset = () => resetCallback.current?.()
    events.onopen = () => setStatus('connected')
    events.onerror = () => setStatus('disconnected')
    events.addEventListener('invalidation', invalidate)
    // The Server sends `reset` on authorization transitions: Guest access
    // disabled, Session revoked/expired/role-changed, or a collection-level
    // privacy reset. The surface re-resolves authorization (design §3.3).
    events.addEventListener('reset', reset)
    return () => {
      events.removeEventListener('invalidation', invalidate)
      events.removeEventListener('reset', reset)
      events.close()
    }
  }, [])
  return status
}
