import { StatusBadge } from './StatusBadge'

type RealtimeState = {
  status: 'connecting' | 'connected' | 'disconnected'
  online: boolean
}

export type RealtimeStreamLabel =
  | 'Connecting to live updates'
  | 'Live updates paused'

type AdminRealtimeStreamLabel = 'Current' | 'Starting' | 'Live updates paused'

/**
 * A public transport notice is only rendered when the transport state changes
 * how the visitor should read the page (design §6.3): an open stream renders no
 * notice at all, because it certifies neither observation freshness nor Node
 * Health and needs no visitor action.
 */
export function realtimeStreamLabel(status: RealtimeState['status']): RealtimeStreamLabel | null {
  return status === 'connected'
    ? null
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
    <div className="realtime-notices" aria-live="polite" data-realtime-status={realtime.status}>
      {streamLabel && (
        <p
          className="realtime-notice"
          role="status"
          aria-label={streamLabel}
        >
          <StatusBadge status={streamLabel} tone={streamTone} />
        </p>
      )}
      {!realtime.online && (
        <p className="realtime-notice">
          <StatusBadge status="You are offline" tone="warning" />
        </p>
      )}
    </div>
  )
}
