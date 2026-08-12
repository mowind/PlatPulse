import { useEffect, useRef, useState } from 'react'
import { diagnostics, setVisibility, type VisibilityRequest, type VisibilityResponse } from './generated'

import type { AgentDiagnostic } from './generated'

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
