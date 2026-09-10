#!/usr/bin/env node
/**
 * Build the locally hosted world country geometry used by the Home Peer
 * country map (issue #133).
 *
 * The committed asset is derived from Natural Earth 1:110m Admin 0 Countries
 * (public domain, https://www.naturalearthdata.com/) at the pinned release
 * below. PlatPulse never fetches map geometry at runtime: the generator runs
 * offline, writes one versioned JSON file into the WebUI's static assets, and
 * the Server serves that file from its own origin.
 *
 * Usage: node scripts/build-world-geometry.mjs [--source <path-or-url>]
 */
import { readFile, mkdir, writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

/** Pinned upstream release; never "latest". */
const SOURCE_URL = 'https://raw.githubusercontent.com/nvkelso/natural-earth-vector/v5.1.2/geojson/ne_110m_admin_0_countries.geojson'
const SOURCE_LABEL = 'Natural Earth 1:110m Admin 0 Countries (v5.1.2)'
const LICENSE = 'Public domain (Natural Earth); no attribution required, credited in the WebUI.'
const ATTRIBUTION = 'Country outlines: Natural Earth 1:110m (public domain).'

/** Equirectangular plot space. Latitude is cropped the way web maps crop
 * Antarctica; the full longitude range always stays visible. */
const PROJECTION = { width: 1000, height: 394, maxLat: 84, minLat: -58 }

/** Douglas-Peucker tolerance in degrees; ~15 km at the equator, invisible at
 * the rendered map size while keeping every country recognizable. */
const SIMPLIFY_TOLERANCE = 0.14

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const OUTPUT = path.join(ROOT, 'public/assets/geo/world-countries-110m-v1.json')

function projectX(lon) {
  return ((lon + 180) / 360) * PROJECTION.width
}

function projectY(lat) {
  const clamped = Math.max(PROJECTION.minLat, Math.min(PROJECTION.maxLat, lat))
  return ((PROJECTION.maxLat - clamped) / (PROJECTION.maxLat - PROJECTION.minLat)) * PROJECTION.height
}

function squaredSegmentDistance(point, start, end) {
  let x = start[0]
  let y = start[1]
  let dx = end[0] - x
  let dy = end[1] - y
  if (dx !== 0 || dy !== 0) {
    const t = ((point[0] - x) * dx + (point[1] - y) * dy) / (dx * dx + dy * dy)
    if (t > 1) {
      x = end[0]
      y = end[1]
    } else if (t > 0) {
      x += dx * t
      y += dy * t
    }
  }
  dx = point[0] - x
  dy = point[1] - y
  return dx * dx + dy * dy
}

/** Iterative Douglas-Peucker so a pathological ring can never blow the stack. */
function simplify(points, tolerance) {
  if (points.length <= 4) return points
  const squaredTolerance = tolerance * tolerance
  const keep = new Uint8Array(points.length)
  keep[0] = 1
  keep[points.length - 1] = 1
  const stack = [[0, points.length - 1]]
  while (stack.length > 0) {
    const [first, last] = stack.pop()
    let index = -1
    let maxDistance = squaredTolerance
    for (let i = first + 1; i < last; i += 1) {
      const distance = squaredSegmentDistance(points[i], points[first], points[last])
      if (distance > maxDistance) {
        index = i
        maxDistance = distance
      }
    }
    if (index !== -1) {
      keep[index] = 1
      stack.push([first, index], [index, last])
    }
  }
  const result = []
  for (let i = 0; i < points.length; i += 1) if (keep[i]) result.push(points[i])
  return result
}

function ringPath(ring) {
  const simplified = simplify(ring, SIMPLIFY_TOLERANCE)
  const points = []
  for (const [lon, lat] of simplified) {
    const x = Number(projectX(lon).toFixed(1))
    const y = Number(projectY(lat).toFixed(1))
    const previous = points.at(-1)
    if (previous && previous[0] === x && previous[1] === y) continue
    points.push([x, y])
  }
  if (points.length < 4) return ''
  return 'M' + points.map(([x, y]) => x + ' ' + y).join('L') + 'Z'
}

function featureCode(properties) {
  for (const key of ['ISO_A2', 'ISO_A2_EH']) {
    const value = properties[key]
    if (typeof value === 'string' && /^[A-Z]{2}$/.test(value)) return value
  }
  return null
}

async function readSource(source) {
  if (source.startsWith('http://') || source.startsWith('https://')) {
    const response = await fetch(source)
    if (!response.ok) throw new Error('geometry source returned HTTP ' + response.status)
    return response.json()
  }
  return JSON.parse(await readFile(source, 'utf8'))
}

async function main() {
  const sourceIndex = process.argv.indexOf('--source')
  const source = sourceIndex === -1 ? SOURCE_URL : process.argv[sourceIndex + 1]
  const geojson = await readSource(source)

  /** @type {Map<string, string[]>} */
  const pathsByCode = new Map()
  let skipped = 0
  for (const feature of geojson.features) {
    const code = featureCode(feature.properties ?? {})
    if (!code) {
      skipped += 1
      continue
    }
    const geometry = feature.geometry
    if (!geometry) continue
    const polygons = geometry.type === 'Polygon' ? [geometry.coordinates] : geometry.coordinates
    const paths = pathsByCode.get(code) ?? []
    for (const polygon of polygons) {
      for (const ring of polygon) {
        const path = ringPath(ring)
        if (path) paths.push(path)
      }
    }
    pathsByCode.set(code, paths)
  }

  const countries = [...pathsByCode.entries()]
    .map(([code, paths]) => ({ code, path: paths.join('') }))
    .filter((country) => country.path.length > 0)
    .sort((left, right) => left.code.localeCompare(right.code))

  const asset = {
    asset: 'world-countries-110m',
    version: 1,
    projection: PROJECTION,
    source: SOURCE_URL,
    sourceLabel: SOURCE_LABEL,
    license: LICENSE,
    attribution: ATTRIBUTION,
    generatedBy: 'platpulse-web/scripts/build-world-geometry.mjs',
    countries,
  }

  await mkdir(path.dirname(OUTPUT), { recursive: true })
  await writeFile(OUTPUT, JSON.stringify(asset) + '\n')
  const bytes = (await readFile(OUTPUT)).byteLength
  console.log(
    'wrote ' + path.relative(ROOT, OUTPUT) + ': ' + countries.length + ' countries, ' +
    (bytes / 1024).toFixed(1) + ' KiB' + (skipped ? ', ' + skipped + ' features without an ISO code skipped' : ''),
  )
}

await main()
