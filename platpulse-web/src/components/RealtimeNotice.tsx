import { StatusBadge } from './StatusBadge'

type RealtimeState = {
  status: 'connecting' | 'connected' | 'disconnected'
  online: boolean
}

export type RealtimeStreamLabel =
  | 'Live updates connected'
  | 'Connecting to live updates'
  | 'Live updates paused'

type AdminRealtimeStreamLabel = 'Current' | 'Starting' | 'Live updates paused'

export function realtimeStreamLabel(status: RealtimeState['status']): RealtimeStreamLabel {
  return status === 'connected'
    ? 'Live updates connected'
    : status === 'connecting'
      ? 'Connecting to live updates'
      : 'Live updates paused'
}

function adminRealtimeStreamLabel(status: RealtimeState['status']): AdminRealtimeStreamLabel {
  return status === 'connected' ? 'Current' : status === 'connecting' ? 'Starting' : 'Live updates paused'
}

/** Shows SSE state and browser connectivity as independent dimensions. */
export function RealtimeNotice({ realtime, surface = 'public' }: { realtime: RealtimeState; surface?: 'public' | 'admin' }) {
  const streamLabel = surface === 'admin'
    ? adminRealtimeStreamLabel(realtime.status)
    : realtimeStreamLabel(realtime.status)
  // Public transport connectivity is deliberately quiet: it does not certify
  // observation freshness, Node Health, or REST success.
  const streamTone = surface === 'admin'
    ? realtime.status === 'connected' ? 'ok' : realtime.status === 'disconnected' ? 'warning' : 'neutral'
    : realtime.status === 'disconnected' ? 'warning' : 'neutral'

  return (
    <div className="realtime-notices" aria-live="polite">
      <p
        className="realtime-notice"
        data-live={realtime.status === 'connected'}
        role="status"
        aria-label={streamLabel}
      >
        <StatusBadge status={streamLabel} tone={streamTone} />
      </p>
      {!realtime.online && (
        <p className="realtime-notice">
          <StatusBadge status="You are offline" tone="warning" />
        </p>
      )}
    </div>
  )
}
