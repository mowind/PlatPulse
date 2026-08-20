import { StatusBadge } from './StatusBadge'

type RealtimeState = {
  status: 'connecting' | 'connected' | 'disconnected'
  online: boolean
}

export function realtimeStreamLabel(status: RealtimeState['status']): 'Current' | 'Starting' | 'Live updates paused' {
  return status === 'connected'
    ? 'Current'
    : status === 'connecting'
      ? 'Starting'
      : 'Live updates paused'
}

/** Shows SSE state and browser connectivity as independent dimensions. */
export function RealtimeNotice({ realtime }: { realtime: RealtimeState }) {
  const streamLabel = realtimeStreamLabel(realtime.status)
  const streamTone = realtime.status === 'connected'
    ? 'ok'
    : realtime.status === 'disconnected'
      ? 'warning'
      : 'neutral'

  return (
    <div className="realtime-notices" aria-live="polite">
      <p className="realtime-notice" data-live={realtime.status === 'connected'}>
        <StatusBadge status={streamLabel} tone={streamTone} />
        <span className="muted"> Server updates arrive as invalidations; REST data stays authoritative.</span>
      </p>
      {!realtime.online && (
        <p className="realtime-notice">
          <StatusBadge status="You are offline" tone="warning" />
        </p>
      )}
    </div>
  )
}
