import { mkdirSync } from 'node:fs'
import { join } from 'node:path'
import { expect, test, type Page, type TestInfo } from '@playwright/test'
import {
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
  loginAs,
} from './helpers'

/**
 * Home compact overview and Peer country map (issue #133).
 *
 * The suite runs against the real Server, the operator-provided MMDB, and the
 * production WebUI build; only the Peer fixture is seeded. Assertions cross
 * the routed Public Home seam: accessible roles and names, visible text, and
 * externally observable geometry. They never depend on component CSS classes
 * or private state. Scenarios normalize the Geo Provider; the disabled-provider
 * scenario restores Local MMDB. DTO overrides below are isolated UI fixtures,
 * not claims about the real Server's Peer observations.
 *
 * The map shows the world, its country fills, the quantity markers, and one
 * expand control. Its abnormal states are announced only to assistive
 * technology, so those assertions read the screen-reader status text.
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
 * written to the gitignored `playwright-report/emerald/` directory, so a
 * local run leaves browsable evidence instead of discarding it with the
 * per-test output directory.
 */
async function capture(page: Page, testInfo: TestInfo, name: string) {
  const body = await page.screenshot()
  await testInfo.attach(name, { body, contentType: 'image/png' })
  const directory = join('playwright-report', 'emerald')
  mkdirSync(directory, { recursive: true })
  await page.screenshot({ path: join(directory, 'after-' + name + '.png') })
}

/** Home's four global statistics, read from their own article roles so the
 *  map can never change or reorder them. */
async function summaryFacts(page: Page): Promise<Array<{ label: string; value: string; box: Box }>> {
  return page.getByRole('article').evaluateAll((elements) =>
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

/** Rendered country geometry, not just the SVG viewport: expansion must scale
 * the actual world rather than add letterboxed blank height. */
async function worldBox(page: Page): Promise<Box> {
  const boxes = await page.getByRole('img', { name: 'Peer countries map' }).locator('path').evaluateAll(paths => paths.map(path => {
    const rect = path.getBoundingClientRect()
    return { x: rect.x, y: rect.y, width: rect.width, height: rect.height }
  }))
  expect(boxes.length).toBeGreaterThan(100)
  return unionBox(boxes)
}

async function overviewBox(page: Page): Promise<Box> {
  const stats = unionBox((await summaryFacts(page)).map((fact) => fact.box))
  const mapBox = (await page.getByRole('region', { name: 'Peer countries' }).boundingBox())!
  return {
    x: Math.min(stats.x, mapBox.x),
    y: Math.min(stats.y, mapBox.y),
    width: Math.max(stats.x + stats.width, mapBox.x + mapBox.width) - Math.min(stats.x, mapBox.x),
    height: Math.max(stats.y + stats.height, mapBox.y + mapBox.height) - Math.min(stats.y, mapBox.y),
  }
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

/** The map is bare: one expand control, no written status, no dialog. */
async function expectQuietMap(page: Page) {
  const map = page.getByRole('region', { name: 'Peer countries' })
  await expect(map.getByRole('heading')).toHaveCount(0)
  await expect(map.getByRole('dialog')).toHaveCount(0)
  await expect(page.getByRole('dialog', { name: 'Map information' })).toHaveCount(0)
  await expect(map.getByText(/Natural Earth|GeoLite|MaxMind|IPinfo|GeoJS/)).toHaveCount(0)
  await expect(map.getByText(/^Scope: |^Known |^Peer observation: |^Map resource: /)).toHaveCount(0)

  // Nothing on the map is written for the eye; the status text stays clipped.
  for (const status of await map.getByRole('status').all()) {
    const presentation = await status.evaluate(element => {
      const box = element.getBoundingClientRect()
      const style = getComputedStyle(element)
      return { width: box.width, height: box.height, clip: style.clip, clipPath: style.clipPath }
    })
    expect(presentation.width).toBeLessThanOrEqual(1)
    expect(presentation.height).toBeLessThanOrEqual(1)
    expect(presentation.clip !== 'auto' || presentation.clipPath !== 'none').toBe(true)
  }

  const buttons = await map.locator('button').all()
  expect(buttons, 'the map keeps exactly one control').toHaveLength(1)
  for (const button of buttons) {
    const box = (await button.boundingBox())!
    expect(box.width).toBeGreaterThanOrEqual(44)
    expect(box.height).toBeGreaterThanOrEqual(44)
    const icon = button.locator('svg')
    await expect(icon).toHaveCount(1)
    const iconBox = (await icon.boundingBox())!
    expect(iconBox.width).toBeCloseTo(16, 0)
    expect(iconBox.height).toBeCloseTo(16, 0)
    expect((await button.textContent())?.trim()).toBe('')
  }
}

test.describe('Home compact overview and Peer country map (issue #133)', () => {
  test('real Server baseline parity and single-dot interactions', async ({ page }, testInfo) => {
    await openHomeWithGeo(page)
    const map = page.getByRole('region', { name: 'Peer countries' })
    const marker = map.getByRole('button', { name: 'Sweden · 1 records', exact: true })
    await expect(marker).toBeVisible({ timeout: 30_000 })

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

    // One record is a bare dot: no numeral, and no white count disc.
    await expect(marker.locator('text')).toHaveCount(0)
    const controls = await marker.locator('circle').evaluateAll(circles => circles.map(circle => {
      const box = circle.getBoundingClientRect()
      return { width: box.width, height: box.height, fill: getComputedStyle(circle).fill }
    }))
    expect(controls).toHaveLength(2)
    expect(controls[0].width).toBeCloseTo(24, 0)
    expect(controls[1].width).toBeCloseTo(7, 0)
    const rgb = controls[1].fill.match(/\d+/g)!.map(Number)
    expect(rgb[1], 'filled Emerald marker').toBeGreaterThan(rgb[0])
    expect(rgb[1]).toBeGreaterThan(rgb[2])

    // The whole Home body never shows a raw Peer address or the database path.
    await expect(page.locator('body')).not.toContainText('89.160.20.112')
    await expect(page.locator('body')).not.toContainText('GeoIP2-Country-Test')

    await capture(page, testInfo, 'real-' + testInfo.project.name + '-compact')
    if (testInfo.project.name === 'desktop-1280') {
      await page.setViewportSize({ width: 1440, height: 1000 })
      await expectQuietMap(page)
      await capture(page, testInfo, 'real-1440x1000-compact')
      await page.setViewportSize({ width: 1280, height: 800 })
    }

    await marker.focus()
    await marker.press('Enter')
    await expect(map.getByRole('tooltip')).toHaveText('Sweden · 1 records')
    await marker.press('Escape')
    await expect(map.getByRole('tooltip')).toHaveCount(0)
    await marker.press('Space')
    await expect(map.getByRole('tooltip')).toBeVisible()
    await marker.press('Escape')
    await expect(map.getByRole('tooltip')).toHaveCount(0)
    if (testInfo.project.use.hasTouch) await marker.tap()
    else await marker.hover()
    await expect(map.getByRole('tooltip')).toHaveText('Sweden · 1 records')
    await page.getByRole('button', { name: 'All Networks', exact: true }).click()
    await expect(map.getByRole('tooltip')).toHaveCount(0)
    await expectQuietMap(page)
    await expectNoHorizontalOverflow(page)
  })

  test('isolated DTO fixture: dense Europe and East Asia keep exact counts and fixed points', async ({ page }, testInfo) => {
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

    const map = page.getByRole('region', { name: 'Peer countries' })
    const markers = map.locator('g[role="button"]')
    await expect(markers).toHaveCount(6)
    await expectQuietMap(page)
    await expect(map.getByRole('status')).toHaveText('Some locations not shown')

    // All Networks contains two Networks, so exact UI counts are twice each fixture count.
    const belgium = map.getByRole('button', { name: 'Belgium · 46 records', exact: true })
    const china = map.getByRole('button', { name: 'China · 2,002 records', exact: true })
    await expect(belgium.locator('text')).toHaveCount(0)

    const positions = () => markers.evaluateAll(elements => elements.map(element => ({
      name: element.getAttribute('aria-label'), point: element.getAttribute('transform')?.match(/translate\([^)]+\)/)?.[0],
    })))
    const before = await positions()
    for (const marker of await markers.all()) {
      const circles = await marker.locator('circle').evaluateAll(elements => elements.map(element => element.getBoundingClientRect().width))
      expect(circles[0]).toBeCloseTo(24, 0)
      expect(circles[1]).toBeGreaterThanOrEqual(6.9)
      expect(circles[1]).toBeLessThanOrEqual(22.1)
      // A rendered numeral must fit inside its own disc. Real layout is required,
      // so this is measured here rather than in jsdom.
      const label = marker.locator('text')
      if (await label.count() > 0) {
        const textWidth = (await label.boundingBox())!.width
        const numeral = await label.textContent()
        expect(textWidth, 'numeral fits its disc: ' + numeral).toBeLessThanOrEqual(circles[1] - 1)
      }
      await marker.focus()
      await marker.press('Enter')
      await expect(map.getByRole('tooltip')).toHaveText((await marker.getAttribute('aria-label'))!)
      await marker.press('Escape')
    }

    await capture(page, testInfo, 'fixture-dense-' + testInfo.project.name)

    // A resize must not move a real representative point.
    await china.focus()
    await china.press('Enter')
    const original = page.viewportSize()!
    await page.setViewportSize({ width: original.width + 37, height: original.height })
    await expect(map.getByRole('tooltip')).toHaveCount(0)
    expect(await positions()).toEqual(before)
    await china.press('Enter')
    await page.getByRole('button', { name: 'Show full map' }).click()
    await expect(map.getByRole('tooltip')).toHaveCount(0)
    expect(await positions()).toEqual(before)
    await expectNoHorizontalOverflow(page)
    await page.getByRole('button', { name: 'Collapse map' }).click()
    await page.setViewportSize(original)
  })

  test('isolated resource fixture: a slow basemap is announced but never written on the map', async ({ page }, testInfo) => {
    await openHomeWithGeo(page)
    let release!: () => void
    const gate = new Promise<void>(resolve => { release = resolve })
    await page.route('**/assets/geo/**', async route => { await gate; await route.continue() })
    try {
      await page.reload()
      const map = page.getByRole('region', { name: 'Peer countries' })
      await expect(map.getByRole('button', { name: 'Show full map' })).toBeDisabled()
      await expect(map.getByRole('status')).toContainText('Loading map')
      await expectQuietMap(page)
      await capture(page, testInfo, 'fixture-loading-' + testInfo.project.name)
      release()
      await expect(map.getByRole('img', { name: 'Peer countries map' })).toBeVisible({ timeout: 30_000 })
      await expect(map.getByRole('button', { name: 'Show full map' })).toBeEnabled()
    } finally {
      release()
      await page.unroute('**/assets/geo/**')
    }
  })

  test('composes 2x2 global statistics beside a transparent country map on desktop', async ({ page }, testInfo) => {
    test.skip(test.info().project.name !== 'desktop-1280', 'the desktop project owns the two-column overview')
    await openHomeWithGeo(page)

    const map = page.getByRole('region', { name: 'Peer countries' })
    await expect(map.getByRole('img', { name: 'Peer countries map' })).toBeVisible({ timeout: 30_000 })
    const facts = await summaryFacts(page)
    expect(facts).toHaveLength(4)
    const statsBox = unionBox(facts.map((fact) => fact.box))
    const mapBox = (await map.boundingBox())!

    // Two columns: the statistics sit left of the map and share its top edge.
    expect(mapBox.x).toBeGreaterThanOrEqual(statsBox.x + statsBox.width - 1)
    expect(Math.abs(mapBox.y - statsBox.y)).toBeLessThanOrEqual(2)
    const columns = new Set(facts.map((fact) => Math.round(fact.box.x))).size
    expect(columns, 'the four statistics form a 2x2 grid').toBe(2)

    // No clipped outline may be painted: Natural Earth's Antarctica ring
    // collapses onto the bottom edge, and drawing it puts a stray full-width
    // rule under the map that reads as a border.
    const flatOutlines = await map.getByRole('img', { name: 'Peer countries map' }).locator('path').evaluateAll(paths => paths
      .map(path => { const box = path.getBBox(); return { height: box.height } })
      .filter(box => box.height < 0.5))
    expect(flatOutlines, 'no zero-height country outline is painted').toHaveLength(0)

    // Emerald card surface: translucent at rest, opaque with an emerald halo
    // while pointed at or focused.
    for (const selector of ['.dashboard-summary-card', '.dashboard-node-card']) {
      const card = page.locator(selector).first()
      const rest = await card.evaluate(element => {
        const style = getComputedStyle(element)
        const alpha = style.backgroundColor.match(/rgba?\(([^)]+)\)/)
        const parts = alpha ? alpha[1].split(',').map(part => part.trim()) : []
        return { alpha: parts.length === 4 ? Number(parts[3]) : 1 }
      })
      expect(rest.alpha, selector + ' is translucent at rest').toBeGreaterThan(0.3)
      expect(rest.alpha, selector + ' is translucent at rest').toBeLessThan(1)
      await card.hover()
      await expect.poll(async () => card.evaluate(element => {
        const style = getComputedStyle(element)
        const alpha = style.backgroundColor.match(/rgba?\(([^)]+)\)/)
        const parts = alpha ? alpha[1].split(',').map(part => part.trim()) : []
        return (parts.length === 4 ? Number(parts[3]) : 1) === 1 && style.boxShadow.includes('rgba(5, 150, 105')
      }), { message: selector + ' lights up on hover' }).toBe(true)
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

    // The map never covers the toolbar or the Node cards.
    const toolbarBox = (await page.getByRole('group', { name: 'Network filter' }).boundingBox())!
    expect(mapBox.y + mapBox.height).toBeLessThanOrEqual(toolbarBox.y + 1)
    // The decorative background stays pointer-inert, so a real click inside
    // the map reaches the map image itself.
    const imageBox = (await page.getByRole('img', { name: 'Peer countries map' }).boundingBox())!
    const hit = await page.evaluate(({ x, y }) => {
      const element = document.elementFromPoint(x, y)
      return element ? { isMap: Boolean(element.closest('svg[role="img"]')) } : null
    }, { x: imageBox.x + imageBox.width / 2, y: imageBox.y + imageBox.height / 2 })
    expect(hit?.isMap, 'the map keeps its own pointer interaction').toBe(true)

    await expectVisibleInteractiveTargets(page)
    await expectQuietMap(page)
    await expectNoHorizontalOverflow(page)

    // Delivered evidence at the project's own 1280x800 acceptance viewport,
    // then at the wider 1440x900 review viewport named by the issue.
    await capture(page, testInfo, 'home-1280-compact')
    const desktopWorld = await worldBox(page)
    await page.getByRole('button', { name: 'Show full map' }).click()
    await expect.poll(async () => (await worldBox(page)).width).toBeGreaterThan(desktopWorld.width)
    await expectNoHorizontalOverflow(page)

    // Expanding must not orphan half a row: the four statistics become one
    // full-width row, and the map takes the whole row beneath them. The pointer
    // is parked first, because a hovered card lifts by 2px.
    await page.mouse.move(4, 4)
    await page.waitForTimeout(250)
    const expandedLayout = await page.evaluate(() => {
      const stats = document.querySelector('.home-overview-stats')!
      const cards = [...document.querySelectorAll<HTMLElement>('.dashboard-summary-card')]
      const map = document.querySelector('.home-geo')!
      const boxes = cards.map(card => card.getBoundingClientRect())
      const firstTop = boxes[0].top
      return {
        inFirstRow: boxes.filter(box => Math.abs(box.top - firstTop) <= 4).length,
        columns: new Set(boxes.map(box => Math.round(box.left))).size,
        cards: boxes.length,
        statsSpan: Math.round(stats.getBoundingClientRect().width),
        mapSpan: Math.round(map.getBoundingClientRect().width),
        mapTop: Math.round(map.getBoundingClientRect().top),
        lowestCard: Math.round(Math.max(...boxes.map(box => box.bottom))),
      }
    })
    expect(expandedLayout.cards, 'all four statistics are laid out').toBe(4)
    expect(expandedLayout.inFirstRow, 'statistics sit in one row while expanded').toBe(4)
    expect(expandedLayout.columns, 'each statistic keeps its own column').toBe(4)
    expect(expandedLayout.statsSpan, 'statistics fill the row').toBe(expandedLayout.mapSpan)
    expect(expandedLayout.mapTop, 'the map follows the statistics row').toBeGreaterThanOrEqual(expandedLayout.lowestCard)
    await capture(page, testInfo, 'home-1280-expanded')
    await page.getByRole('button', { name: 'Collapse map' }).click()

    await page.setViewportSize({ width: 1440, height: 900 })
    await expectNoHorizontalOverflow(page)
    const wideOverview = await overviewBox(page)
    const wideShare = (await summaryFacts(page)).reduce((total, fact) => total + fact.box.width, 0) / 2 / wideOverview.width
    expect(wideShare, '1440x900 statistics share').toBeGreaterThanOrEqual(0.46)
    expect(wideShare, '1440x900 statistics share').toBeLessThanOrEqual(0.49)
    await capture(page, testInfo, 'home-1440-compact')
    await page.setViewportSize({ width: 1280, height: 800 })

    // A routine Current projection dedicates its height to the map, not metadata.
    // The projection is Server-shaped and internally consistent: one resolved
    // country is one Known Peer record.
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
    await expect(map.getByRole('img', { name: 'Peer countries map' })).toBeVisible({ timeout: 30_000 })
    await expect(map.getByRole('status')).toHaveCount(0)
    const routineSvg = (await map.getByRole('img', { name: 'Peer countries map' }).boundingBox())!
    expect(routineSvg.height).toBeGreaterThanOrEqual(206)
    expect((await worldBox(page)).height, 'actual world grows beyond the old 136px map').toBeGreaterThan(200)
    expect(routineSvg.height).toBeLessThanOrEqual(260)
    const compactOverview = await overviewBox(page)
    expect(compactOverview.height, 'routine overview start').toBeGreaterThanOrEqual(240)
    expect(compactOverview.height, 'routine overview start').toBeLessThanOrEqual(280)
  })

  test('stacks the statistics over a compact, expandable map on narrow screens', async ({ page }) => {
    test.skip(test.info().project.name === 'desktop-1280', 'the desktop project keeps the two-column overview')
    await openHomeWithGeo(page)

    const map = page.getByRole('region', { name: 'Peer countries' })
    await expect(map).toBeVisible({ timeout: 30_000 })
    const canvas = page.getByRole('img', { name: 'Peer countries map' })
    await expect(canvas).toBeVisible({ timeout: 30_000 })

    const facts = await summaryFacts(page)
    const stats = unionBox(facts.map((fact) => fact.box))
    const mapBox = (await map.boundingBox())!
    // No room for two columns: the map sits below the 2x2 statistics.
    expect(mapBox.y).toBeGreaterThanOrEqual(stats.y + stats.height - 1)

    // No stretch and no letterboxing: the rendered box must match the SVG's own
    // viewBox ratio, and that viewBox must still contain the whole world
    // projection (the map reserves a little room for edge markers, so the box
    // ratio is the viewBox ratio, not a fixed chart constant).
    const compact = (await canvas.boundingBox())!
    const compactWorld = await worldBox(page)
    expect(Math.round(compact.height), 'compact map canvas').toBeGreaterThanOrEqual(120)
    const viewBox = (await canvas.getAttribute('viewBox'))!.split(/\s+/).map(Number)
    expect(compact.height / compact.width, 'no vertical stretch').toBeCloseTo(viewBox[3] / viewBox[2], 3)
    expect(viewBox[2], 'viewBox keeps the whole world width').toBeLessThanOrEqual(1004)
    expect(viewBox[3] / viewBox[2], 'world is never cropped to a sliver').toBeGreaterThan(0.38)

    const toggle = map.getByRole('button', { name: 'Show full map' })
    await expect(toggle).toHaveAttribute('aria-expanded', 'false')
    await toggle.focus()
    await expectFocusedElementHasVisibleFocus(page)
    await toggle.press('Enter')
    await expect(map.getByRole('button', { name: 'Collapse map' })).toHaveAttribute('aria-expanded', 'true')
    await expect.poll(async () => (await worldBox(page)).width).toBeGreaterThan(compactWorld.width)
    expect((await worldBox(page)).height).toBeGreaterThan(compactWorld.height)
    await expectNoHorizontalOverflow(page)

    // Collapsing restores the compact canvas instead of hiding the map.
    await map.getByRole('button', { name: 'Collapse map' }).press('Enter')
    await expect(map.getByRole('button', { name: 'Show full map' })).toHaveAttribute('aria-expanded', 'false')
    await expect.poll(async () => Math.round((await canvas.boundingBox())!.height)).toBeLessThanOrEqual(Math.round(compact.height) + 2)

    // The country's exact quantity stays reachable on the compact map.
    const marker = map.locator('g[role="button"]').first()
    const label = await marker.getAttribute('aria-label')
    await marker.focus()
    await marker.press('Enter')
    await expect(map.getByRole('tooltip')).toHaveText(label!)
    await marker.press('Escape')

    await expectVisibleInteractiveTargets(page)
    await expectQuietMap(page)
    await expectNoHorizontalOverflow(page)
  })

  test('reaches the 375px phone width declared by the issue', async ({ page }, testInfo) => {
    test.skip(test.info().project.name !== 'phone-390-touch', 'the 375px check belongs to the 390px phone project')
    await openHomeWithGeo(page)
    const map = page.getByRole('region', { name: 'Peer countries' })
    await expect(map.getByRole('img', { name: 'Peer countries map' })).toBeVisible({ timeout: 30_000 })

    // A real 375px viewport, not a device-scale emulation of another width.
    await page.setViewportSize({ width: 375, height: 844 })
    await expect(page.getByRole('img', { name: 'Peer countries map' })).toBeVisible()
    await expectNoHorizontalOverflow(page)

    const canvas = page.getByRole('img', { name: 'Peer countries map' })
    const compact = (await canvas.boundingBox())!
    const compactWorld = await worldBox(page)
    expect(Math.round(compact.height), '375px compact canvas').toBeGreaterThanOrEqual(120)
    expect(Math.round(compact.height), '375px compact canvas').toBeLessThanOrEqual(160)

    const mapBox = (await map.boundingBox())!
    expect(mapBox.x).toBeGreaterThanOrEqual(0)
    expect(mapBox.x + mapBox.width).toBeLessThanOrEqual(375)
    // The compact map never covers the statistics above it.
    const stats = unionBox((await summaryFacts(page)).map((fact) => fact.box))
    expect(mapBox.y).toBeGreaterThanOrEqual(stats.y + stats.height - 1)

    await capture(page, testInfo, 'home-375-compact')
    await map.getByRole('button', { name: 'Show full map' }).click()
    await expect.poll(async () => (await worldBox(page)).width).toBeGreaterThan(compactWorld.width)
    expect((await worldBox(page)).height).toBeGreaterThan(compactWorld.height)
    await expectNoHorizontalOverflow(page)
    await capture(page, testInfo, 'home-375-expanded')
  })

  test('keeps the four statistics global while the filter changes the map scope', async ({ page }) => {
    await openHomeWithGeo(page)
    const map = page.getByRole('region', { name: 'Peer countries' })
    await expect(map).toBeVisible({ timeout: 30_000 })

    const markers = map.locator('g[role="button"]')
    const markerNames = () => markers.evaluateAll(elements => elements.map(element => element.getAttribute('aria-label')))
    await expect(map.getByRole('img', { name: 'Peer countries map' })).toBeVisible({ timeout: 30_000 })
    await expect.poll(() => markers.count(), 'the unfiltered map plots a marker').toBeGreaterThan(0)
    const allNames = await markerNames()
    const before = await summaryValues(page)

    // Filtering re-scopes the map and the Node list; the four statistics stay
    // global, and a narrower scope may only ever drop countries — it can never
    // add one that the full scope did not report.
    const pills = page.getByRole('group', { name: 'Network filter' })
    for (const name of ['PlatON E2E Network', 'Home Convergence Network With An Extremely Long Display Name']) {
      await pills.getByRole('button', { name, exact: true }).click()
      await expect.poll(async () => (await markerNames()).every(label => allNames.includes(label))).toBe(true)
      expect(await summaryValues(page), 'statistics stay global').toEqual(before)
    }

    await pills.getByRole('button', { name: 'All Networks', exact: true }).click()
    await expect.poll(() => markers.count(), 'the full scope comes back').toBe(allNames.length)
    await expectQuietMap(page)
    await expectNoHorizontalOverflow(page)
  })

  test('isolated DTO fixture: keeps every projection state honest, including never observed and a zero basis', async ({ page }) => {
    await openHomeWithGeo(page)
    const map = page.getByRole('region', { name: 'Peer countries' })

    // A Network that never reported a successful Peer Snapshot has no basis at
    // all, so no count may be presented as a real zero.
    await withGeoProjection(page, () => ({
      state: 'unknown', scope: 'unobserved', countries: null,
      knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null,
      unknownWithPublicIpCount: null, unknownWithoutRemoteIpCount: null,
      attribution: null, lastGoodAt: null, staleSince: null, databaseAgeSeconds: null, errorReason: null,
    }))
    await page.reload()
    await expect(map.getByRole('status')).toContainText('No observations yet', { timeout: 30_000 })
    await expect(map.getByRole('img', { name: 'Peer countries map' })).toHaveCount(0)
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
    await expect(map.getByRole('status')).toContainText('No data', { timeout: 30_000 })
    await expect(map.getByRole('img', { name: 'Peer countries map' })).toBeVisible()
    await expectQuietMap(page)
  })

  test('degrades locally for a disabled provider and an unavailable basemap', async ({ page }, testInfo) => {
    await loginAs(page)
    await setGeoProvider(page, 'Disabled')
    try {
      await page.goto('/')
      const map = page.getByRole('region', { name: 'Peer countries' })
      await expect(map.getByRole('status')).toHaveText('Peer countries · Disabled by server')
      await expect(map.getByRole('img', { name: 'Peer countries map' })).toHaveCount(0)
      // Nothing is expanded and nothing can be: the single control stays, disabled.
      await expect(map.getByRole('button', { name: 'Show full map' })).toBeDisabled()
      await expectQuietMap(page)
      await capture(page, testInfo, 'fixture-disabled-' + testInfo.project.name)

      // The basemap resource itself can fail while the Server data survives.
      await setGeoProvider(page, 'Local MMDB')
      await page.route('**/assets/geo/**', route => route.abort())
      await page.goto('/')
      await expect(map.getByRole('status')).toContainText('Map unavailable', { timeout: 30_000 })
      await expect(map.getByRole('img', { name: 'Peer countries map' })).toHaveCount(0)
      // Home keeps working: the statistics and the Node cards are untouched.
      await expect(page.getByRole('article').first()).toBeVisible()
      await expectQuietMap(page)
      await capture(page, testInfo, 'fixture-failure-' + testInfo.project.name)
    } finally {
      await page.unroute('**/assets/geo/**')
      await setGeoProvider(page, 'Local MMDB')
    }
  })
})
