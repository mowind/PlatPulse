import { useEffect, useRef, useState } from 'react'
import { publicNetwork, publicNetworks, publicNodeDetail, type PublicNetwork, type PublicNode } from './generated'

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

export type PublicHistoryItem = { nodeId: string; height: number | null; blockTimeMs: number | null; transactionCount: number | null; observedAt: string | null; freshness: string | null; gapFromHeight: number | null; gapToHeight: number | null }

export async function fetchNodeHistory(nodeId: string): Promise<PublicHistoryItem[]> {
  const response = await fetch(`/api/public/v1/nodes/${encodeURIComponent(nodeId)}/history`)
  if (!response.ok) throw new Error('Unable to load block history')
  return response.json() as Promise<PublicHistoryItem[]>
}

export type RealtimeStatus = 'connecting' | 'connected' | 'disconnected'

export function usePublicRealtime(onInvalidate: () => void): RealtimeStatus {
  const [status, setStatus] = useState<RealtimeStatus>('connecting')
  const callback = useRef(onInvalidate)
  callback.current = onInvalidate
  useEffect(() => {
    if (typeof EventSource === 'undefined') return
    const events = new EventSource('/api/public/v1/events')
    const invalidate = () => callback.current()
    events.onopen = () => setStatus('connected')
    events.onerror = () => setStatus('disconnected')
    events.addEventListener('invalidation', invalidate)
    return () => { events.removeEventListener('invalidation', invalidate); events.close() }
  }, [])
  return status
}
