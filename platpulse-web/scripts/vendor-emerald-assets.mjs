#!/usr/bin/env node
/**
 * Vendor the two asset sets the Emerald visual migration needs, from pinned
 * upstream revisions. Run offline by a developer; the built WebUI never
 * fetches any of this at runtime (the Server enforces connect-src 'self').
 *
 * 1. World geometry for the Home Peer-country map.
 *    Upstream komari-theme-emerald fetches its GeoJSON from cdn.jsdelivr.net
 *    / fastly / gcore / raw.githubusercontent at runtime, unpinned
 *    ("@master"). PlatPulse vendors one pinned copy instead. The file is
 *    Apache-2.0 (apache/echarts-www) and carries 217 named polygons and
 *    26,273 coordinate points, against 175 / 7,366 in the Natural Earth
 *    1:110m build it replaces (scripts/build-world-geometry.mjs).
 *
 * 2. Country flags referenced by the map tooltip and node cards
 *    (/assets/flags/{iso2}.svg, lowercase). These are NOT in the upstream
 *    repository at all: the Komari host serves them. PlatPulse vendors
 *    flag-icons (MIT) instead. Upstream's own file names and aspect ratio
 *    are unverifiable from the public repo, so the aspect ratio is recorded
 *    as a known difference.
 *
 * Usage: node scripts/vendor-emerald-assets.mjs
 */
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile, cp } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import os from 'node:os'
import path from 'node:path'

/** Pinned Apache ECharts world map (apache/echarts-www). Never "master". */
const GEO_COMMIT = '4e9b6889abf0995b1784610a907cdb8821f59996'
const GEO_URL = 'https://raw.githubusercontent.com/apache/echarts-www/' + GEO_COMMIT + '/asset/map/json/world.json'
const GEO_SHA256 = '049b334579e5a42d5d16c72d014d380e048e39fc1504049f212acb589484d2fa'
const GEO_OUTPUT = 'world-countries-echarts-www-v1.json'
const GEO_LICENSE_URL = 'https://raw.githubusercontent.com/apache/echarts-www/' + GEO_COMMIT + '/LICENSE'
const GEO_LICENSE_OUTPUT = 'apache-echarts-www-LICENSE.txt'

/** Pinned flag-icons release (MIT). */
const FLAGS_VERSION = '7.5.0'
const FLAG_DIR = '4x3'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const GEO_DIR = path.join(ROOT, 'public/assets/geo')
const FLAGS_DIR = path.join(ROOT, 'public/assets/flags')

async function fetchChecked(url) {
  const response = await fetch(url, { redirect: 'follow' })
  if (!response.ok) throw new Error('GET ' + url + ' -> ' + response.status)
  return Buffer.from(await response.arrayBuffer())
}

/**
 * Country-name to ISO-3166 alpha-2 resolution for the map choropleth.
 *
 * Upstream rewrites every feature name to ISO2 at runtime from a 1718-line
 * region table plus 35 ECharts aliases. PlatPulse resolves the mapping once,
 * offline, and writes `properties.iso2` into the vendored asset, so the
 * browser never carries that table. Names that resolve to nothing are
 * reported and left without a code (the country still renders its base fill
 * and its Peer dots still plot from the Server centroid).
 */
const NE_URL = 'https://raw.githubusercontent.com/nvkelso/natural-earth-vector/v5.1.2/geojson/ne_110m_admin_0_countries.geojson'

/** ECharts' abbreviated feature names that Natural Earth spells differently. */
const ECHARTS_NAME_ALIASES = {
  'congo': 'CG', 'dem. rep. congo': 'CD', 'macedonia': 'MK', 'n. cyprus': 'CY',
  'swaziland': 'SZ', 'u.s. virgin is.': 'VI', 'w. sahara': 'EH', 'somaliland': 'SO',
  'bosnia and herz.': 'BA', 'central african rep.': 'CF', 'dominican rep.': 'DO',
  'eq. guinea': 'GQ', 'falkland is.': 'FK', 'fr. s. antarctic lands': 'TF',
  'guinea-bissau': 'GW', 'korea': 'KR', 'lao pdr': 'LA', 'solomon is.': 'SB',
  's. sudan': 'SS', 'timor-leste': 'TL', 'trinidad and tobago': 'TT',
  'turks and caicos is.': 'TC', 'cayman is.': 'KY', 'czech rep.': 'CZ',
  'n. mariana is.': 'MP', 'br. indian ocean ter.': 'IO', 'cook is.': 'CK',
  'french polynesia': 'PF', 'new caledonia': 'NC', 'niger': 'NE', 'niue': 'NU',
  'palestine': 'PS', 'sao tome and principe': 'ST', 'kosovo': 'XK',
  'serbia': 'RS', 'montenegro': 'ME', 'puerto rico': 'PR', 'greenland': 'GL',
  'antarctica': 'AQ', 'western sahara': 'EH',
  // Natural Earth 1:110m omits these entirely, or spells them differently.
  'liechtenstein': 'LI', 'aland': 'AX', 'andorra': 'AD', 'american samoa': 'AS',
  'antigua and barb.': 'AG', 'bahrain': 'BH', 'bermuda': 'BM', 'barbados': 'BB',
  'cape verde': 'CV', 'comoros': 'KM', 'curacao': 'CW', 'dominica': 'DM',
  'faeroe is.': 'FO', 'micronesia': 'FM', 'grenada': 'GD', 'guam': 'GU',
  'heard i. and mcdonald is.': 'HM', 'isle of man': 'IM', 'jersey': 'JE',
  'kiribati': 'KI', 'saint lucia': 'LC', 'malta': 'MT', 'montserrat': 'MS',
  'mauritius': 'MU', 'norway': 'NO', 'palau': 'PW', 'fr. polynesia': 'PF',
  'singapore': 'SG', 's. geo. and s. sandw. is.': 'GS', 'saint helena': 'SH',
  'st. pierre and miquelon': 'PM', 'sao tome and principe': 'ST', 'seychelles': 'SC',
  'tonga': 'TO', 'st. vin. and gren.': 'VC', 'samoa': 'WS',
  // Siachen Glacier is a disputed area with no ISO 3166-1 code: left unresolved.
}

/** Lowercased, diacritic-free key so "São Tomé" and "Sao Tome" agree. */
function nameKey(value) {
  return value.trim().toLowerCase().normalize('NFD').replace(/[\u0300-\u036f]/g, '')
}

async function resolveIso2(geojson) {
  const naturalEarth = JSON.parse((await fetchChecked(NE_URL)).toString('utf8'))
  const byName = new Map()
  for (const feature of naturalEarth.features) {
    const properties = feature.properties ?? {}
    // Natural Earth stores -99 for several countries in ISO_A2 and supplies the
    // real code in ISO_A2_EH.
    const candidates = [properties.ISO_A2, properties.ISO_A2_EH]
    const code = candidates.find((value) => typeof value === 'string' && value.length === 2 && value !== '-9') ?? null
    if (!code) continue
    for (const key of [properties.NAME, properties.NAME_LONG, properties.ADMIN, properties.SOVEREIGNT]) {
      if (typeof key !== 'string') continue
      const normalized = nameKey(key)
      if (normalized && !byName.has(normalized)) byName.set(normalized, code)
    }
  }
  const unresolved = []
  let resolved = 0
  for (const feature of geojson.features) {
    const name = typeof feature.properties?.name === 'string' ? feature.properties.name : ''
    const normalized = nameKey(name)
    const code = ECHARTS_NAME_ALIASES[normalized] ?? byName.get(normalized) ?? null
    if (code) resolved += 1
    else if (normalized) unresolved.push(name)
    feature.properties = { ...feature.properties, ...(code ? { iso2: code } : {}) }
  }
  console.log('iso2: ' + resolved + '/' + geojson.features.length + ' features resolved')
  if (unresolved.length) console.log('iso2 unresolved: ' + unresolved.join(' | '))
}

async function vendorGeometry() {
  const raw = await fetchChecked(GEO_URL)
  const sha = createHash('sha256').update(raw).digest('hex')
  if (sha !== GEO_SHA256) {
    throw new Error('world.json checksum changed: ' + sha + ' (expected ' + GEO_SHA256 + ')')
  }
  const geojson = JSON.parse(raw.toString('utf8'))
  if (!Array.isArray(geojson.features)) throw new Error('world.json is not a GeoJSON FeatureCollection')
  await resolveIso2(geojson)

  let points = 0
  const walk = (coordinates) => {
    if (typeof coordinates[0] === 'number') {
      points += 1
      return
    }
    for (const child of coordinates) walk(child)
  }
  for (const feature of geojson.features) walk(feature.geometry.coordinates)

  const asset = {
    ...geojson,
    platpulseProvenance: {
      asset: 'world-countries-echarts-www',
      version: 1,
      source: GEO_URL,
      sourceLabel: 'Apache ECharts world map (apache/echarts-www@' + GEO_COMMIT.slice(0, 7) + ')',
      license: 'Apache-2.0 (Apache ECharts); geometry derived from Natural Earth (public domain)',
      attribution: 'Country outlines: Apache ECharts world map (Apache-2.0).',
      generatedBy: 'platpulse-web/scripts/vendor-emerald-assets.mjs',
      sha256: sha,
      features: geojson.features.length,
      coordinatePoints: points,
      replaces: 'world-countries-110m-v1.json (Natural Earth 1:110m, 175 countries, 7,366 points)',
    },
  }

  await mkdir(GEO_DIR, { recursive: true })
  await writeFile(path.join(GEO_DIR, GEO_OUTPUT), JSON.stringify(asset) + '\n')
  await writeFile(path.join(GEO_DIR, GEO_LICENSE_OUTPUT), await fetchChecked(GEO_LICENSE_URL))
  const bytes = (await readFile(path.join(GEO_DIR, GEO_OUTPUT))).byteLength
  console.log('geo: ' + geojson.features.length + ' features, ' + points + ' points, ' + (bytes / 1024).toFixed(1) + ' KiB -> ' + GEO_OUTPUT)
}

async function vendorFlags() {
  const temp = await mkdtemp(path.join(os.tmpdir(), 'platpulse-flags-'))
  try {
    const packed = execFileSync('npm', ['pack', 'flag-icons@' + FLAGS_VERSION, '--silent'], {
      cwd: temp,
      encoding: 'utf8',
    }).trim().split('\n').pop()
    execFileSync('tar', ['-xzf', path.join(temp, packed)], { cwd: temp })
    const source = path.join(temp, 'package', 'flags', FLAG_DIR)
    const files = (await readdir(source)).filter((name) => name.endsWith('.svg'))
    await rm(FLAGS_DIR, { recursive: true, force: true })
    await mkdir(FLAGS_DIR, { recursive: true })
    for (const name of files) await cp(path.join(source, name), path.join(FLAGS_DIR, name))
    await cp(path.join(temp, 'package', 'LICENSE'), path.join(FLAGS_DIR, 'LICENSE-flag-icons.txt'))
    let bytes = 0
    for (const name of files) bytes += (await readFile(path.join(FLAGS_DIR, name))).byteLength
    console.log('flags: ' + files.length + ' svg (' + FLAG_DIR + '), ' + (bytes / 1024).toFixed(1) + ' KiB')
  } finally {
    await rm(temp, { recursive: true, force: true })
  }
}

await vendorGeometry()
await vendorFlags()
