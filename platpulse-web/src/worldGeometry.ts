/**
 * Locally hosted world country geometry (issue #133).
 *
 * The committed asset is generated offline from Natural Earth 1:110m Admin 0
 * Countries by `scripts/build-world-geometry.mjs`. PlatPulse never fetches
 * map geometry from a runtime CDN or tile service: the WebUI loads one
 * versioned file from its own origin, and a missing or unusable file degrades
 * locally without touching Peer country counts.
 */

export type WorldProjection = {
  width: number
  height: number
  maxLat: number
  minLat: number
}

export type WorldGeometryCountry = { code: string; path: string }

/** A Server-provided country representative location. Both coordinates are
 * required: half a point is no point at all, and the browser never invents a
 * substitute such as `[0, 0]`. */
export type RepresentativePoint = { lat: number; lon: number }

export type WorldGeometry = {
  version: number
  attribution: string
  sourceLabel: string
  projection: WorldProjection
  countries: WorldGeometryCountry[]
}

/** Versioned, immutable static asset served from the WebUI's own origin. */
export const WORLD_GEOMETRY_PATH = '/assets/geo/world-countries-110m-v1.json'

/** Equirectangular placement of a Server-provided representative location.
 * Only country centroids are ever plotted; the browser never derives a point
 * from Peer addresses. */
export function projectCountryPoint(projection: WorldProjection, point: RepresentativePoint): { x: number; y: number } {
  const boundedLat = Math.max(projection.minLat, Math.min(projection.maxLat, point.lat))
  return {
    x: ((point.lon + 180) / 360) * projection.width,
    y: ((projection.maxLat - boundedLat) / (projection.maxLat - projection.minLat)) * projection.height,
  }
}

/** The Server's SPA fallback answers unknown paths with `index.html`, so a
 * missing asset arrives as HTML with a 200. Both the parse and this shape
 * check must fail before any geometry is drawn. */
export function parseWorldGeometry(value: unknown): WorldGeometry | null {
  if (typeof value !== 'object' || value === null) return null
  const candidate = value as Partial<WorldGeometry>
  const projection = candidate.projection
  if (
    typeof projection !== 'object' || projection === null
    || !isPositive(projection.width) || !isPositive(projection.height)
    || !Number.isFinite(projection.minLat) || !Number.isFinite(projection.maxLat)
    || projection.maxLat <= projection.minLat
  ) return null
  if (!Array.isArray(candidate.countries)) return null
  const countries = candidate.countries.flatMap((country) => {
    if (typeof country !== 'object' || country === null) return []
    const { code, path } = country as Partial<WorldGeometryCountry>
    if (typeof code !== 'string' || !/^[A-Z]{2}$/.test(code)) return []
    if (typeof path !== 'string' || !path.startsWith('M') || path.length < 8) return []
    return [{ code, path }]
  })
  if (countries.length === 0) return null
  return {
    version: typeof candidate.version === 'number' ? candidate.version : 0,
    attribution: typeof candidate.attribution === 'string' ? candidate.attribution : '',
    sourceLabel: typeof candidate.sourceLabel === 'string' ? candidate.sourceLabel : '',
    projection: {
      width: projection.width,
      height: projection.height,
      maxLat: projection.maxLat,
      minLat: projection.minLat,
    },
    countries,
  }
}

export async function loadWorldGeometry(signal?: AbortSignal): Promise<WorldGeometry> {
  const response = await fetch(WORLD_GEOMETRY_PATH, { signal, cache: 'force-cache' })
  if (!response.ok) throw new Error('map geometry request failed with ' + response.status)
  const parsed = parseWorldGeometry(JSON.parse(await response.text()))
  if (!parsed) throw new Error('map geometry payload is not usable')
  return parsed
}

function isPositive(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value > 0
}
