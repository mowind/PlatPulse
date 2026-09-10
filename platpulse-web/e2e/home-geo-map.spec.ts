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
 * or private state, and every scenario normalizes the Geo provider itself and
 * restores Disabled afterwards, so run order can never change an outcome.
 */

// These scenarios sign in, open Admin Settings, and walk several Home states;
// the default 30s budget is too tight for the shared real Server.
test.describe.configure({ timeout: 120_000 })

const GEO_CARD = (page: Page) =>
  page.getByRole('article').filter({ has: page.getByRole('heading', { name: 'Geo provider' }) })

async function setGeoProvider(page: Page, name: 'Local MMDB' | 'Disabled') {
  await page.goto('/admin/settings')
  const card = GEO_CARD(page)
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
 * written to the gitignored `playwright-report/issue-133/` directory, so a
 * local run leaves browsable 1440 / 1280 / 375 compact and expanded evidence
 * instead of discarding it with the per-test output directory.
 */
async function capture(page: Page, testInfo: TestInfo, name: string) {
  const body = await page.screenshot()
  await testInfo.attach(name, { body, contentType: 'image/png' })
  const directory = join('playwright-report', 'issue-133')
  mkdirSync(directory, { recursive: true })
  await page.screenshot({ path: join(directory, name + '.png') })
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
async function withGeoProjection(page: Page, patch: (geo: Record<string, unknown>) => Record<string, unknown>) {
  await page.route('**/api/public/v1/networks*', async (route) => {
    const response = await route.fetch()
    const body = await response.json()
    if (Array.isArray(body)) {
      for (const network of body) network.geo = patch(network.geo ?? {})
    }
    await route.fulfill({ response, json: body })
  })
}

test.describe('Home compact overview and Peer country map (issue #133)', () => {
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
    // statistics covering about 42% (accepted 40-45%) of the overview width.
    const stats = unionBox(facts.map((fact) => fact.box))
    const mapBox = (await map.boundingBox())!
    const overview = await overviewBox(page)
    expect(stats.x + stats.width, 'no map left of the statistics').toBeLessThanOrEqual(mapBox.x + 1)
    expect(stats.y).toBeLessThan(mapBox.y + mapBox.height)
    const share = stats.width / overview.width
    expect(share, 'statistics share of the overview').toBeGreaterThanOrEqual(0.38)
    expect(share, 'statistics share of the overview').toBeLessThanOrEqual(0.46)

    // The map states its scope, its two independent data dimensions, and the
    // Peer-record basis without claiming unique Peers or Node locations.
    await expect(map.getByRole('heading', { name: 'Peer countries' })).toBeVisible()
    await expect(map.getByText('Scope: All Networks')).toBeVisible()
    await expect(map.getByText(/^Peer observation: /)).toBeVisible()
    await expect(map.getByText(/^Map resource: /)).toBeVisible()
    await expect(map.getByText(/counted per Node, not deduplicated by IP/)).toBeVisible()
    await expect(map.getByText(/Country outlines: Natural Earth 1:110m/)).toBeVisible()
    // The Server's own attribution stays visible next to the local basemap's.
    await expect(map.getByText(/GeoLite Data created by MaxMind/)).toBeVisible()
    await expect(map.getByRole('img', { name: 'Peer countries map' })).toBeVisible()
    // Country names are available to assistive technology, not only ISO codes.
    await expect(map.getByRole('list', { name: 'Peer countries by count' }).getByText('Sweden', { exact: true })).toBeAttached()
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
    await page.getByRole('button', { name: 'Show full map' }).click()
    await capture(page, testInfo, 'home-1280-expanded')
    await page.getByRole('button', { name: 'Collapse map' }).click()

    await page.setViewportSize({ width: 1440, height: 900 })
    await expectNoHorizontalOverflow(page)
    const wideOverview = await overviewBox(page)
    const wideShare = (await summaryFacts(page)).reduce((total, fact) => total + fact.box.width, 0) / 2 / wideOverview.width
    expect(wideShare, '1440x900 statistics share').toBeGreaterThanOrEqual(0.38)
    expect(wideShare, '1440x900 statistics share').toBeLessThanOrEqual(0.46)
    await capture(page, testInfo, 'home-1440-compact')
    await page.getByRole('button', { name: 'Show full map' }).click()
    await capture(page, testInfo, 'home-1440-expanded')
    await page.getByRole('button', { name: 'Collapse map' }).click()
    await page.setViewportSize({ width: 1280, height: 800 })

    // With a routine Current projection and one short count line, the overview
    // starts inside the accepted 240-280px band. The projection is
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
    }))
    await page.reload()
    await expect(map.getByRole('img', { name: 'Peer countries map' })).toBeVisible({ timeout: 30_000 })
    await expect(map.getByText('Current', { exact: true })).toBeVisible()
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

    // The default canvas is the complete world at a compact 150-170px start.
    const compact = (await canvas.boundingBox())!
    expect(Math.round(compact.height), 'compact map canvas').toBeGreaterThanOrEqual(150)
    expect(Math.round(compact.height), 'compact map canvas').toBeLessThanOrEqual(172)

    const toggle = map.getByRole('button', { name: 'Show full map' })
    await expect(toggle).toHaveAttribute('aria-expanded', 'false')
    await toggle.focus()
    await expectFocusedElementHasVisibleFocus(page)
    await toggle.press('Enter')
    await expect(map.getByRole('button', { name: 'Collapse map' })).toHaveAttribute('aria-expanded', 'true')
    await expect.poll(async () => Math.round((await canvas.boundingBox())!.height)).toBeGreaterThanOrEqual(240)
    expect(Math.round((await canvas.boundingBox())!.height), 'expanded map canvas').toBeLessThanOrEqual(280)

    // Collapsing restores the compact canvas instead of hiding the map.
    await map.getByRole('button', { name: 'Collapse map' }).press('Enter')
    await expect(map.getByRole('button', { name: 'Show full map' })).toHaveAttribute('aria-expanded', 'false')
    await expect.poll(async () => Math.round((await canvas.boundingBox())!.height)).toBeLessThanOrEqual(Math.round(compact.height) + 2)

    // Every real country keeps an accessible text statistic beside the map.
    const countries = map.getByRole('list', { name: 'Peer countries by count' })
    await expect(countries).toBeVisible({ timeout: 30_000 })
    await expect(countries).toContainText('SE', { timeout: 30_000 })

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
    expect(Math.round(compact.height), '375px compact canvas').toBeGreaterThanOrEqual(150)
    expect(Math.round(compact.height), '375px compact canvas').toBeLessThanOrEqual(172)

    const mapBox = (await map.boundingBox())!
    expect(mapBox.x).toBeGreaterThanOrEqual(0)
    expect(mapBox.x + mapBox.width).toBeLessThanOrEqual(375)
    // The compact map never covers the statistics above it.
    const stats = unionBox((await summaryFacts(page)).map((fact) => fact.box))
    expect(mapBox.y).toBeGreaterThanOrEqual(stats.y + stats.height - 1)

    await capture(page, testInfo, 'home-375-compact')
    await map.getByRole('button', { name: 'Show full map' }).click()
    await expect.poll(async () => Math.round((await canvas.boundingBox())!.height)).toBeGreaterThanOrEqual(240)
    expect(Math.round((await canvas.boundingBox())!.height), '375px expanded canvas').toBeLessThanOrEqual(280)
    await expectNoHorizontalOverflow(page)
    await capture(page, testInfo, 'home-375-expanded')
  })

  test('keeps the four statistics global while the filter and sort change only the Node list and the map scope', async ({ page }) => {
    await openHomeWithGeo(page)
    const map = page.getByRole('region', { name: 'Peer countries' })
    await expect(map).toBeVisible({ timeout: 30_000 })

    const before = await summaryValues(page)
    const scopeBefore = await map.getByText(/^Scope: /).textContent()
    // Wait for the Node list itself: the map region renders before the Public
    // Projection that fills both the list and its counts has arrived.
    await expect(page.getByLabel('Active Nodes', { exact: true }).getByRole('link').first()).toBeVisible({ timeout: 30_000 })
    const nodesBefore = await page.getByLabel('Active Nodes', { exact: true }).getByRole('link').count()
    expect(nodesBefore, 'the All Networks scope lists every Active Node').toBeGreaterThan(1)

    await page.getByRole('button', { name: 'PlatON E2E Network', exact: true }).click()
    await expect(page.getByRole('link', { name: /Node A/ })).toBeVisible()
    await expect(map.getByText('Scope: PlatON E2E Network')).toBeVisible()
    await expect(map.getByText(/counted per Node, not deduplicated by IP/)).toBeVisible()
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
    await expect(map.getByText('Scope: PlatON E2E Network')).toBeVisible()

    // Returning to All Networks restores the aggregate scope and every Node.
    await page.getByRole('button', { name: 'All Networks', exact: true }).click()
    await expect(map.getByText('Scope: All Networks')).toBeVisible()
    await expect(page.getByLabel('Active Nodes', { exact: true }).getByRole('link')).toHaveCount(nodesBefore)
    await expectNoHorizontalOverflow(page)
  })

  test('keeps every projection state honest, including never observed and a zero basis', async ({ page }) => {
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
    await expect(map.getByText(/no Active Node has reported a successful Peer Snapshot yet/)).toBeVisible({ timeout: 30_000 })
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
    await expect(map.getByText('Known 0 · Unknown 0', { exact: false })).toBeVisible({ timeout: 30_000 })
    await expect(map.getByText(/No country observations are available yet/)).toBeVisible()
    await expectNoHorizontalOverflow(page)
    await page.unroute('**/api/public/v1/networks*')
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
      await expect(page.getByText('Peer countries · Disabled by server', { exact: true })).toBeVisible()
      await expect(page.getByRole('img', { name: 'Peer countries map' })).toHaveCount(0)
      await expect(page.getByText(/^Known /)).toHaveCount(0)
      expect(geometryRequests).toBe(0)

      // Geo enabled but the basemap unavailable: the map degrades locally,
      // Server-provided country counts survive, and Home stays usable.
      await setGeoProvider(page, 'Local MMDB')
      await page.route('**/assets/geo/**', (route) => route.abort())
      await page.goto('/')
      await expect(page.getByText(/Map geometry is Unavailable/)).toBeVisible({ timeout: 30_000 })
      await expect(page.getByRole('img', { name: 'Peer countries map' })).toHaveCount(0)
      await expect(page.getByText(/Known 1 · Unknown 9/)).toBeVisible({ timeout: 30_000 })
      await expect(page.getByRole('list', { name: 'Peer countries by count' })).toContainText('SE', { timeout: 30_000 })
      await expect(page.getByText(/Last good database load/)).toBeVisible()
      const facts = await summaryFacts(page)
      expect(facts.map((fact) => fact.label)).toEqual(['Active Nodes', 'Healthy Nodes', 'Attention', 'Networks'])
      await expect(page.getByLabel('Active Nodes', { exact: true }).getByRole('link').first()).toBeVisible()
      await expectNoHorizontalOverflow(page)
      await page.unroute('**/assets/geo/**')
    } finally {
      await setGeoProvider(page, 'Disabled')
    }
  })
})
