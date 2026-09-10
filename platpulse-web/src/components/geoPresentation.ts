import type { PublicGeoInsight } from '../api/generated'

/**
 * Geo presentation vocabulary shared by the Network Overview Peer Countries
 * panel and the compact Home Peer country map (issue #133). Both surfaces show
 * the same Server-owned Geo Insight, so their state labels, tones, count
 * formatting, age wording, and Unknown reasons live here once.
 */

export function geoStateLabel(state: string): string {
  switch (state) {
    case 'current':
      return 'Current'
    case 'stale':
      return 'Stale'
    case 'error':
      return 'Error'
    default:
      return 'Unknown'
  }
}

export function geoStateTone(state: string): 'ok' | 'warning' | 'error' | 'neutral' {
  switch (state) {
    case 'current':
      return 'ok'
    case 'stale':
      return 'warning'
    case 'error':
      return 'error'
    default:
      return 'neutral'
  }
}

export function formatGeoCount(value: number): string {
  return value.toLocaleString()
}

export function describeAge(seconds: number): string {
  if (seconds < 60) return `${seconds} second${seconds === 1 ? '' : 's'}`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes} minute${minutes === 1 ? '' : 's'}`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} hour${hours === 1 ? '' : 's'}`
  const days = Math.floor(hours / 24)
  return `${days} day${days === 1 ? '' : 's'}`
}

/** The Server-computed reasons behind an Unknown country count. Both buckets
 * are Server fields; the browser never derives one by subtraction. */
export function geoUnknownReasons(insight: Pick<PublicGeoInsight,
  'unknownWithoutRemoteIpCount' | 'unknownWithPublicIpCount'> | undefined): string[] {
  return [
    [insight?.unknownWithoutRemoteIpCount, 'without a usable public remote IP'],
    [insight?.unknownWithPublicIpCount, 'without a retained country result'],
  ].flatMap(([value, reason]) => (typeof value === 'number' && value > 0 ? [formatGeoCount(value) + ' ' + reason] : []))
}

/** The Peer-record basis of the Geo projection. Records are counted per Node
 * and are never deduplicated by IP, so the wording is fixed here too. */
export function geoPeerRecordBasis(availablePeerCount: number | null | undefined): string {
  if (availablePeerCount == null) return ''
  return `${formatGeoCount(availablePeerCount)} Peer records in scope; counted per Node, not deduplicated by IP.`
}

/** Peer observation freshness as one user-facing phrase. `mixed` names the
 * scope, because the in-scope Networks disagree and no single value is true
 * for all of them. */
export function peerObservationLabel(observation: 'current' | 'stale' | 'unknown' | 'mixed'): string {
  switch (observation) {
    case 'current':
      return 'Current'
    case 'stale':
      return 'Stale'
    case 'mixed':
      return 'Varies by Network'
    default:
      return 'Unknown'
  }
}

/** The country name for an ISO 3166-1 alpha-2 code, so the accessible text is
 * a name and not only a two-letter code. It is a display name for the code the
 * Server already returned; no country attribution is derived here. */
export function countryDisplayName(code: string): string {
  try {
    const name = new Intl.DisplayNames(['en'], { type: 'region' }).of(code)
    return name && name !== code ? name : code
  } catch {
    return code
  }
}

export const PEER_COUNTRIES_HEADING = 'Peer countries'
export const PEER_COUNTRIES_DISABLED_NOTICE = 'Peer countries · Disabled by server'
export const PEER_COUNTRIES_LIST_LABEL = 'Peer countries by count'
