import { useEffect, useRef, useState } from 'react'
import { diagnostics, setVisibility, type VisibilityRequest, type VisibilityResponse, type AgentDiagnostic } from './generated'

export type AdminHistoryItem = { nodeId: string; height: number | null; blockTimeMs: number | null; transactionCount: number | null; coinbase: string | null; sealSignerMatch: string | null; sealSignerKeyFingerprint: string | null; nodeKeyFingerprint: string | null; nodeKeyValidFrom: string | null; nodeKeyValidUntil: string | null; sealRecoveryRule: string | null; sealEvidence: string | null; protocolProposer: string | null; attributionReason: string | null; observedAt: string | null; freshness: string | null; gapFromHeight: number | null; gapToHeight: number | null }
export async function fetchAdminNodeHistory(nodeId: string): Promise<AdminHistoryItem[]> {
  const response = await fetch(`/api/admin/v1/nodes/${encodeURIComponent(nodeId)}/history`)
  if (!response.ok) throw new Error('Unable to load Admin block history')
  return response.json() as Promise<AdminHistoryItem[]>
}



export async function fetchAdminDiagnostics(): Promise<AgentDiagnostic[]> {
  const { data, error } = await diagnostics()
  if (error || !data) throw new Error((error as { error?: { message?: string } } | undefined)?.error?.message ?? 'Unable to load Admin diagnostics')
  return data
}

export function useAdminRealtime(onInvalidate: () => void): 'connecting' | 'connected' | 'disconnected' {
  const [status, setStatus] = useState<'connecting' | 'connected' | 'disconnected'>('connecting')
  const callback = useRef(onInvalidate)
  callback.current = onInvalidate
  useEffect(() => {
    if (typeof EventSource === 'undefined') return
    const events = new EventSource('/api/admin/v1/events')
    const invalidate = () => callback.current()
    events.onopen = () => setStatus('connected')
    events.onerror = () => setStatus('disconnected')
    events.addEventListener('invalidation', invalidate)
    return () => { events.removeEventListener('invalidation', invalidate); events.close() }
  }, [])
  return status
}

export async function updateNodeVisibility(
  nodeId: string,
  visibility: VisibilityRequest['visibility'],
  csrfToken: string,
): Promise<VisibilityResponse> {
  const { data, error } = await setVisibility({
    path: { node_id: nodeId },
    body: { visibility },
    headers: { 'X-CSRF-Token': csrfToken },
  })
  if (error || !data) throw new Error(error?.error?.message ?? 'Unable to update Node visibility')
  return data
}
