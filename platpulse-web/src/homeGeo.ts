import type { PublicCountryCount, PublicNetwork, PublicNodeDetail } from './api/generated'
import type { RepresentativePoint } from './worldGeometry'

/**
 * Home Geo overview (issue #133). It is a pure projection of the Server's
 * Public Geo Insight: the browser never derives a country, a count, or an
 * Unknown bucket on its own. Country counts stay on the Server's per-Node
 * Peer-record basis, so the same Peer observed on two Nodes is two records
 * and the same country observed on two Networks keeps both contributions.
 */

export type HomeGeoState = 'current' | 'stale' | 'error' | 'disabled' | 'unknown'
export type HomeGeoScope = 'complete' | 'partial' | 'unobserved' | 'unavailable'
/** Peer observation freshness is a separate dimension from the Geo database
 * state. `mixed` means the in-scope Networks disagree and no single value
 * describes the whole scope. */
export type PeerObservation = 'current' | 'stale' | 'unknown' | 'mixed'

export type HomeGeoCountry = {
  code: string
  count: number
  staleCount: number
  /** Server-provided representative location, or null when the Server has
   * none for this country. */
  point: RepresentativePoint | null
}

export type HomeGeoOverview = {
  /** The scope the counts cover: `All Networks`, one Network display name,
   * or the Node display name for the Node Peer Country View. */
  scopeLabel: string
  /** Home's Network filter key, exposed as the map's `data-network-filter`
   * hook. It is null when the scope is not a Network selection. */
  scopeKey: string | null
  networksInScope: number
  /** In-scope Networks that published a Known/Unknown denominator. */
  networksWithBasis: number
  state: HomeGeoState
  scope: HomeGeoScope
  countries: HomeGeoCountry[]
  knownCountryCount: number | null
  unknownCountryCount: number | null
  unknownWithPublicIpCount: number | null
  unknownWithoutRemoteIpCount: number | null
  availablePeerCount: number | null
  attribution: string | null
  errorReason: string | null
  lastGoodAt: string | null
  staleSince: string | null
  databaseAgeSeconds: number | null
  /** Peer collection freshness, kept separate from the Geo database state. */
  peerObservation: PeerObservation
}

const STATE_SEVERITY: Record<string, number> = { error: 3, stale: 2, current: 1, unknown: 0, disabled: 0 }

export function homeGeoOverview(networks: PublicNetwork[], networkFilter: string): HomeGeoOverview {
  const inScope = networkFilter === 'all'
    ? networks
    : networks.filter((network) => network.networkKey === networkFilter)
  const scopeLabel = networkFilter === 'all'
    ? 'All Networks'
    : inScope[0]?.displayName ?? networkFilter

  const countries = new Map<string, HomeGeoCountry>()
  let knownCountryCount: number | null = null
  let unknownCountryCount: number | null = null
  let unknownWithPublicIpCount: number | null = null
  let unknownWithoutRemoteIpCount: number | null = null
  let availablePeerCount: number | null = null
  let networksWithBasis = 0

  for (const network of inScope) {
    // A Geo Insight that is missing or partial must degrade to Unknown for
    // that Network; it must never crash Home or fabricate a zero.
    const geo = network.geo ?? {}
    if (geo.knownCountryCount != null && geo.unknownCountryCount != null) {
      networksWithBasis += 1
      knownCountryCount = (knownCountryCount ?? 0) + geo.knownCountryCount
      unknownCountryCount = (unknownCountryCount ?? 0) + geo.unknownCountryCount
    }
    unknownWithPublicIpCount = addNullable(unknownWithPublicIpCount, geo.unknownWithPublicIpCount)
    unknownWithoutRemoteIpCount = addNullable(unknownWithoutRemoteIpCount, geo.unknownWithoutRemoteIpCount)
    availablePeerCount = addNullable(availablePeerCount, geo.availablePeerCount)
    for (const country of geo.countries ?? []) {
      const existing = countries.get(country.countryCode)
      if (!existing) {
        countries.set(country.countryCode, geoCountry(country))
        continue
      }
      existing.count += country.count
      existing.staleCount += country.staleCount
      // A Server representative point is filled in from the first Network
      // that has one; countries whose coordinates are missing on every
      // Network stay unplottable instead of being placed at a guessed spot.
      existing.point ??= geoCountry(country).point
    }
  }

  const worst = worstGeoNetwork(inScope)
  return {
    scopeLabel,
    scopeKey: networkFilter,
    networksInScope: inScope.length,
    networksWithBasis,
    state: combinedState(inScope),
    scope: combinedScope(inScope),
    countries: [...countries.values()].sort(byCountThenCode),
    knownCountryCount,
    unknownCountryCount,
    unknownWithPublicIpCount,
    unknownWithoutRemoteIpCount,
    availablePeerCount,
    attribution: inScope.map((network) => network.geo?.attribution).find(isPresent) ?? null,
    errorReason: worst?.geo?.errorReason ?? null,
    lastGoodAt: worst?.geo?.lastGoodAt ?? null,
    staleSince: worst?.geo?.staleSince ?? null,
    databaseAgeSeconds: worst?.geo?.databaseAgeSeconds ?? null,
    peerObservation: combinedPeerObservation(inScope),
  }
}

/**
 * Node Peer Country View overview (issue #201): the same projection shape as
 * Home for exactly one Node's Geo Insight, so the shared map component and
 * its state vocabulary need no Node-specific branch. The Server owns the
 * scope and every count; this only reshapes fields the API already returned.
 */
export function nodeGeoOverview(node: PublicNodeDetail): HomeGeoOverview {
  const geo = node.geo ?? {}
  const countries = (geo.countries ?? []).map(geoCountry).sort(byCountThenCode)
  const hasBasis = geo.knownCountryCount != null && geo.unknownCountryCount != null
  const rawScope = geo.scope
  const scope = normalizeNodeGeoScope(rawScope)
  // A Node scope is complete, never-observed, or unavailable (Geo Disabled or
  // failed). Partial describes a Network with unobserved Nodes, so a payload
  // claiming it for one Node is not a Node scope and reads Unknown instead of
  // borrowing that Network wording.
  const nodeScope = rawScope === 'complete' || rawScope === 'unobserved' || rawScope === 'unavailable'
  return {
    scopeLabel: node.displayName ?? node.nodeId,
    // The Node scope is not a Home Network selection, so the map carries no
    // Network-filter hook.
    scopeKey: null,
    networksInScope: 1,
    networksWithBasis: hasBasis ? 1 : 0,
    state: nodeScope ? normalizeGeoState(geo.state) : 'unknown',
    scope,
    countries,
    knownCountryCount: geo.knownCountryCount ?? null,
    unknownCountryCount: geo.unknownCountryCount ?? null,
    unknownWithPublicIpCount: geo.unknownWithPublicIpCount ?? null,
    unknownWithoutRemoteIpCount: geo.unknownWithoutRemoteIpCount ?? null,
    availablePeerCount: geo.availablePeerCount ?? null,
    attribution: geo.attribution ?? null,
    errorReason: geo.errorReason ?? null,
    lastGoodAt: geo.lastGoodAt ?? null,
    staleSince: geo.staleSince ?? null,
    databaseAgeSeconds: geo.databaseAgeSeconds ?? null,
    peerObservation: peerFreshness(node.peers?.freshness),
  }
}

/** What the map slot presents: the Server's own Geo database state, or one of
 * the slots only the client knows. `starting` is an in-flight projection,
 * `unavailable` is no projection at all, and `empty` is a scope with no
 * unit in it. */
export type GeoMapStatus = HomeGeoState | 'starting' | 'empty' | 'unavailable'

/** The map status is the one piece of state the caller already knows and the
 * component must not recompute: pass the loaded overview with whether its
 * source request is still loading or has no projection. */
export function geoMapStatus(
  overview: HomeGeoOverview,
  source: { loading: boolean; hasProjection: boolean },
): GeoMapStatus {
  if (source.loading) return 'starting'
  if (!source.hasProjection) return 'unavailable'
  if (overview.networksInScope === 0) return 'empty'
  return overview.state
}

function normalizeGeoState(value: string | null | undefined): HomeGeoState {
  return value != null && isHomeGeoState(value) ? value : 'unknown'
}

/** A Node scope is complete, never-observed, or unavailable (Geo Disabled or
 * failed), never partial. Any other value is not a Node scope this map can
 * trust, so it degrades to unavailable. */
function normalizeNodeGeoScope(value: string | null | undefined): HomeGeoScope {
  return value === 'complete' || value === 'unobserved' || value === 'unavailable'
    ? value
    : 'unavailable'
}

/** One Server country count as the map's country shape. */
function geoCountry(country: PublicCountryCount): HomeGeoCountry {
  return {
    code: country.countryCode,
    count: country.count,
    staleCount: country.staleCount,
    point: representativePoint(country.centroidLat, country.centroidLon),
  }
}

/** The one ordering every Peer country map uses. */
function byCountThenCode(left: HomeGeoCountry, right: HomeGeoCountry): number {
  return right.count - left.count || left.code.localeCompare(right.code)
}

function representativePoint(lat: number | null | undefined, lon: number | null | undefined): RepresentativePoint | null {
  if (lat == null || lon == null) return null
  return { lat, lon }
}

function isPresent(value: string | null | undefined): value is string {
  return typeof value === 'string' && value.length > 0
}

function addNullable(total: number | null, value: number | null | undefined): number | null {
  if (value == null) return total
  return (total ?? 0) + value
}

/** The Network carrying the most severe Geo database state, so its reason,
 * last-good time, and age stay attached to the aggregate explanation. */
function worstGeoNetwork(networks: PublicNetwork[]): PublicNetwork | undefined {
  let worst: PublicNetwork | undefined
  let worstSeverity = -1
  for (const network of networks) {
    const severity = STATE_SEVERITY[network.geo?.state ?? 'unknown'] ?? 0
    if (severity > worstSeverity) {
      worst = network
      worstSeverity = severity
    }
  }
  if (worstSeverity > 0) return worst
  return networks.find((network) => isPresent(network.geo?.lastGoodAt)) ?? worst
}

/** Geo database state for the whole scope: one Error or Stale Network is
 * never hidden by a healthy neighbour, and a mixed scope is Unknown rather
 * than a claim of Current. */
function combinedState(networks: PublicNetwork[]): HomeGeoState {
  if (networks.length === 0) return 'unknown'
  const states = new Set(networks.map((network) => network.geo?.state ?? 'unknown'))
  if (states.size === 1) {
    const [only] = [...states]
    return isHomeGeoState(only) ? only : 'unknown'
  }
  if (states.has('error')) return 'error'
  if (states.has('stale')) return 'stale'
  return 'unknown'
}

function isHomeGeoState(value: string): value is HomeGeoState {
  return ['current', 'stale', 'error', 'disabled', 'unknown'].includes(value)
}

/** Scope completeness for the whole selection. Any Network without a country
 * basis makes the aggregate Partial, and an unavailable projection keeps the
 * whole scope Unavailable so a partial list is never read as complete. */
function combinedScope(networks: PublicNetwork[]): HomeGeoScope {
  if (networks.length === 0) return 'unavailable'
  const scopes = networks.map((network) => network.geo?.scope ?? 'unavailable')
  if (scopes.includes('unavailable')) return 'unavailable'
  if (scopes.every((scope) => scope === 'complete')) return 'complete'
  if (scopes.every((scope) => scope === 'unobserved')) return 'unobserved'
  return 'partial'
}

/** The Server's Peer freshness vocabulary is `current`, `stale`, and
 * `unknown`; anything else is not a freshness the Server established. */
function peerFreshness(freshness: string | null | undefined): PeerObservation {
  return freshness === 'current' || freshness === 'stale' ? freshness : 'unknown'
}

function combinedPeerObservation(networks: PublicNetwork[]): PeerObservation {
  if (networks.length === 0) return 'unknown'
  const states = new Set(networks.map((network) => peerFreshness(network.peers?.freshness)))
  if (states.size === 1) {
    const [only] = [...states]
    return only
  }
  return 'mixed'
}
