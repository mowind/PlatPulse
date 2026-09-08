import type { NodeIdentityStatus } from './api/generated'

/** Registry-relative identity disposition. Only the Server-owned states are
 * named; anything else is Unknown and is never treated as matched. */
export function identityBadge(identity: NodeIdentityStatus): {
  label: string
  tone: 'ok' | 'warning' | 'error' | 'neutral'
} {
  switch (identity.state) {
    case 'matched':
      return { label: 'Matched', tone: 'ok' }
    case 'mismatched':
      return { label: 'Mismatched', tone: 'error' }
    default:
      return { label: 'Unknown', tone: 'neutral' }
  }
}

export function healthTone(health: string): 'ok' | 'error' | 'neutral' {
  return health === 'healthy' ? 'ok' : health === 'unhealthy' ? 'error' : 'neutral'
}

/** Server freshness dimension -> badge tone (current/stale/unknown). */
export function freshnessTone(freshness: string): 'ok' | 'warning' | 'neutral' {
  return freshness === 'current' ? 'ok' : freshness === 'stale' ? 'warning' : 'neutral'
}

export function visibilityBadge(visibility: string): { label: string; tone: 'ok' | 'neutral' } {
  if (visibility === 'public') return { label: 'Public', tone: 'ok' }
  if (visibility === 'private') return { label: 'Private', tone: 'neutral' }
  return { label: 'Unknown', tone: 'neutral' }
}

/** Lifecycle follows the latest Agent Inventory with a fixed vocabulary:
 * only active and retired are known states — anything else is Unknown
 * rather than a definite lifecycle (preserve-last-good). */
export function lifecycleLabel(lifecycle: string): { label: string; tone: 'ok' | 'neutral' } {
  if (lifecycle === 'active') return { label: 'Active', tone: 'ok' }
  if (lifecycle === 'retired') return { label: 'Retired', tone: 'neutral' }
  return { label: 'Unknown', tone: 'neutral' }
}
