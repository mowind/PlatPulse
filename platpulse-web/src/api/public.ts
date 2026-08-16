import { useEffect, useRef, useState } from 'react'
import {
  publicAccessSettings,
  publicNetwork,
  publicNetworks,
  publicNodeDetail,
  publicNodePeerHistory,
  type PublicAccessSettings,
  type PublicNetwork,
  type PublicNode,
  type PublicPeerHistory,
} from './generated'

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
 * Non-sensitive Public access probe: whether anonymous Home (Guest access)
 * is enabled (design §12.1). Reachable without a Session in both modes so
 * the WebUI can decide whether a Guest may render Home or must sign in.
 */
export async function fetchAccessSettings(signal?: AbortSignal): Promise<PublicAccessSettings> {
  const { data, error } = await publicAccessSettings({ signal })
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
export async function refreshGuestEnabled(signal?: AbortSignal): Promise<boolean> {
  try {
    const settings = await fetchAccessSettings(signal)
    guestEnabledCache = settings.guestEnabled
  } catch (caught) {
    if (caught instanceof DOMException && caught.name === 'AbortError') throw caught
    guestEnabledCache = false
  }
  for (const listener of guestEnabledListeners) listener(guestEnabledCache)
  return guestEnabledCache
}

export async function ensureGuestEnabledKnown(): Promise<boolean> {
  if (guestEnabledCache === null) await refreshGuestEnabled()
  return guestEnabledCache ?? false
}

export async function fetchNodePeerHistory(nodeId: string, signal?: AbortSignal): Promise<PublicPeerHistory> {
  const { data, error } = await publicNodePeerHistory({ path: { node_id: nodeId }, signal })
  if (error || !data) throw new Error(error?.error?.message ?? 'Unable to load Peer history')
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
