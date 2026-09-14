/**
 * Locally hosted world geometry for the Home Peer-country map (issue #133).
 *
 * The committed asset is vendored by `scripts/vendor-emerald-assets.mjs` from
 * apache/echarts-www@4e9b6889abf0995b1784610a907cdb8821f59996 (Apache-2.0) —
 * the very file upstream komari-theme-emerald fetches from four CDNs at
 * runtime, unpinned. PlatPulse pins the revision, checks the checksum, serves
 * it from its own origin, and adds an `iso2` property to every feature so the
 * browser never carries a country-name table.
 *
 * 217 polygons and 26,273 coordinate points, against 175 / 7,366 in the
 * Natural Earth 1:110m build this file replaced.
 */

/** A Server-provided country representative location. Both coordinates are
 * required: half a point is no point at all, and the browser never invents a
 * substitute such as [0, 0]. */
export type RepresentativePoint = { lat: number; lon: number }

export type WorldFeature = {
  properties?: { name?: string; iso2?: string }
  geometry?: { type?: string; coordinates?: unknown }
}

export type WorldGeoJson = {
  type: string
  features: WorldFeature[]
  platpulseProvenance?: Record<string, unknown>
}

/** Versioned, immutable static asset served from the WebUI's own origin. */
export const WORLD_GEOMETRY_PATH = '/assets/geo/world-countries-echarts-www-v1.json'

/** The registered ECharts map name; never a CDN lookup. */
export const WORLD_MAP_NAME = 'platpulse-world'

export async function loadWorldGeoJson(signal?: AbortSignal): Promise<WorldGeoJson> {
  const response = await fetch(WORLD_GEOMETRY_PATH, { signal, cache: 'force-cache' })
  if (!response.ok) throw new Error('world geometry unavailable: ' + response.status)
  const payload: unknown = await response.json()
  if (
    typeof payload !== 'object' ||
    payload === null ||
    (payload as WorldGeoJson).type !== 'FeatureCollection' ||
    !Array.isArray((payload as WorldGeoJson).features)
  ) {
    throw new Error('world geometry is not a FeatureCollection')
  }
  return payload as WorldGeoJson
}

/**
 * ISO-3166 alpha-2 to the region name ECharts matches a `map` series datum
 * against. Built from the asset itself, so the two can never drift.
 */
export function regionNameByCode(geojson: WorldGeoJson): Map<string, string> {
  const names = new Map<string, string>()
  for (const feature of geojson.features) {
    const name = feature.properties?.name
    const code = feature.properties?.iso2
    if (typeof name === 'string' && typeof code === 'string' && code && !names.has(code)) {
      names.set(code, name)
    }
  }
  return names
}
