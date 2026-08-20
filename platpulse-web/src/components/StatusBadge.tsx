// Status display with text plus icon (design §2.1, §10.3): color is
// supplementary, never the only channel. WebUI-owned dimensions map onto the
// fixed vocabulary; Server-owned summary words (Node Health severity,
// attention severity) are presented as the Server sends them (webui.md §5.4).

const STATUS_ICONS: Record<string, string> = {
  Starting: '…',
  Current: '✓',
  Stale: '◔',
  Error: '✕',
  Unknown: '?',
  Disabled: '⊘',
  Unsupported: '⚠',
  Empty: '∅',
  'Evaluation unavailable': '?',
  'Live updates paused': '⏸',
  'You are offline': '✕',
  // Server-owned words are presented as the Server sends them (webui.md
  // §5.4): the positive realtime state, the Node Health Summary severity,
  // the attention severity, Operation states (§5.5), and Doctor check
  // states are not WebUI dimensions.
  Connected: '✓',
  Healthy: '✓',
  Unhealthy: '✕',
  Critical: '✕',
  Warning: '⚠',
  healthy: '✓',
  unhealthy: '✕',
  // Operation states (webui.md §5.5); SucceededWithWarnings is never
  // displayed as plain Success.
  Queued: '…',
  Running: '↻',
  Succeeded: '✓',
  'Succeeded with warnings': '⚠',
  Failed: '✕',
  Cancelled: '⊘',
  // Doctor check statuses are Server-owned words shown as sent.
  Pass: '✓',
  Fail: '✕',
  'Not configured': '∅',
  Skipped: '→',
  // Restore validation (issue #51): a short-circuited check was never
  // reached and is never presented as a passing result.
  'Not checked': '∅',
  'Checking…': '…',
}

/** Server component states map onto the WebUI collection vocabulary. */
export function componentStateLabel(state: string | null | undefined): string {
  switch (state) {
    case 'ok':
      return 'Current'
    case 'error':
      return 'Error'
    case 'starting':
      return 'Starting'
    case 'disabled':
      return 'Disabled'
    case 'unsupported':
      return 'Unsupported'
    default:
      return 'Unknown'
  }
}

/**
 * Agent liveness maps onto the WebUI value vocabulary (webui.md §2.1 bans
 * generic `Online` labels): reporting is `Current`, stopped reporting is
 * `Error` with last-good evidence, never observed is `Unknown`.
 */
export function livenessLabel(liveness: string | null | undefined): string {
  switch (liveness) {
    case 'online':
      return 'Current'
    case 'offline':
      return 'Error'
    default:
      return 'Unknown'
  }
}

/** Server freshness dimension maps onto the WebUI freshness vocabulary. */
export function freshnessLabel(freshness: string | null | undefined): string {
  switch (freshness) {
    case 'current':
      return 'Current'
    case 'stale':
      return 'Stale'
    default:
      return 'Unknown'
  }
}

/** Compact UTC rendering of Server timestamps; never derives freshness. */
export function formatObservedAt(timestamp: string | null | undefined): string {
  if (!timestamp) return 'Never observed'
  return `${timestamp.slice(0, 19).replace('T', ' ')} UTC`
}

export function StatusBadge({
  status,
  tone,
}: {
  status: string
  tone?: 'ok' | 'warning' | 'error' | 'neutral'
}) {
  const icon = STATUS_ICONS[status] ?? '·'
  return (
    <span className={`status-badge status-badge-${tone ?? 'neutral'}`}>
      <span className="status-badge-icon" aria-hidden="true">
        {icon}
      </span>
      <span className="status-badge-text">{status}</span>
    </span>
  )
}
