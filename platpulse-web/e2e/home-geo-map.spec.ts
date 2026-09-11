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
 * scenario restores Disabled. DTO overrides below are isolated UI fixtures,
 * not claims about the real Server's Peer observations.
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

async function openMapInformation(page: Page) {
  const dialog = page.getByRole('dialog', { name: 'Map information' })
  if (!await dialog.isVisible()) {
    const trigger = page.getByRole('button', { name: 'Map information', exact: true })
    if (test.info().project.use.hasTouch) await trigger.tap()
    else await trigger.click()
  }
  await expect(dialog).toBeVisible()
  return dialog
}

async function closeMapInformation(page: Page) {
  await page.getByRole('button', { name: 'Close map information' }).click()
  await expect(page.getByRole('dialog', { name: 'Map information' })).toHaveCount(0)
}

type Box = { x: number; y: number; width: number; height: number }

/**
 * Deliver one review screenshot: it is attached to the test result and also
 * written to the gitignored `playwright-report/emerald/` directory, so a
 * local run leaves browsable 1440 / 1280 / 375 compact and expanded evidence
 * instead of discarding it with the per-test output directory.
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

/** Accessible live status must remain available without standing visible prose. */
async function expectQuietMap(page: Page) {
  const map = page.getByRole('region', { name: 'Peer countries' })
  await expect(map.getByRole('heading')).toHaveCount(0)
  await expect(map.getByText(/^Scope: |^Known |^Peer observation: |^Map resource: /)).toHaveCount(0)
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
  for (const button of await map.locator('button').all()) {
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
    const dialog = await openMapInformation(page)
    await expect(dialog.getByText('Scope: All Networks')).toBeVisible()
    await expect(dialog.getByText(/Known 1 · Unknown 9 · 10 Peer records in scope/)).toBeVisible()
    await expect(dialog.getByRole('listitem').filter({ hasText: 'Sweden' })).toContainText('1')
    await closeMapInformation(page)
    await capture(page, testInfo, 'real-' + testInfo.project.name + '-compact')
    if (testInfo.project.name === 'desktop-1280') {
      await page.setViewportSize({ width: 1440, height: 1000 })
      await expectQuietMap(page)
      await capture(page, testInfo, 'real-1440x1000-compact')
    }
    await marker.focus()
    await marker.press('Enter')
    await expect(map.getByRole('tooltip')).toHaveText('Sweden · 1 records')
    await marker.press('Escape')
    await expect(map.getByRole('tooltip')).toHaveCount(0)
    await marker.press('Space')
    await expect(map.getByRole('tooltip')).toBeVisible()
    await page.keyboard.press('Tab')
    await expect(map.getByRole('tooltip')).toHaveCount(0)
    if (testInfo.project.use.hasTouch) await marker.tap()
    else await marker.hover()
    await expect(map.getByRole('tooltip')).toHaveText('Sweden · 1 records')
    await page.getByRole('button', { name: 'All Networks', exact: true }).click()
    await expect(map.getByRole('tooltip')).toHaveCount(0)
    const status = map.getByRole('button', { name: /^Map status: / })
    await status.focus()
    await status.press('Enter')
    await expect(page.getByRole('dialog', { name: 'Map information' })).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(status).toBeFocused()
    await expectQuietMap(page)
    await expectNoHorizontalOverflow(page)
  })

  test('isolated DTO fixture: dense Europe and East Asia retain exact counts and fixed points across resize', async ({ page }, testInfo) => {
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
    // Crowded markers fall back to a bare dot at every width; an isolated
    // quantity keeps its bounded numeral wherever the layout has room for it.
    // Either way the exact count stays reachable through the tooltip and list.
    await expect(belgium.locator('text')).toHaveCount(0)
    if (test.info().project.name === 'desktop-1280') {
      await expect(china.locator('text')).toHaveText('2002')
    }
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
    const dialog = await openMapInformation(page)
    await expect(dialog.getByRole('listitem')).toHaveCount(7)
    await expect(dialog.getByRole('listitem').filter({ hasText: 'China' })).toContainText('2,002')
    await expect(dialog.getByRole('listitem').filter({ hasText: 'Sweden' })).toContainText('no representative point')
    await expect(map.getByRole('button', { name: /^Sweden/ })).toHaveCount(0)
    await closeMapInformation(page)
  })

  test('isolated resource fixture: loading remains icon-only and dialog explains it', async ({ page }, testInfo) => {
    await openHomeWithGeo(page)
    let release!: () => void
    const gate = new Promise<void>(resolve => { release = resolve })
    await page.route('**/assets/geo/**', async route => { await gate; await route.continue() })
    try {
      await page.reload()
      const map = page.getByRole('region', { name: 'Peer countries' })
      await expect(map.getByRole('button', { name: 'Show full map' })).toBeDisabled()
      await expectQuietMap(page)
      const dialog = await openMapInformation(page)
      await expect(dialog.getByText('Map resource: Starting')).toBeVisible()
      await expect(dialog.getByText(/Known 1 · Unknown 9/)).toBeVisible()
      await closeMapInformation(page)
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
    test.skip(test.info().project.name !== 'desktop-1280', 'the side-by-side overview is the desktop composition')
    await openHomeWithGeo(page)

    const map = page.getByRole('region', { name: 'Peer countries' })
    await expect(map).toBeVisible({ timeout: 30_000 })
    // Logo, Owner Admin entry, and the connection indicator stay in place.
    await expect(page.getByRole('link', { name: 'PlatPulse', exact: true })).toBeVisible()
    await expect(page.getByRole('link', { name: 'Admin' })).toBeVisible()
    await expect(page.getByText('Live updates connected')).toBeVisible()

    // Four global statistics in the accepted order, each in its own card.
    const facts = await summaryFacts(page)
    expect(facts.map((fact) => fact.label)).toEqual(['Active Nodes', 'Healthy Nodes', 'Attention', 'Networks'])
    for (const fact of facts) {
      expect(fact.value.length).toBeGreaterThan(0)
      expect(fact.box.height).toBeGreaterThanOrEqual(80)
      expect(fact.box.height).toBeLessThanOrEqual(140)
    }

    // A stable Grid places the 2x2 statistics left and the map right, with the
    // statistics covering the Emerald layout’s 48% column share (minus the gap).
    const stats = unionBox(facts.map((fact) => fact.box))
    const mapBox = (await map.boundingBox())!
    const overview = await overviewBox(page)
    expect(stats.x + stats.width, 'no map left of the statistics').toBeLessThanOrEqual(mapBox.x + 1)
    expect(stats.y).toBeLessThan(mapBox.y + mapBox.height)
    const share = stats.width / overview.width
    expect(share, 'statistics share of the overview').toBeGreaterThanOrEqual(0.46)
    expect(share, 'statistics share of the overview').toBeLessThanOrEqual(0.49)

    // The map states its scope, its two independent data dimensions, and the
    // Peer-record basis without claiming unique Peers or Node locations.
    await expect(map.getByRole('heading', { name: 'Peer countries' })).toHaveCount(0)
    await openMapInformation(page)
    await expect(map.getByText('Scope: All Networks')).toBeVisible()
    await expect(map.getByText(/^Peer observation: /)).toBeVisible()
    await expect(map.getByText(/^Map resource: /)).toBeVisible()
    await expect(map.getByText(/counted per Node, not deduplicated by IP/)).toBeVisible()
    await expect(page.getByRole('dialog', { name: 'Map information' }).getByText(/Country outlines: Natural Earth 1:110m/)).toBeVisible()
    // The Server's own attribution stays visible next to the local basemap's.
    await expect(page.getByRole('dialog', { name: 'Map information' }).getByText(/GeoLite Data created by MaxMind/)).toBeVisible()
    await expect(map.getByRole('img', { name: 'Peer countries map' })).toBeVisible()
    // Country names are available to assistive technology, not only ISO codes.
    await expect(map.getByRole('list', { name: 'Peer countries by count' }).getByText('Sweden', { exact: true })).toBeAttached()
    await closeMapInformation(page)
    // The whole Home body never shows a raw Peer address or the database path.
    await expect(page.locator('body')).not.toContainText('89.160.20.112')
    await expect(page.locator('body')).not.toContainText('GeoIP2-Country-Test')

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
      return element ? { isMap: Boolean(element.closest('svg[role="img"]')), tag: element.tagName } : null
    }, { x: imageBox.x + imageBox.width / 2, y: imageBox.y + imageBox.height / 2 })
    expect(hit?.isMap, 'the map keeps its own pointer interaction').toBe(true)

    await expectVisibleInteractiveTargets(page)
    await expectNoHorizontalOverflow(page)
    // Delivered evidence at the project's own 1280x800 acceptance viewport,
    // then at the wider 1440x900 review viewport named by the issue.
    await capture(page, testInfo, 'home-1280-compact')
    const desktopWorld = await worldBox(page)
    await page.getByRole('button', { name: 'Show full map' }).click()
    await expect.poll(async () => (await worldBox(page)).width).toBeGreaterThan(desktopWorld.width)
    await expectNoHorizontalOverflow(page)
    await capture(page, testInfo, 'home-1280-expanded')
    await page.getByRole('button', { name: 'Collapse map' }).click()

    await page.setViewportSize({ width: 1440, height: 900 })
    await expectNoHorizontalOverflow(page)
    const wideOverview = await overviewBox(page)
    const wideShare = (await summaryFacts(page)).reduce((total, fact) => total + fact.box.width, 0) / 2 / wideOverview.width
    expect(wideShare, '1440x900 statistics share').toBeGreaterThanOrEqual(0.46)
    expect(wideShare, '1440x900 statistics share').toBeLessThanOrEqual(0.49)
    await capture(page, testInfo, 'home-1440-compact')
    await page.getByRole('button', { name: 'Show full map' }).click()
    await capture(page, testInfo, 'home-1440-expanded')
    await page.getByRole('button', { name: 'Collapse map' }).click()
    await page.setViewportSize({ width: 1280, height: 800 })

    // A routine Current projection dedicates its height to the map, not metadata.
    // The overview remains compact while the SVG grows to 220-260px. The projection is
    // Server-shaped and internally consistent: one resolved country is one
    // Known Peer record.
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
    // The 220px canvas footprint reserves a 14px credit strip below the SVG.
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

    // The complete world retains the same natural aspect ratio on touch layouts.
    const compact = (await canvas.boundingBox())!
    const compactWorld = await worldBox(page)
    expect(Math.round(compact.height), 'compact map canvas').toBeGreaterThanOrEqual(120)
    // No stretch and no letterboxing: the rendered box must match the SVG's own
    // viewBox ratio exactly, and that viewBox must still contain the whole world
    // projection (the map reserves a little room for edge markers, so the box
    // ratio is the viewBox ratio, not a fixed chart constant).
    const geometry = await canvas.evaluate(element => ({
      viewBox: element.getAttribute('viewBox')!.split(/\s+/).map(Number),
      projection: 0,
    }))
    const [, , viewWidth, viewHeight] = geometry.viewBox
    expect(compact.height / compact.width, 'no vertical stretch').toBeCloseTo(viewHeight / viewWidth, 3)
    expect(viewWidth, 'viewBox keeps the whole world width').toBeLessThanOrEqual(1004)
    expect(viewHeight, 'viewBox keeps the whole world height plus marker room').toBeLessThanOrEqual(600)
    expect(viewHeight / viewWidth, 'world is never cropped to a sliver').toBeGreaterThan(0.38)

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

    // Every real country keeps an accessible text statistic beside the map.
    await openMapInformation(page)
    const countries = map.getByRole('list', { name: 'Peer countries by count' })
    await expect(countries).toBeVisible({ timeout: 30_000 })
    await expect(countries).toContainText('SE', { timeout: 30_000 })
    await closeMapInformation(page)

    await expectVisibleInteractiveTargets(page)
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
    expect(Math.round(compact.height), '375px compact canvas').toBeGreaterThanOrEqual(125)
    expect(Math.round(compact.height), '375px compact canvas').toBeLessThanOrEqual(150)

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

  test('keeps the four statistics global while the filter and sort change only the Node list and the map scope', async ({ page }) => {
    await openHomeWithGeo(page)
    const map = page.getByRole('region', { name: 'Peer countries' })
    await expect(map).toBeVisible({ timeout: 30_000 })

    const before = await summaryValues(page)
    await openMapInformation(page)
    const scopeBefore = await map.getByText(/^Scope: /).textContent()
    await closeMapInformation(page)
    // Wait for the Node list itself: the map region renders before the Public
    // Projection that fills both the list and its counts has arrived.
    await expect(page.getByLabel('Active Nodes', { exact: true }).getByRole('link').first()).toBeVisible({ timeout: 30_000 })
    const nodesBefore = await page.getByLabel('Active Nodes', { exact: true }).getByRole('link').count()
    expect(nodesBefore, 'the All Networks scope lists every Active Node').toBeGreaterThan(1)

    await page.getByRole('button', { name: 'PlatON E2E Network', exact: true }).click()
    await expect(page.getByRole('link', { name: /Node A/ })).toBeVisible()
    await openMapInformation(page)
    await expect(map.getByText('Scope: PlatON E2E Network')).toBeVisible()
    await expect(map.getByText(/counted per Node, not deduplicated by IP/)).toBeVisible()
    await closeMapInformation(page)
    const nodesFiltered = await page.getByLabel('Active Nodes', { exact: true }).getByRole('link').count()
    expect(nodesFiltered).toBeLessThan(nodesBefore)
    // The four global statistics are projections of the whole Public
    // Projection: filtering or sorting the Node list never changes them.
    expect(await summaryValues(page)).toEqual(before)
    expect(scopeBefore).toBe('Scope: All Networks')

    // Sorting reorders the Node list only: the statistics and map scope stay.
    const firstBefore = await page.getByLabel('Active Nodes', { exact: true }).getByRole('link').first().textContent()
    await page.getByRole('combobox', { name: 'Sort' }).selectOption('name')
    await expect(page.getByRole('link', { name: /Node A/ })).toBeVisible()
    const firstAfter = await page.getByLabel('Active Nodes', { exact: true }).getByRole('link').first().textContent()
    expect(firstAfter).not.toBe(firstBefore)
    expect(await summaryValues(page)).toEqual(before)
    await openMapInformation(page)
    await expect(map.getByText('Scope: PlatON E2E Network')).toBeVisible()

    await closeMapInformation(page)
    // Returning to All Networks restores the aggregate scope and every Node.
    await page.getByRole('button', { name: 'All Networks', exact: true }).click()
    await openMapInformation(page)
    await expect(map.getByText('Scope: All Networks')).toBeVisible()
    await expect(page.getByLabel('Active Nodes', { exact: true }).getByRole('link')).toHaveCount(nodesBefore)
    await expectNoHorizontalOverflow(page)
  })

  test('isolated DTO fixture: keeps every projection state honest, including never observed and a zero basis', async ({ page }) => {
    await openHomeWithGeo(page)
    const map = page.getByRole('region', { name: 'Peer countries' })

    // Never observed: no Active Node has produced a Peer Snapshot, so there is
    // no denominator and no count may be presented as a real zero.
    await withGeoProjection(page, () => ({
      state: 'unknown',
      scope: 'unobserved',
      countries: null,
      knownCountryCount: null,
      unknownCountryCount: null,
      availablePeerCount: null,
      unknownWithPublicIpCount: null,
      unknownWithoutRemoteIpCount: null,
      attribution: null,
      lastGoodAt: null,
      staleSince: null,
      databaseAgeSeconds: null,
      errorReason: null,
    }))
    await page.reload()
    await expect(map.getByRole('status')).toContainText('No observations yet', { timeout: 30_000 })
    await expectQuietMap(page)
    await capture(page, test.info(), 'fixture-unobserved-' + test.info().project.name)
    await openMapInformation(page)
    await expect(map.getByText(/no Active Node has reported a successful Peer Snapshot yet/)).toBeVisible()
    await expect(map.getByText(/^Known /)).toHaveCount(0)
    await expect(map.getByRole('list', { name: 'Peer countries by count' })).toHaveCount(0)
    await expect(page.getByLabel('Active Nodes', { exact: true }).getByRole('link').first()).toBeVisible()

    // A successful empty projection is an authoritative zero with its basis.
    await withGeoProjection(page, () => ({
      state: 'current',
      scope: 'complete',
      countries: [],
      knownCountryCount: 0,
      unknownCountryCount: 0,
      availablePeerCount: 0,
      unknownWithPublicIpCount: 0,
      unknownWithoutRemoteIpCount: 0,
      attribution: null,
      lastGoodAt: null,
      staleSince: null,
      databaseAgeSeconds: null,
      errorReason: null,
    }))
    await page.reload()
    await expect(map.getByRole('status')).toContainText('No data', { timeout: 30_000 })
    await expectQuietMap(page)
    await capture(page, test.info(), 'fixture-empty-' + test.info().project.name)
    await openMapInformation(page)
    await expect(map.getByText(/^Known 0 · 0 Peer records in scope;/)).toBeVisible()
    await expect(map.getByText(/Unknown 0/)).toHaveCount(0)
    await expect(map.getByText(/No country observations are available yet/)).toBeVisible()
    await expectNoHorizontalOverflow(page)
    await page.unroute('**/api/public/v1/networks*')
  })

  test('discloses map information without a modal and restores keyboard focus', async ({ page }) => {
    await openHomeWithGeo(page)
    const map = page.getByRole('region', { name: 'Peer countries' })
    await expect(map.getByRole('img', { name: 'Peer countries map' })).toBeVisible()
    const trigger = map.getByRole('button', { name: 'Map information', exact: true })
    await expect(map.getByText(/^Scope: /)).toHaveCount(0)
    await expect(map.getByText(/^Known /)).toHaveCount(0)
    await expect(map.getByText(/Last good database load/)).toHaveCount(0)
    await expect(map.getByRole('status')).toHaveCount(1)
    await expect(map.getByRole('status')).toContainText('Data stale')
    await trigger.focus()
    await expectFocusedElementHasVisibleFocus(page)
    await trigger.press('Enter')
    const dialog = page.getByRole('dialog', { name: 'Map information' })
    await expect(dialog).toBeVisible()
    await expect(dialog).not.toHaveAttribute('aria-modal', 'true')
    await expect(trigger).toHaveAttribute('aria-expanded', 'true')
    await expect(dialog.getByText('Scope: All Networks')).toBeVisible()
    await expect(dialog.getByText(/Known 1 · Unknown 9/)).toBeVisible()
    await expect(dialog.getByText(/counted per Node, not deduplicated by IP/)).toBeVisible()
    await expect(dialog.getByText(/without a usable public remote IP/)).toBeVisible()
    await expect(dialog.getByText(/without a retained country result/)).toBeVisible()
    await expect(dialog.getByText(/Last good database load/)).toBeVisible()
    await expect(dialog.getByRole('list', { name: 'Peer countries by count' })).toContainText('Sweden')
    // Source links are inline attribution prose, not toolbar controls. The
    // close control retains the full touch target; source links stay real and focusable.
    const close = dialog.getByRole('button', { name: 'Close map information' })
    const closeBox = (await close.boundingBox())!
    expect(closeBox.width).toBeGreaterThanOrEqual(44)
    expect(closeBox.height).toBeGreaterThanOrEqual(44)
    for (const link of await dialog.getByRole('link').all()) {
      await expect(link).toHaveAttribute('href', /^https:\/\//)
      await link.focus()
      await expect(link).toBeFocused()
    }
    await expectNoHorizontalOverflow(page)
    await page.keyboard.press('Escape')
    await expect(dialog).toHaveCount(0)
    await expect(trigger).toBeFocused()
    await expectVisibleInteractiveTargets(page)
    await trigger.press('Enter')
    await page.getByRole('article').filter({ has: page.getByText('Active Nodes', { exact: true }) }).click()
    await expect(dialog).toHaveCount(0)
    await expect(trigger).toBeFocused()
    await trigger.press('Enter')
    await page.getByRole('button', { name: 'Close map information' }).click()
    await expect(dialog).toHaveCount(0)
    await expect(trigger).toBeFocused()
  })

  test('isolated DTO fixture: exposes prioritized exceptions only through accessible status and information', async ({ page }) => {
    await openHomeWithGeo(page)
    const map = page.getByRole('region', { name: 'Peer countries' })
    const current = {
      state: 'current', scope: 'complete',
      countries: [{ countryCode: 'SE', count: 1, staleCount: 0, centroidLat: 60.1282, centroidLon: 18.6435 }],
      knownCountryCount: 1, unknownCountryCount: 0, availablePeerCount: 1,
      unknownWithPublicIpCount: 0, unknownWithoutRemoteIpCount: 0,
      attribution: null, lastGoodAt: null, staleSince: null, databaseAgeSeconds: null, errorReason: null,
    }
    await withGeoProjection(page, () => current, 'current')
    await page.reload()
    await expect(map.getByRole('img', { name: 'Peer countries map' })).toBeVisible()
    await expect(map.getByRole('status')).toHaveCount(0)
    await expect(map.getByRole('heading', { name: 'Peer countries', exact: true })).toHaveCount(0)
    await expect(map.getByText('· 2 records', { exact: true })).toHaveCount(0)
    const marker = map.getByRole('button', { name: 'Sweden · 2 records', exact: true })
    if (test.info().project.use.hasTouch) await marker.tap()
    else await marker.click()
    // Desktop pointer entry opens the tooltip; clicking may toggle it closed.
    if (!test.info().project.use.hasTouch) await marker.press('Enter')
    await expect(map.getByRole('tooltip')).toHaveText('Sweden · 2 records')
    await marker.press('Escape')
    await expect(map.getByRole('tooltip')).toHaveCount(0)
    await expect(map.getByText(/^Scope: /)).toHaveCount(0)
    await expect(map.getByText(/^Known /)).toHaveCount(0)
    for (const scenario of [
      { patch: { unknownCountryCount: 2, availablePeerCount: 3, unknownWithPublicIpCount: 2 }, hint: '4 unknown locations' },
      { patch: { scope: 'partial' }, hint: 'Partial data' },
      { patch: { state: 'stale', unknownCountryCount: 2, availablePeerCount: 3, unknownWithPublicIpCount: 2 }, hint: 'Data stale' },
      { patch: { countries: [{ countryCode: 'SE', count: 1, staleCount: 0, centroidLat: null, centroidLon: null }] }, hint: 'Some locations not shown' },
    ]) {
      await page.unroute('**/api/public/v1/networks*')
      await withGeoProjection(page, () => ({ ...current, ...scenario.patch }), 'current')
      await page.reload()
      await expect(map.getByRole('img', { name: 'Peer countries map' })).toBeVisible()
      await expect(map.getByRole('status')).toHaveCount(1)
      await expect(map.getByRole('status')).toContainText(scenario.hint)
      await expectQuietMap(page)
      await expect(map.getByRole('button', { name: 'Map status: ' + await map.getByRole('status').textContent(), exact: true })).toBeVisible()
      await expectNoHorizontalOverflow(page)
    }
  })

  test('degrades locally for a disabled provider and an unavailable basemap', async ({ page }) => {
    await loginAs(page)
    try {
      // Normalize first: the map scenarios above leave Local MMDB selected.
      // A Server-disabled Geo Provider renders the neutral notice and never
      // loads map geometry at all.
      await setGeoProvider(page, 'Disabled')
      let geometryRequests = 0
      page.on('request', (request) => {
        if (request.url().includes('/assets/geo/')) geometryRequests += 1
      })
      await page.goto('/')
      const map = page.getByRole('region', { name: 'Peer countries' })
      await expect(map).toBeVisible({ timeout: 30_000 })
      await expect(map.getByRole('status')).toHaveText('Peer countries · Disabled by server')
      await expectQuietMap(page)
      const disabledInfo = await openMapInformation(page)
      await expect(disabledInfo.getByText('Peer countries · Disabled by server', { exact: true })).toBeVisible()
      await closeMapInformation(page)
      await expect(page.getByRole('img', { name: 'Peer countries map' })).toHaveCount(0)
      await expect(page.getByText(/^Known /)).toHaveCount(0)
      expect(geometryRequests).toBe(0)

      // Geo enabled but the basemap unavailable: the map degrades locally,
      // Server-provided country counts survive, and Home stays usable.
      await setGeoProvider(page, 'Local MMDB')
      await page.route('**/assets/geo/**', (route) => route.abort())
      await page.goto('/')
      await expect(map.getByRole('status')).toContainText('Map unavailable', { timeout: 30_000 })
      await expect(map.getByRole('status')).toHaveCount(1)
      await expectQuietMap(page)
      await capture(page, test.info(), 'fixture-failure-' + test.info().project.name)
      await openMapInformation(page)
      await expect(page.getByRole('img', { name: 'Peer countries map' })).toHaveCount(0)
      await expect(page.getByText(/Known 1 · Unknown 9/)).toBeVisible({ timeout: 30_000 })
      await expect(page.getByRole('list', { name: 'Peer countries by count' })).toContainText('SE', { timeout: 30_000 })
      await expect(page.getByText(/Last good database load/)).toBeVisible()
      const facts = await summaryFacts(page)
      expect(facts.map((fact) => fact.label)).toEqual(['Active Nodes', 'Healthy Nodes', 'Attention', 'Networks'])
      await expect(page.getByLabel('Active Nodes', { exact: true }).getByRole('link').first()).toBeVisible()
      await expectNoHorizontalOverflow(page)
      await closeMapInformation(page)
      await expect(map.getByRole('button', { name: 'Retry map' })).toHaveCount(0)
      await page.unroute('**/assets/geo/**')
      await map.getByRole('button', { name: /^Map status: Map unavailable/ }).click()
      await page.getByRole('dialog', { name: 'Map information' }).getByRole('button', { name: 'Retry map' }).click()
      await expect(map.getByRole('img', { name: 'Peer countries map' })).toBeVisible({ timeout: 30_000 })
    } finally {
      await setGeoProvider(page, 'Disabled')
    }
  })
})
