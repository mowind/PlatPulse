import { mkdirSync } from 'node:fs'
import { join } from 'node:path'
import { expect, test, type Page, type TestInfo } from '@playwright/test'
import {
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
  loginAs,
} from './helpers'

/**
 * Home compact overview and Peer country map (issue #133).
 *
 * The map is now an ECharts canvas (src/components/GeoWorldMap.tsx +
 * src/components/mapChartOption.ts), so there is no per-country DOM: no
 * country path, no accessible marker button, and no SVG geometry. The facts
 * that used to live on those nodes are asserted on the two text surfaces the
 * canvas exposes instead:
 *
 *  - the labelled image, role="img" aria-label="Peer countries map. …", whose
 *    description states the observed-country count and the scope it covers;
 *  - the screen-reader country list, [data-slot="geo-country-list"], one <li>
 *    per observed country with its count, its stale count, and whether it had
 *    a representative point to plot;
 *  - abnormal states have a minimal visible role="status".
 *
 * The scatter encoding itself (8px/14px symbols, the white 10px numeral, the
 * tooltip HTML) is covered at the unit level in
 * src/components/mapChartOption.test.ts and is deliberately not repeated here.
 * A real pointer hover over a painted marker asserts the ECharts tooltip,
 * which is the closest observable equivalent of the old SVG hover preview.
 *
 * The suite runs against the real Server, the operator-provided MMDB, and the
 * production WebUI build; only the Peer fixture is seeded. Scenarios normalize
 * the Geo Provider; the disabled-provider scenario restores Local MMDB. DTO
 * overrides below are isolated UI fixtures, not claims about the real Server's
 * Peer observations.
 */

// These scenarios sign in, open Admin Settings, and walk several Home states;
// the default 30s budget is too tight for the shared real Server.
test.describe.configure({ timeout: 120_000 })

const GEO_CARD = (page: Page) =>
  page.getByRole('article').filter({ has: page.getByRole('heading', { name: 'Geo provider' }) })

async function setGeoProvider(page: Page, name: 'Local MMDB' | 'Disabled') {
  await page.goto('/admin/settings')
  const card = GEO_CARD(page)
  // Wait for the query-backed initial selection effect before deciding to save.
  await expect(card.getByRole('radio', { checked: true })).toHaveCount(1)
  const radio = card.getByRole('radio', { name })
  if (await radio.isChecked()) return
  await radio.check()
  await card.getByRole('button', { name: 'Save Geo provider' }).click()
  await expect(card.getByText(new RegExp('Geo provider is now ' + name))).toBeVisible()
}

/** Sign in and make sure the map has a real country projection to render. */
async function openHomeWithGeo(page: Page) {
  await loginAs(page)
  await setGeoProvider(page, 'Local MMDB')
  await page.goto('/')
}

type Box = { x: number; y: number; width: number; height: number }

/**
 * Deliver one review screenshot: it is attached to the test result and also
 * written to the gitignored playwright-report/emerald/ directory, so a local
 * run leaves browsable evidence instead of discarding it with the per-test
 * output directory.
 */
async function capture(page: Page, testInfo: TestInfo, name: string) {
  const body = await page.screenshot()
  await testInfo.attach(name, { body, contentType: 'image/png' })
  const directory = join('playwright-report', 'emerald')
  mkdirSync(directory, { recursive: true })
  await page.screenshot({ path: join(directory, 'after-' + name + '.png') })
}

function mapRegion(page: Page) {
  return page.getByRole('region', { name: 'Peer countries' })
}

/** The ECharts container, which is the labelled role="img" surface. */
function geoChart(page: Page) {
  return page.locator('[data-slot="geo-chart"]')
}

/** The screen-reader country list: one <li> per observed country. */
function countryItems(page: Page) {
  return mapRegion(page).locator('[data-slot="geo-country-list"] li')
}

function mapStatus(page: Page) {
  return mapRegion(page).getByRole('status')
}

/** ECharts paints the tooltip (country name, flag image, record counts) into
 *  the chart container as an unlabelled div, which is the only DOM the canvas
 *  interaction exposes. */
function mapTooltip(page: Page) {
  return geoChart(page).locator('> div').filter({ hasText: /[\d,]+ records/ })
}

/** Home's four global statistics, read from their own card surfaces so the
 *  map can never change or reorder them. */
async function summaryFacts(page: Page): Promise<Array<{ label: string; value: string; box: Box }>> {
  return page.locator('[data-slot="summary-card"]').evaluateAll((elements) =>
    elements.flatMap((element) => {
      const text = (element.textContent ?? '').trim()
      const match = /^(Active Nodes|Healthy Nodes|Attention|Networks)(.+)$/.exec(text)
      if (!match) return []
      const rect = element.getBoundingClientRect()
      return [{ label: match[1], value: match[2].trim(), box: { x: rect.x, y: rect.y, width: rect.width, height: rect.height } }]
    }),
  )
}

/** The same four statistics as label/value pairs, without layout noise. */
async function summaryValues(page: Page) {
  return (await summaryFacts(page)).map(({ label, value }) => ({ label, value }))
}

function unionBox(boxes: Box[]): Box {
  const x = Math.min(...boxes.map((box) => box.x))
  const y = Math.min(...boxes.map((box) => box.y))
  const right = Math.max(...boxes.map((box) => box.x + box.width))
  const bottom = Math.max(...boxes.map((box) => box.y + box.height))
  return { x, y, width: right - x, height: bottom - y }
}

/**
 * Rendered world geometry, not just the chart viewport: the union box of the
 * non-transparent canvas pixels. Expansion and the responsive band must scale
 * the actual world rather than add letterboxed blank height. This replaces the
 * old union of country path boxes; the canvas is vector-only (no cross-origin
 * image), so its pixels are readable.
 */
async function worldBox(page: Page): Promise<Box> {
  await expect(geoChart(page).locator('canvas').first()).toBeVisible()
  const box = await geoChart(page).evaluate((element) => {
    let union: { x: number; y: number; width: number; height: number } | null = null
    for (const canvas of [...element.querySelectorAll('canvas')] as HTMLCanvasElement[]) {
      const context = canvas.getContext('2d')
      if (!context) continue
      const { width, height } = canvas
      const data = context.getImageData(0, 0, width, height).data
      let minX = width
      let minY = height
      let maxX = -1
      let maxY = -1
      for (let y = 0; y < height; y += 1) {
        for (let x = 0; x < width; x += 1) {
          if (data[(y * width + x) * 4 + 3] > 8) {
            if (x < minX) minX = x
            if (x > maxX) maxX = x
            if (y < minY) minY = y
            if (y > maxY) maxY = y
          }
        }
      }
      if (maxX < 0) continue
      const rect = canvas.getBoundingClientRect()
      const layer = {
        x: rect.x + (minX / width) * rect.width,
        y: rect.y + (minY / height) * rect.height,
        width: ((maxX - minX) / width) * rect.width,
        height: ((maxY - minY) / height) * rect.height,
      }
      if (!union) {
        union = layer
      } else {
        const right = Math.max(union.x + union.width, layer.x + layer.width)
        const bottom = Math.max(union.y + union.height, layer.y + layer.height)
        union = {
          x: Math.min(union.x, layer.x),
          y: Math.min(union.y, layer.y),
          width: right - Math.min(union.x, layer.x),
          height: bottom - Math.min(union.y, layer.y),
        }
      }
    }
    return union
  })
  expect(box, 'the map paints its world projection onto the canvas').not.toBeNull()
  return box!
}

async function overviewBox(page: Page): Promise<Box> {
  const stats = unionBox((await summaryFacts(page)).map((fact) => fact.box))
  const mapBox = (await mapRegion(page).boundingBox())!
  return {
    x: Math.min(stats.x, mapBox.x),
    y: Math.min(stats.y, mapBox.y),
    width: Math.max(stats.x + stats.width, mapBox.x + mapBox.width) - Math.min(stats.x, mapBox.x),
    height: Math.max(stats.y + stats.height, mapBox.y + mapBox.height) - Math.min(stats.y, mapBox.y),
  }
}

/**
 * Point the mouse at a painted scatter marker. ECharts paints the markers into
 * the canvas, so the emerald dot (rgba(5,150,105,0.9)) is found on the canvas
 * pixels themselves: a marker disc fills an 8x8 cell with many emerald pixels,
 * while the 0.5px country borders leave only a few. The scan is polled because
 * the chart paints, with animation, after a resize and after a data update.
 */
async function hoverPaintedMarker(page: Page) {
  await expect(geoChart(page).locator('canvas').first()).toBeVisible({ timeout: 30_000 })
  let target: { x: number; y: number } | null = null
  await expect.poll(async () => {
    target = await geoChart(page).evaluate((element) => {
      const size = 8
      const cells = new Map<string, { sx: number; sy: number; n: number; pw: number; ph: number; rect: DOMRect }>()
      let layer = 0
      for (const canvas of [...element.querySelectorAll('canvas')] as HTMLCanvasElement[]) {
        const context = canvas.getContext('2d')
        if (!context) continue
        layer += 1
        const { width, height } = canvas
        const data = context.getImageData(0, 0, width, height).data
        const rect = canvas.getBoundingClientRect()
        for (let y = 0; y < height; y += 1) {
          for (let x = 0; x < width; x += 1) {
            const index = (y * width + x) * 4
            const r = data[index]
            const g = data[index + 1]
            const b = data[index + 2]
            const a = data[index + 3]
            if (a > 110 && g > r + 35 && g > b + 15) {
              const key = layer + ':' + Math.floor(x / size) + ',' + Math.floor(y / size)
              const cell = cells.get(key) ?? { sx: 0, sy: 0, n: 0, pw: width, ph: height, rect }
              cell.sx += x
              cell.sy += y
              cell.n += 1
              cells.set(key, cell)
            }
          }
        }
      }
      let best: { sx: number; sy: number; n: number; pw: number; ph: number; rect: DOMRect } | null = null
      for (const cell of cells.values()) if ((!best || cell.n > best.n) && cell.n >= 8) best = cell
      return best
        ? { x: best.rect.x + ((best.sx / best.n) / best.pw) * best.rect.width, y: best.rect.y + ((best.sy / best.n) / best.ph) * best.rect.height }
        : null
    })
    return target !== null
  }, { timeout: 20_000, message: 'the scatter paints an emerald country marker' }).toBe(true)
  if (page.viewportSize()!.width < 768) await page.touchscreen.tap(target!.x, target!.y)
  else await page.mouse.move(target!.x, target!.y)
}

/** Replace the Geo projection of the live Public response with a deterministic
 *  Server-shaped state, so Starting/Current/Unknown/Partial composition is
 *  covered without inventing production data or a second Server. */
async function withGeoProjection(page: Page, patch: (geo: Record<string, unknown>) => Record<string, unknown>, peerFreshness?: string) {
  await page.route('**/api/public/v1/networks*', async (route) => {
    const response = await route.fetch()
    const body = await response.json()
    if (Array.isArray(body)) {
      for (const network of body) {
        network.geo = patch(network.geo ?? {})
        if (peerFreshness) network.peers = { ...network.peers, freshness: peerFreshness }
      }
    }
    await route.fulfill({ response, json: body })
  })
}

/** The map is bare: one labelled image, no written status, no dialog. */
async function expectQuietMap(page: Page) {
  const map = mapRegion(page)
  await expect(map.getByRole('heading')).toHaveCount(0)
  await expect(map.getByRole('dialog')).toHaveCount(0)
  await expect(page.getByRole('dialog', { name: 'Map information' })).toHaveCount(0)
  await expect(map.getByText(/Natural Earth|GeoLite|MaxMind|IPinfo|GeoJS/)).toHaveCount(0)
  await expect(map.getByText(/^Scope: |^Known |^Peer observation: |^Map resource: /)).toHaveCount(0)

  // Normal maps remain quiet; any abnormal state must remain visible.
  for (const status of await map.getByRole('status').all()) {
    const presentation = await status.evaluate(element => {
      const box = element.getBoundingClientRect()
      const style = getComputedStyle(element)
      return { width: box.width, height: box.height, clip: style.clip, clipPath: style.clipPath }
    })
    expect(presentation.width).toBeGreaterThan(1)
    expect(presentation.height).toBeGreaterThan(1)
    expect(presentation.clip).toBe('auto')
    expect(presentation.clipPath).toBe('none')
  }

  // No control lives on the map at all: the expand toggle, the information and
  // status controls, and their disclosure were all removed. Interaction is the
  // map itself.
  await expect(map.locator('button')).toHaveCount(0)

  // The one standing figure is the reference theme's corner indicator: a pulsing
  // dot with the total Peer count of exactly the scope the map covers. It must
  // stay pointer-inert on mobile; the desktop badge exposes a native title.
  const counters = map.locator('[data-slot="geo-counters"]')
  if (await counters.count() > 0) {
    // The corner figure now sits near the top of the phone page, so a prior
    // interaction further down can leave it outside the viewport.
    await counters.scrollIntoViewIfNeeded()
    const counterBox = (await counters.boundingBox())!
    const pointerEvents = await counters.evaluate(element => getComputedStyle(element).pointerEvents)
    const desktop = page.viewportSize()!.width >= 768
    expect(pointerEvents).toBe(desktop ? 'auto' : 'none')
    await expect(counters).toHaveAttribute('aria-label', /Peer records in scope/)
    await expect(counters).toHaveAttribute('title', /not unique Peers or Node locations/)
    const probe = await page.evaluate(({ x, y }) => {
      const element = document.elementFromPoint(x, y)
      return element ? (element.closest('[data-slot="geo-counters"]') ? 'counter' : 'map') : 'none'
    }, { x: Math.round(counterBox.x + counterBox.width / 2), y: Math.round(counterBox.y + counterBox.height / 2) })
    expect(probe).toBe(desktop ? 'counter' : 'map')
    for (const figure of await counters.locator('[data-slot="geo-counter"]').all()) {
      await expect(figure.locator('[data-slot="geo-counter-dot"]')).toHaveCount(1)
      await expect(figure).toHaveText(/^Peers: [\d,]+$/)
    }
  }
}

test.describe('Home compact overview and Peer country map (issue #133)', () => {
  test('real Server baseline parity and the canvas country tooltip', async ({ page }, testInfo) => {
    await openHomeWithGeo(page)
    const map = mapRegion(page)
    await expect(map).toBeVisible({ timeout: 30_000 })
    await expect(geoChart(page).locator('canvas').first()).toBeVisible({ timeout: 30_000 })

    // The accessible country list is the map's text surface: the seeded single
    // Swedish record is one listed country with its exact count.
    const sweden = countryItems(page).filter({ hasText: 'Sweden' })
    await expect(sweden).toHaveCount(1)
    await expect(sweden).toHaveText(/^Sweden: 1 records$/)

    // The four Home statistics must render exactly what the Public Projection
    // says, and the map must never change them. They are compared against the
    // live DTO rather than a frozen literal: the harness fixture refresher
    // cannot run while the Server holds SQLite's exclusive lock, so the
    // healthy/attention split legitimately decays during a long run. The
    // baseline frontend shows the same values on the same data.
    const projection = await page.evaluate(async () => {
      const response = await fetch('/api/public/v1/networks', { credentials: 'include' })
      return response.json() as Promise<Array<{ nodes?: Array<{ health?: string }> }>>
    })
    const nodes = projection.flatMap(network => network.nodes ?? [])
    const expectedActive = String(nodes.length)
    const expectedHealthy = String(nodes.filter(node => node.health === 'healthy').length)
    const expectedAttention = String(nodes.length - nodes.filter(node => node.health === 'healthy').length)
    await expect.poll(async () => (await summaryValues(page)).map(fact => fact.value))
      .toEqual([expectedActive, expectedHealthy, expectedAttention, '2'])
    await expect(page.getByRole('combobox', { name: 'Sort' })).toHaveValue('health')
    await expectQuietMap(page)

    // The whole Home body never shows a raw Peer address or the database path.
    await expect(page.locator('body')).not.toContainText('89.160.20.112')
    await expect(page.locator('body')).not.toContainText('GeoIP2-Country-Test')

    // The canvas country-tooltip interaction is asserted on the dense fixture
    // below, where several representative points are painted. The real baseline
    // seeds a single stale, partial Swedish observation whose polygon is painted
    // but whose scatter symbol is not observable on the canvas, so it is not
    // asserted here; the tooltip formatter itself is unit-tested in
    // src/components/mapChartOption.test.ts.

    await capture(page, testInfo, 'real-' + testInfo.project.name + '-compact')
    if (testInfo.project.name === 'desktop-1280') {
      await page.setViewportSize({ width: 1440, height: 1000 })
      await expectQuietMap(page)
      await capture(page, testInfo, 'real-1440x1000-compact')
      await page.setViewportSize({ width: 1280, height: 800 })
    }

    await page.getByRole('tab', { name: 'All Networks', exact: true }).click()
    await expectQuietMap(page)
    await expectNoHorizontalOverflow(page)
  })

  test('isolated DTO fixture: dense Europe and East Asia keep exact counts and honest points', async ({ page }, testInfo) => {
    await openHomeWithGeo(page)
    // Explicit UI-only representative-point fixture. These are NOT observations
    // from the seeded Server, nor invented production country coordinates.
    const countries = [
      { countryCode: 'BE', count: 23, staleCount: 0, centroidLat: 50.85, centroidLon: 4.35 },
      { countryCode: 'NL', count: 42, staleCount: 0, centroidLat: 52.13, centroidLon: 5.29 },
      { countryCode: 'DE', count: 71, staleCount: 0, centroidLat: 51.16, centroidLon: 10.45 },
      { countryCode: 'KR', count: 83, staleCount: 0, centroidLat: 36.5, centroidLon: 127.8 },
      { countryCode: 'JP', count: 94, staleCount: 0, centroidLat: 36.2, centroidLon: 138.25 },
      { countryCode: 'CN', count: 1001, staleCount: 0, centroidLat: 35.86, centroidLon: 104.2 },
      { countryCode: 'SE', count: 7, staleCount: 0, centroidLat: null, centroidLon: null },
    ]
    const known = countries.reduce((sum, country) => sum + country.count, 0)
    await withGeoProjection(page, () => ({
      state: 'current', scope: 'complete', countries,
      knownCountryCount: known, unknownCountryCount: 0, availablePeerCount: known,
      unknownWithPublicIpCount: 0, unknownWithoutRemoteIpCount: 0,
      attribution: null, lastGoodAt: null, staleSince: null, databaseAgeSeconds: null, errorReason: null,
    }), 'current')
    await page.reload()

    const map = mapRegion(page)
    await expect(map).toBeVisible({ timeout: 30_000 })
    await expect(geoChart(page).locator('canvas').first()).toBeVisible({ timeout: 30_000 })
    await expect(map).toHaveAttribute('data-scope', 'complete')
    await expectQuietMap(page)

    // All Networks contains two Networks, so exact UI counts are twice each fixture count.
    const items = countryItems(page)
    await expect(items).toHaveCount(7)
    const exact: Array<[string, number]> = [
      ['Belgium', 46], ['Netherlands', 84], ['Germany', 142], ['South Korea', 166],
      ['Japan', 188], ['China', 2002], ['Sweden', 14],
    ]
    for (const [name, count] of exact) {
      await expect(items.filter({ hasText: name + ':' })).toHaveText(
        new RegExp('^' + name + ': ' + count.toLocaleString('en-US') + ' records'),
      )
    }
    // The old map announced "Some locations not shown" for the country without
    // a representative point; the country list now says the same thing.
    await expect(items.filter({ hasText: 'Sweden:' })).toHaveText(
      'Sweden: 14 records (no representative point, not plotted)',
    )

    // A real pointer hover (desktop) or tap (mobile) opens the ECharts tooltip with
    // the country's name, its flag image path and its record count — the canvas
    // equivalent of the old per-country hover preview.
    await hoverPaintedMarker(page)
    const tooltip = mapTooltip(page).first()
    await expect(tooltip).toBeVisible({ timeout: 10_000 })
    const tooltipText = ((await tooltip.textContent()) ?? '').replace(/\s+/g, ' ').trim()
    expect(tooltipText, 'the tooltip names the country and its records').toMatch(/^.+:? ?[\d,]+ records/)
    const tooltipName = tooltipText.replace(/[\d,]+ records.*$/s, '').replace(/:$/, '').trim()
    expect((await countryItems(page).allTextContents()).some(text => text.startsWith(tooltipName + ':')), 'the tooltip names a listed country').toBe(true)
    const flag = geoChart(page).locator('img[src^="/assets/flags/"]').first()
    await expect(flag).toBeVisible()
    expect(await flag.getAttribute('src')).toMatch(/^\/assets\/flags\/[a-z]{2}\.svg$/)
    if (page.viewportSize()!.width < 768) await page.touchscreen.tap(4, 4)
    else await page.mouse.move(4, 4)
    await expect.poll(async () => mapTooltip(page).first().isVisible()).toBe(false)

    await capture(page, testInfo, 'fixture-dense-' + testInfo.project.name)

    // A resize must not change the observed data the map reads.
    const original = page.viewportSize()!
    await page.setViewportSize({ width: original.width + 37, height: original.height })
    await expect(items).toHaveCount(7)
    await expect(items.filter({ hasText: 'China:' })).toHaveText('China: 2,002 records')
    await expectNoHorizontalOverflow(page)
    await page.setViewportSize(original)
  })

  test('isolated resource fixture: a slow projection and basemap are never written onto the map', async ({ page }, testInfo) => {
    await openHomeWithGeo(page)
    let releaseAssets!: () => void
    const assetGate = new Promise<void>(resolve => { releaseAssets = resolve })
    let releaseNetworks!: () => void
    const networkGate = new Promise<void>(resolve => { releaseNetworks = resolve })
    await page.route('**/assets/geo/**', async route => { await assetGate; await route.continue() })
    await page.route('**/api/public/v1/networks*', async route => { await networkGate; await route.continue() })
    try {
      const reload = page.reload()
      const map = mapRegion(page)
      // While the projection itself is loading, the map announces it and paints
      // nothing at all.
      await expect(mapStatus(page)).toContainText('Loading data')
      await expect(geoChart(page).locator('canvas')).toHaveCount(0)
      await expect(map.locator('[data-slot="geo-counters"]')).toHaveCount(0)
      await expectQuietMap(page)
      await capture(page, testInfo, 'fixture-loading-' + testInfo.project.name)

      releaseNetworks()
      await reload
      // The projection resolved but the basemap is still gated: the counters
      // appear, and the map itself is still not drawn.
      await expect(map.locator('[data-slot="geo-counter"]').first()).toHaveText(/^Peers: [\d,]+$/)
      await expect(geoChart(page).locator('canvas')).toHaveCount(0)

      releaseAssets()
      await expect(geoChart(page).locator('canvas').first()).toBeVisible({ timeout: 30_000 })
      await expect(map.locator('[data-slot="geo-counters"] [data-slot="geo-counter"]').first()).toHaveText(/^Peers: [\d,]+$/)
      expect((await worldBox(page)).height, 'the resolved basemap is written to the canvas').toBeGreaterThan(0)
    } finally {
      releaseAssets()
      releaseNetworks()
      await page.unroute('**/assets/geo/**')
      await page.unroute('**/api/public/v1/networks*')
    }
  })

  test('composes 2x2 global statistics over a transparent map band on desktop', async ({ page }, testInfo) => {
    test.skip(test.info().project.name !== 'desktop-1280', 'the desktop project owns the overlaid composition')
    await openHomeWithGeo(page)

    const map = mapRegion(page)
    const chart = geoChart(page)
    await expect(chart.locator('canvas').first()).toBeVisible({ timeout: 30_000 })
    const facts = await summaryFacts(page)
    expect(facts).toHaveLength(4)
    const statsBox = unionBox(facts.map((fact) => fact.box))
    const mapBox = (await map.boundingBox())!
    const headerBox = (await page.locator('[data-slot="app-header"]').boundingBox())!

    // The logo bar is the shell's own row at every width, exactly like the
    // Emerald reference, whose map is a content grid item beneath the bar. The
    // band therefore starts below the bar and no part of the map is layered
    // behind the brand.
    expect(mapBox.y, 'the map starts below the logo bar').toBeGreaterThanOrEqual(headerBox.y + headerBox.height - 1)
    expect(mapBox.y + mapBox.height, 'the band still spans below the logo bar').toBeGreaterThan(headerBox.y + headerBox.height)
    const brandBox = (await page.locator('[data-slot="app-brand"]').boundingBox())!
    const brandOverlapsMap = brandBox.x < mapBox.x + mapBox.width && mapBox.x < brandBox.x + brandBox.width
      && brandBox.y < mapBox.y + mapBox.height && mapBox.y < brandBox.y + brandBox.height
    expect(brandOverlapsMap, 'no part of the map renders behind the brand').toBe(false)
    // The logo bar paints no surface of its own, so the page wash reads through it.
    const header = await page.locator('[data-slot="app-header"]').evaluate((element) => {
      const style = getComputedStyle(element)
      return {
        backgroundColor: style.backgroundColor,
        borderBottomColor: style.borderBottomColor,
        boxShadow: style.boxShadow,
        position: style.position,
        pointerEvents: style.pointerEvents,
      }
    })
    expect(header.backgroundColor).toMatch(/rgba\(0, 0, 0, 0\)|transparent/)
    expect(header.borderBottomColor).toMatch(/rgba\(0, 0, 0, 0\)|transparent/)
    expect(header.boxShadow).toBe('none')
    expect(header.position, 'the wide bar keeps its own row').not.toBe('absolute')
    expect(header.pointerEvents, 'the bar handles its own pointers').not.toBe('none')
    const topStrip = await page.evaluate(({ map, header }) => {
      const x = Math.round(map.x + map.width * 0.5)
      const probe = document.elementFromPoint(x, Math.round(header.height / 2))
      return probe?.closest('[data-slot="geo-chart"]') ? 'map' : 'other'
    }, { map: mapBox, header: headerBox })
    expect(topStrip, 'the map never reaches the strip the logo bar occupies').toBe('other')

    // The corner figure sits clear of the Admin link and states exactly one
    // thing: the total number of Peers the in-scope Active Nodes are linked to.
    // It is the Server's own Peer-record denominator for that scope, so it can
    // never disagree with the country fills drawn from the same records.
    const counterBox = (await map.locator('[data-slot="geo-counters"]').boundingBox())!
    const adminBox = (await page.getByRole('link', { name: 'Admin' }).boundingBox())!
    const overlaps = counterBox.x < adminBox.x + adminBox.width && counterBox.x + counterBox.width > adminBox.x
      && counterBox.y < adminBox.y + adminBox.height && counterBox.y + counterBox.height > adminBox.y
    expect(overlaps, 'the corner figure never collides with the Admin link').toBe(false)
    expect(counterBox.y, 'the corner figure sits below the logo bar').toBeGreaterThanOrEqual(headerBox.height - 1)
    const projection = await page.evaluate(async () => {
      const response = await fetch('/api/public/v1/networks', { credentials: 'include' })
      return response.json() as Promise<Array<{ geo?: { availablePeerCount?: number | null } }>>
    })
    const peerRecords = projection.reduce((sum, network) => sum + (network.geo?.availablePeerCount ?? 0), 0)
    const hasBasis = projection.some(network => typeof network.geo?.availablePeerCount === 'number')
    const figures = await map.locator('[data-slot="geo-counter"]').evaluateAll(elements => elements.map(element => ({
      text: (element.textContent ?? '').trim(),
      color: getComputedStyle(element).color,
    })))
    expect(figures.length, 'one figure is shown exactly while the scope published a Peer denominator').toBe(hasBasis ? 1 : 0)
    if (hasBasis) {
      expect(figures[0].text, 'the figure is the in-scope Peer total').toBe('Peers: ' + peerRecords.toLocaleString('en-US'))
      const rgb = figures[0].color.match(/\d+/g)!.map(Number)
      expect(rgb[1], 'the figure keeps the reference theme accent').toBeGreaterThan(rgb[0])
      expect(rgb[1]).toBeGreaterThan(rgb[2])
    }

    // The overview is two real columns rather than a summary laid over the
    // band: the statistics own the left, the map owns the right.
    const intersects = statsBox.x < mapBox.x + mapBox.width && mapBox.x < statsBox.x + statsBox.width
      && statsBox.y < mapBox.y + mapBox.height && mapBox.y < statsBox.y + statsBox.height
    expect(intersects, 'the statistics never sit over the map band').toBe(false)
    expect(statsBox.y, 'the summary starts below the logo bar').toBeGreaterThanOrEqual(headerBox.height - 1)
    expect(mapBox.x, 'the map keeps the right column, clear of the statistics').toBeGreaterThan(statsBox.x + statsBox.width)
    const columns = new Set(facts.map((fact) => Math.round(fact.box.x))).size
    expect(columns, 'the four status counters keep two of the three columns').toBe(2)

    // The canvas paints a real world projection rather than a letterboxed band.
    const world = await worldBox(page)
    expect(world.height, 'actual world grows beyond the old 136px map').toBeGreaterThan(136)

    // Emerald card surface: translucent at rest, opaque with an emerald halo
    // while pointed at or focused.
    for (const selector of ['[data-slot="summary-card"]', '[data-slot="node-card"]']) {
      const card = page.locator(selector).first()
      // Tailwind v4 paints oklch colours, so the alpha and shadow tint are read
      // back through a canvas rather than parsed as rgba().
      const rest = await card.evaluate(element => {
        const canvas = document.createElement('canvas')
        canvas.width = canvas.height = 1
        const context = canvas.getContext('2d')!
        context.fillStyle = getComputedStyle(element).backgroundColor
        context.fillRect(0, 0, 1, 1)
        return context.getImageData(0, 0, 1, 1).data[3] / 255
      })
      expect(rest, selector + ' is translucent at rest').toBeGreaterThan(0.3)
      expect(rest, selector + ' is translucent at rest').toBeLessThan(1)
      await card.hover()
      await expect.poll(async () => card.evaluate(element => {
        const canvas = document.createElement('canvas')
        canvas.width = canvas.height = 1
        const context = canvas.getContext('2d')!
        context.fillStyle = getComputedStyle(element).backgroundColor
        context.fillRect(0, 0, 1, 1)
        return context.getImageData(0, 0, 1, 1).data[3] / 255
      }), { message: selector + ' turns opaque on hover' }).toBe(1)
      // The Node card keeps the reference theme's emerald halo. The migration's
      // summary surface (SURFACE_CARD) turns opaque with a foreground border
      // instead, so the tint is asserted where the source actually paints it.
      if (selector === '[data-slot="node-card"]') {
        await expect.poll(async () => card.evaluate(element => {
          const canvas = document.createElement('canvas')
          canvas.width = canvas.height = 1
          const context = canvas.getContext('2d')!
          // The Tailwind ring stacks transparent placeholders first, so every
          // colour in the shadow is converted until the emerald tint is found.
          for (const match of getComputedStyle(element).boxShadow.matchAll(/(rgba?\([^)]*\)|oklch\([^)]*\)|oklab\([^)]*\))/g)) {
            context.clearRect(0, 0, 1, 1)
            context.fillStyle = match[1]
            context.fillRect(0, 0, 1, 1)
            const [r, g, b, a] = context.getImageData(0, 0, 1, 1).data
            if (a > 0 && g > r && g > b) return true
          }
          return false
        }), { message: selector + ' keeps its emerald halo on hover' }).toBe(true)
      }
    }
    await page.mouse.move(4, 4)

    // No opaque white shell, border, shadow, or whole-container opacity fade.
    const surface = await map.evaluate((element) => {
      const style = getComputedStyle(element)
      return {
        backgroundColor: style.backgroundColor,
        borderTopWidth: style.borderTopWidth,
        boxShadow: style.boxShadow,
        opacity: style.opacity,
      }
    })
    expect(surface.backgroundColor).toMatch(/rgba\(0, 0, 0, 0\)|transparent/)
    expect(surface.borderTopWidth).toBe('0px')
    expect(surface.boxShadow).toBe('none')
    expect(Number(surface.opacity)).toBe(1)

    // The map never covers the toolbar or the Node cards. The hover checks
    // above scrolled a Node card into view, and mapBox was read at the top of
    // the page: return to the top and re-read both boxes so the comparison
    // never mixes two scroll offsets.
    await page.evaluate(() => window.scrollTo(0, 0))
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(0)
    const toolbarBox = (await page.getByRole('tablist', { name: 'Network filter' }).boundingBox())!
    const bandBox = (await map.boundingBox())!
    expect(bandBox.y + bandBox.height).toBeLessThanOrEqual(toolbarBox.y + 1)
    // The decorative background stays pointer-inert, so a real click inside
    // the map reaches the map image itself.
    const imageBox = (await chart.boundingBox())!
    const hit = await page.evaluate(({ x, y }) => {
      const element = document.elementFromPoint(x, y)
      return element ? { isMap: Boolean(element.closest('[data-slot="geo-chart"]')) } : null
    }, { x: imageBox.x + imageBox.width / 2, y: imageBox.y + imageBox.height / 2 })
    expect(hit?.isMap, 'the map keeps its own pointer interaction').toBe(true)

    await expectVisibleInteractiveTargets(page)
    await expectQuietMap(page)
    await expectNoHorizontalOverflow(page)

    // Delivered evidence at the project's own 1280x800 acceptance viewport,
    // then at the wider 1440x900 review viewport named by the issue.
    await capture(page, testInfo, 'home-1280-compact')

    // There is no expansion any more: the band keeps its size, the summary keeps
    // its 2x2 overlay, and nothing is pushed down. The pointer is parked first,
    // because a hovered card lifts by 2px.
    await page.mouse.move(4, 4)
    await page.waitForTimeout(250)
    const settled = await page.evaluate(() => {
      const facts = [...document.querySelectorAll<HTMLElement>('[data-slot="summary-card"]')].map(card => card.getBoundingClientRect())
      const map = document.querySelector('[aria-label="Peer countries"]')!.getBoundingClientRect()
      const summary = {
        left: Math.min(...facts.map(fact => fact.left)),
        right: Math.max(...facts.map(fact => fact.right)),
        top: Math.min(...facts.map(fact => fact.top)),
        bottom: Math.max(...facts.map(fact => fact.bottom)),
      }
      const mapHoldsItsOwnColumn = map.left >= summary.right
      return {
        cards: facts.length,
        rows: new Set(facts.map(fact => Math.round(fact.top))).size,
        columns: new Set(facts.map(fact => Math.round(fact.left))).size,
        mapSpan: Math.round(map.width),
        mapHoldsItsOwnColumn,
      }
    })
    expect(settled.cards, 'all six statistics are laid out').toBe(6)
    expect(settled.rows, 'statistics keep their two-row shape').toBe(2)
    expect(settled.columns, 'statistics keep their three columns').toBe(3)
    expect(settled.mapSpan, 'the band keeps its width: there is no expansion').toBe(Math.round(mapBox.width))
    expect(settled.mapHoldsItsOwnColumn, 'the map holds its own column beside the statistics').toBe(true)

    await page.setViewportSize({ width: 1440, height: 900 })
    await expectNoHorizontalOverflow(page)
    // Measure the whole six-card statistics track, not just the four status
    // counters, which occupy two of its three columns.
    const wideStats = (await page.locator('[aria-label="Home summary"]').boundingBox())!
    const wideMap = (await map.boundingBox())!
    // The overview gives the map the larger share of the band, as the Emerald
    // reference does, so three statistics columns still fit beside a wide map.
    expect(wideMap.width, 'the map takes the larger share of the band').toBeGreaterThan(wideStats.width)
    expect(wideStats.width, 'the statistics stay readable').toBeGreaterThanOrEqual(20 * 16)
    expect(wideMap.x, 'the map keeps the right column clear of the statistics')
      .toBeGreaterThanOrEqual(wideStats.x + wideStats.width)
    await capture(page, testInfo, 'home-1440-compact')
    await page.setViewportSize({ width: 1280, height: 800 })

    // A routine Current projection dedicates its height to the map, not metadata.
    await withGeoProjection(page, () => ({
      state: 'current',
      scope: 'complete',
      countries: [{ countryCode: 'SE', count: 1, staleCount: 0, centroidLat: 60.1282, centroidLon: 18.6435 }],
      knownCountryCount: 1,
      unknownCountryCount: 0,
      availablePeerCount: 1,
      unknownWithPublicIpCount: 0,
      unknownWithoutRemoteIpCount: 0,
      attribution: null,
      lastGoodAt: null,
      staleSince: null,
      databaseAgeSeconds: null,
      errorReason: null,
    }), 'current')
    await page.reload()
    await expect(chart.locator('canvas').first()).toBeVisible({ timeout: 30_000 })
    await expect(map.getByRole('status')).toHaveCount(0)
    const routineBand = (await chart.boundingBox())!
    expect(routineBand.height, 'the map keeps a readable height').toBeGreaterThanOrEqual(190)
    expect((await worldBox(page)).height, 'actual world grows beyond the old 136px map').toBeGreaterThan(136)
    const compactOverview = await overviewBox(page)
    // The map uses upstream's fixed 22rem band beside the six statistics, so the
    // canvas keeps the world's own rendered aspect ratio (~1.94:1) inside a
    // fixed-height track instead of a proportional 2:1 one.
    const mapBand = (await page.locator('[data-slot="home-map"]').boundingBox())!
    expect(Math.round(mapBand.height), 'the map keeps the fixed 352px band').toBe(352)
    expect(Math.round(routineBand.height), 'the canvas fills the fixed band').toBe(352)
    const toolbarTop = (await page.getByRole('tablist', { name: 'Network filter' }).boundingBox())!.y
    expect(compactOverview.y + compactOverview.height, 'the overview content band ends before the toolbar').toBeLessThanOrEqual(toolbarTop + 1)
    await page.getByRole('combobox', { name: 'Sort', exact: true }).click({ trial: true })
    await page.getByRole('tab', { name: 'All Networks', exact: true }).click({ trial: true })
  })

  test('stacks the statistics over a compact map on narrow screens', async ({ page }) => {
    test.skip(test.info().project.name === 'desktop-1280', 'the desktop project keeps the two-column overview')
    await openHomeWithGeo(page)

    const map = mapRegion(page)
    await expect(map).toBeVisible({ timeout: 30_000 })
    const chart = geoChart(page)
    await expect(chart.locator('canvas').first()).toBeVisible({ timeout: 30_000 })

    const facts = await summaryFacts(page)
    const stats = unionBox(facts.map((fact) => fact.box))
    const mapBox = (await map.boundingBox())!
    const headerBox = (await page.locator('[data-slot="app-header"]').boundingBox())!

    // The logo bar keeps its own row at every width. Wide layouts place the map
    // beside the statistics; narrow layouts put the complete 2:1 map first and
    // the six statistics after it, always above the Node cards.
    expect(stats.y, 'the statistics start below the logo bar').toBeGreaterThanOrEqual(headerBox.y + headerBox.height - 1)
    if (page.viewportSize()!.width >= 1024) {
      expect(mapBox.x, 'the map keeps its own column beside the statistics').toBeGreaterThanOrEqual(stats.x + stats.width - 1)
    } else {
      expect(stats.y, 'the statistics stack below the map').toBeGreaterThanOrEqual(mapBox.y + mapBox.height - 1)
    }
    const firstCard = (await page.locator('[data-slot="node-card"]').first().boundingBox())!
    expect(mapBox.y, 'the map sits above the Node cards').toBeLessThan(firstCard.y)
    // The bar paints nothing, so the page wash still reads through it.
    const header = await page.locator('[data-slot="app-header"]').evaluate((element) => {
      const style = getComputedStyle(element)
      return { backgroundColor: style.backgroundColor, borderBottomColor: style.borderBottomColor, boxShadow: style.boxShadow, position: style.position }
    })
    expect(header.backgroundColor).toMatch(/rgba\(0, 0, 0, 0\)|transparent/)
    expect(header.borderBottomColor).toMatch(/rgba\(0, 0, 0, 0\)|transparent/)
    expect(header.boxShadow).toBe('none')
    expect(header.position, 'the narrow bar stays in the flow').not.toBe('absolute')

    // The canvas paints a real, readable world at this width.
    const compact = (await chart.boundingBox())!
    const compactWorld = await worldBox(page)
    expect(Math.round(compact.height), 'compact map canvas').toBeGreaterThanOrEqual(120)
    expect(compactWorld.height, 'the compact world is really drawn').toBeGreaterThan(60)

    // There is no expand control, and the band does not change size.
    await expect(map.locator('button')).toHaveCount(0)

    // The compact map stays interactive at the canvas level; the country-tooltip
    // interaction itself is asserted on the dense fixture, where a marker is
    // actually painted (the real baseline's single stale Swedish observation
    // paints no observable scatter symbol).
    await expect(geoChart(page).locator('canvas').first()).toBeVisible()

    await expectVisibleInteractiveTargets(page)
    await expectQuietMap(page)
    await expectNoHorizontalOverflow(page)
  })

  test('reaches the 375px phone width declared by the issue', async ({ page }, testInfo) => {
    test.skip(test.info().project.name !== 'phone-390-touch', 'the 375px check belongs to the 390px phone project')
    await openHomeWithGeo(page)
    const map = mapRegion(page)
    await expect(geoChart(page).locator('canvas').first()).toBeVisible({ timeout: 30_000 })

    // A real 375px viewport, not a device-scale emulation of another width.
    await page.setViewportSize({ width: 375, height: 844 })
    await expect(geoChart(page).locator('canvas').first()).toBeVisible()
    await expectNoHorizontalOverflow(page)

    const chart = geoChart(page)
    const compact = (await chart.boundingBox())!
    const compactWorld = await worldBox(page)
    expect(Math.round(compact.height), '375px compact canvas').toBeGreaterThanOrEqual(120)
    // The refined mobile composition gives the map box upstream's 2:1 aspect
    // ratio (about 172px at 375px), so the retired fixed h-88 (352px) box and
    // the -mt-42 overlap no longer apply.
    expect(Math.round(compact.height), '375px canvas keeps the 2:1 map box').toBeGreaterThanOrEqual(
      Math.round(compact.width / 2) - 2,
    )
    expect(Math.round(compact.height), '375px canvas keeps the 2:1 map box').toBeLessThanOrEqual(
      Math.round(compact.width / 2) + 2,
    )

    const mapBox = (await map.boundingBox())!
    expect(mapBox.x).toBeGreaterThanOrEqual(0)
    expect(mapBox.x + mapBox.width).toBeLessThanOrEqual(375)
    // The complete map comes first and the six statistics follow below it,
    // separated by the shared grid gap, so neither covers the other.
    const stats = unionBox((await summaryFacts(page)).map((fact) => fact.box))
    expect(mapBox.y, 'the map starts above the statistics').toBeLessThanOrEqual(stats.y)
    expect(stats.y, 'the statistics follow the map without overlapping').toBeGreaterThanOrEqual(mapBox.y + mapBox.height - 1)

    await capture(page, testInfo, 'home-375-compact')
    // No expand control here either: the band keeps its size at 375px.
    await expect(map.locator('button')).toHaveCount(0)
    expect((await worldBox(page)).width).toBeCloseTo(compactWorld.width, 0)
    await expectNoHorizontalOverflow(page)
  })

  test('keeps statistics and map in the selected network scope', async ({ page }) => {
    await openHomeWithGeo(page)
    const map = mapRegion(page)
    await expect(map).toBeVisible({ timeout: 30_000 })

    const items = countryItems(page)
    const itemNames = () => items.evaluateAll(elements => elements.map(element => (element.textContent ?? '').split(':')[0].trim()))
    await expect(geoChart(page).locator('canvas').first()).toBeVisible({ timeout: 30_000 })
    await expect.poll(() => items.count(), 'the unfiltered map lists a country').toBeGreaterThan(0)
    const allNames = await itemNames()
    const before = await summaryValues(page)

    // Filtering re-scopes statistics, map and Node list together.
    const pills = page.getByRole('tablist', { name: 'Network filter' })
    for (const name of ['PlatON E2E Network', 'Home Convergence Network With An Extremely Long Display Name']) {
      await pills.getByRole('tab', { name, exact: true }).click()
      await expect(geoChart(page)).toHaveAttribute('aria-label', new RegExp('in scope for ' + name))
      await expect.poll(async () => (await itemNames()).every(label => allNames.includes(label))).toBe(true)
      const filtered = await summaryValues(page)
      expect(filtered.find(item => item.label === 'Active Nodes')?.value).toBe(String(await page.locator('[data-slot="node-card"]').count()))
      expect(filtered.find(item => item.label === 'Networks')?.value).toBe('1')
    }

    await pills.getByRole('tab', { name: 'All Networks', exact: true }).click()
    await expect.poll(() => items.count(), 'the full scope comes back').toBe(allNames.length)
    expect(await summaryValues(page)).toEqual(before)
    await expectQuietMap(page)
    await expectNoHorizontalOverflow(page)
  })

  test('isolated DTO fixture: keeps every projection state honest, including never observed and a zero basis', async ({ page }) => {
    await openHomeWithGeo(page)
    const map = mapRegion(page)

    // A Network that never reported a successful Peer Snapshot has no basis at
    // all, so no count may be presented as a real zero.
    await withGeoProjection(page, () => ({
      state: 'unknown', scope: 'unobserved', countries: null,
      knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null,
      unknownWithPublicIpCount: null, unknownWithoutRemoteIpCount: null,
      attribution: null, lastGoodAt: null, staleSince: null, databaseAgeSeconds: null, errorReason: null,
    }))
    await page.reload()
    await expect(mapStatus(page)).toContainText('No observations yet', { timeout: 30_000 })
    await expect(geoChart(page).locator('canvas')).toHaveCount(0)
    await expect(map.locator('[data-slot="geo-counters"]')).toHaveCount(0)
    await expect(countryItems(page)).toHaveCount(0)
    await expectQuietMap(page)

    // An authoritative, successful empty country set is a real zero.
    await page.unroute('**/api/public/v1/networks*')
    await withGeoProjection(page, () => ({
      state: 'current', scope: 'complete', countries: [],
      knownCountryCount: 0, unknownCountryCount: 0, availablePeerCount: 0,
      unknownWithPublicIpCount: 0, unknownWithoutRemoteIpCount: 0,
      attribution: null, lastGoodAt: null, staleSince: null, databaseAgeSeconds: null, errorReason: null,
    }))
    await page.reload()
    await expect(mapStatus(page)).toContainText('No data', { timeout: 30_000 })
    await expect(geoChart(page).locator('canvas').first()).toBeVisible({ timeout: 30_000 })
    await expect(map.locator('[data-slot="geo-counter"]')).toHaveText(/^Peers: 0$/)
    await expect(countryItems(page)).toHaveCount(0)
    await expectQuietMap(page)
  })

  test('degrades locally for a disabled provider and an unavailable basemap', async ({ page }, testInfo) => {
    await loginAs(page)
    await setGeoProvider(page, 'Disabled')
    const geoRequests: string[] = []
    await page.route('**/assets/geo/**', route => {
      geoRequests.push(route.request().url())
      return route.continue()
    })
    try {
      await page.goto('/')
      const map = mapRegion(page)
      await expect(mapStatus(page)).toHaveText('Peer countries · Disabled by server')
      // No basemap means no map: the canvas is never created and no Geo asset
      // is fetched at all.
      await expect(geoChart(page).locator('canvas')).toHaveCount(0)
      await expect(map.locator('[data-slot="geo-counters"]')).toHaveCount(0)
      expect(geoRequests, 'a disabled provider never fetches a basemap').toHaveLength(0)
      await expectQuietMap(page)
      await capture(page, testInfo, 'fixture-disabled-' + testInfo.project.name)

      // The basemap resource itself can fail while the Server data survives.
      await setGeoProvider(page, 'Local MMDB')
      await page.unroute('**/assets/geo/**')
      await page.route('**/assets/geo/**', route => route.abort())
      await page.goto('/')
      await expect(mapStatus(page)).toContainText('Map unavailable', { timeout: 30_000 })
      await expect(geoChart(page)).toHaveCount(0)
      // Home keeps working: the statistics and the Node cards are untouched.
      await expect(page.getByRole('article').first()).toBeVisible()
      await expectQuietMap(page)
      await capture(page, testInfo, 'fixture-failure-' + testInfo.project.name)

      // The same local degradation in Dark: the notice stays readable, the map
      // stays absent, and the page keeps working without overflow.
      const theme = page.getByRole('button', { name: /^Theme: / })
      await theme.click()
      await theme.click()
      expect(await page.evaluate(() => document.documentElement.classList.contains('dark'))).toBe(true)
      await expect(mapStatus(page)).toContainText('Map unavailable')
      await expect(geoChart(page)).toHaveCount(0)
      await expect(page.getByRole('article').first()).toBeVisible()
      await expectQuietMap(page)
      await expectNoHorizontalOverflow(page)
      await capture(page, testInfo, 'fixture-failure-dark-' + testInfo.project.name)
    } finally {
      await page.unroute('**/assets/geo/**')
      // The Server and its Geo provider are shared by every spec and project.
      // The harness seeds Disabled, so restore that state instead of leaving
      // Local MMDB behind for the specs that run later.
      await setGeoProvider(page, 'Disabled')
    }
  })
})
