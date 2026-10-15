import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test'
import type { PublicNetwork, PublicNode } from '../src/api/generated'
import { closeHomeSurface, expectNoHorizontalOverflow, loginAs, openHomeSort } from './helpers'

// Exercise the routed Public Home seam. Keep the real authenticated Server and
// its response shape; only replace the Public Nodes needed for this scenario.
const LAST_PROGRESS = '2026-08-25T02:00:00+02:00'
const UTC_PROGRESS = '2026-08-25T00:00:00.000Z'
const UTC_LABEL = '25 Aug 2026, 00:00:00 UTC'
const RESYNC_NAME = /Resync progress: 4\.34% toward the Historical High-Water Mark/
const nodeCard = (page: Page, name: string) => page.getByRole('article').filter({ has: page.getByRole('link', { name, exact: true }) })

async function fixture(page: Page) {
  await loginAs(page)
  await page.clock.setFixedTime(new Date('2026-08-25T00:02:00Z'))
  await page.route('**/api/public/v1/networks*', async route => {
    const response = await route.fetch()
    const [network]: PublicNetwork[] = await response.json()
    const base: PublicNode = {
      ...network.nodes[0], health: 'healthy', healthReason: 'Fresh component observations',
      validator: undefined, validatorIdentityReason: undefined,
      currentHead: 6_920_136, historicalHighWatermark: 159_311_799,
      // Deliberately different: the percentage must use the retained watermark.
      networkReferenceHead: 200_000_000,
      resyncLastProgressAt: LAST_PROGRESS,
      resyncProgress: '6920136/159311799 (last progress 2026-08-25T00:00:00Z)',
    }
    const nodes: PublicNode[] = [
      { ...base, nodeId: 'resync-browser-active', displayName: 'A Resync', resyncState: 'resyncing' },
      { ...base, nodeId: 'resync-browser-normal', displayName: 'B Normal', resyncState: 'normal' },
      { ...base, nodeId: 'resync-browser-unknown', displayName: 'C Unknown progress', resyncState: 'resyncing', resyncLastProgressAt: undefined },
      // An abnormal health diagnostic, a live resync percentage and no Block
      // Height comparison coexist on one card (#239): health, the Server-owned
      // resync state and the reference comparison are three independent
      // statements, and none of them hides another.
      { ...base, nodeId: 'resync-browser-unhealthy', displayName: 'D Unhealthy', resyncState: 'resyncing', health: 'unhealthy', healthReason: 'RPC observation failed' },
    ]
    await route.fulfill({ response, json: [{ ...network, nodes }] })
  })
  await page.goto('/')
  // The Sort order lives behind the Home toolbar's Sort & filter entry (issue
  // #232); the surface is closed again so it cannot overlay the cards below.
  await (await openHomeSort(page)).selectOption('name')
  await closeHomeSurface(page)
  await expect(nodeCard(page, 'A Resync')).toBeVisible()
}

async function box(locator: Locator) {
  const result = await locator.boundingBox()
  expect(result).not.toBeNull()
  return result!
}

async function evidence(page: Page, testInfo: TestInfo, name: string) {
  if (process.env.EMERALD_EVIDENCE === '1') {
    await page.screenshot({ path: testInfo.outputPath(name + '.png'), fullPage: true })
  }
}

// Observe content order and containment, not a reserved top offset shared with
// other cards. Resync/diagnostics may naturally wrap the identity of this row.
async function cardContentGeometry(cards: Locator) {
  await expect.poll(() => cards.evaluateAll(elements => elements.every(card =>
    Math.abs(card.getBoundingClientRect().top - card.parentElement!.getBoundingClientRect().top) <= 1,
  ))).toBe(true)
  const geometry = await cards.evaluateAll(elements => elements.map(card => {
    const bounds = card.getBoundingClientRect()
    const regions = ['identity', 'resource', 'chain', 'validator'].map(name => {
      const region = card.querySelector('[data-node-region="' + name + '"]')!.getBoundingClientRect()
      return { name, top: region.top, bottom: region.bottom, left: region.left, right: region.right, height: region.height }
    })
    return { top: bounds.top, bottom: bounds.bottom, left: bounds.left, right: bounds.right, height: bounds.height, regions }
  }))
  for (const card of geometry) {
    for (const [index, region] of card.regions.entries()) {
      expect(region.height, region.name + ' is real visible content').toBeGreaterThan(0)
      expect(region.left).toBeGreaterThanOrEqual(card.left - 1)
      expect(region.right).toBeLessThanOrEqual(card.right + 1)
      expect(region.top).toBeGreaterThanOrEqual(card.top - 1)
      expect(region.bottom).toBeLessThanOrEqual(card.bottom + 1)
      if (index > 0) expect(region.top, 'content regions never overlap').toBeGreaterThanOrEqual(card.regions[index - 1].bottom)
    }
    expect(card.regions[3].height, 'unknown identity is a short line, not an empty reserved panel').toBeLessThan(60)
    for (const peer of geometry.filter(peer => Math.abs(peer.top - card.top) <= 1)) {
      expect(Math.abs(peer.bottom - card.bottom), 'only same-row bottoms coordinate').toBeLessThanOrEqual(1)
    }
  }
  return geometry
}

async function statusAndGeometry(page: Page, testInfo: TestInfo) {
  // Move away from the card before measuring: its existing hover lift must not
  // be mistaken for different grid alignment after a dialog interaction.
  await page.mouse.move(0, 0)
  const active = nodeCard(page, 'A Resync')
  const normal = nodeCard(page, 'B Normal')
  const cue = active.getByRole('button', { name: RESYNC_NAME })
  await expect(cue).toBeVisible()
  await expect(cue.getByText('Resyncing', { exact: true })).toBeVisible()
  await expect(cue.getByText('4.34%', { exact: true })).toBeVisible()
  // The full target and last progress live behind the focusable disclosure.
  await cue.click()
  const dialog = page.getByRole('dialog', { name: 'Resync progress' })
  await expect(dialog.getByText('6,920,136 / 159,311,799', { exact: true })).toBeVisible()
  await expect(dialog.getByText('Last progress', { exact: false })).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(dialog).toHaveCount(0)
  await expect(active).not.toContainText(/2026-08-25|T00:00:00|T02:00:00|6920136|159311799 /)
  await expect(active.getByRole('img', { name: 'Healthy', exact: true })).toBeVisible()
  await expect(normal.getByRole('img', { name: 'Healthy', exact: true })).toBeVisible()
  await expect(normal.getByRole('button', { name: /Resync progress/ })).toHaveCount(0)
  await expect(normal).not.toContainText(/Resyncing|last progress/i)
  // The compact card keeps its three resource columns, its one chain height and
  // the Peer count on the identity line; the removed consensus rows are gone.
  for (const label of ['CPU', 'Mem', 'Disk', 'Head']) {
    await expect(normal.getByText(label, { exact: true })).toBeVisible()
  }
  await expect(normal.locator('[data-slot="node-peers-meta"]')).toHaveText(/^Peers \d+$/)
  await expect(normal.getByText('Txs', { exact: true })).toHaveCount(0)
  const unknown = nodeCard(page, 'C Unknown progress')
  await unknown.getByRole('button', { name: /Resync progress: 4\.34% toward/ }).click()
  const unknownDialog = page.getByRole('dialog', { name: 'Resync progress' })
  await expect(unknownDialog.getByText('Last progress: Unknown', { exact: true })).toBeVisible()
  await expect(unknownDialog).not.toContainText(/2026-08-25/)
  await page.keyboard.press('Escape')
  await expect(unknownDialog).toHaveCount(0)
  await expect(unknown).not.toContainText(/2026-08-25/)

  // The unhealthy card keeps all three independent statements in their own
  // places: the Server's diagnostic, the Resyncing percentage it published, and
  // the absolute Block Height the Server is not confirming as current — which
  // says so instead of reading as an ordinary current height, and carries the
  // comparison conclusion the evidence cannot support nowhere at all (#239).
  const unhealthy = nodeCard(page, 'D Unhealthy')
  await expect(unhealthy.getByText('RPC observation failed')).toBeVisible()
  await expect(unhealthy.getByRole('button', { name: RESYNC_NAME })).toBeVisible()
  await expect(unhealthy.locator('[data-slot="node-business-metrics"] [data-slot="metric-row-value"]')).toHaveText('6,920,136')
  await expect(unhealthy.locator('[data-slot="node-head-comparison"]')).toHaveCount(0)
  const retained = unhealthy.locator('[data-slot="node-head-retained"]')
  await expect(retained).toHaveText('Last-good')
  await expect(retained).toHaveAttribute('title', /Current Node Head not confirmed by Server/)
  await expect(retained).toHaveClass(/text-warning/)

  // The same card's Block Height explanation is its accessible entry, reached by
  // ordinary keyboard traversal from the card link and, on a touch project, by a
  // real tap (#239). It states what the retained height is, why no comparison
  // comes from it, and which deployment-local reference a comparison would have
  // read — never a public, canonical or explorer head.
  const homeUrl = page.url()
  const identity = unhealthy.getByRole('button', { name: 'Node identity details' })
  const explanation = page.getByRole('dialog', { name: 'D Unhealthy', exact: true })
  await unhealthy.getByRole('link', { name: 'D Unhealthy', exact: true }).focus()
  for (let step = 0; step < 8; step++) {
    await page.keyboard.press('Tab')
    if (await identity.evaluate(el => el === document.activeElement)) break
  }
  await expect(identity).toBeFocused()
  await page.keyboard.press('Enter')
  await expect(explanation).toBeVisible()
  await expect(explanation).toContainText('Block Height: the last Block Height the Server retained for this Node')
  await expect(explanation).toContainText('marked Last-good because Current Node Head not confirmed by Server')
  await expect(explanation).toContainText('No comparison is asserted from a retained height')
  await expect(explanation).toContainText('Observed Network Head')
  await expect(explanation).toContainText('not the public chain head')
  await expect(page).toHaveURL(homeUrl)
  await page.keyboard.press('Escape')
  await expect(explanation).toBeHidden()
  await expect(identity).toBeFocused()
  const identityTarget = await box(identity)
  expect(identityTarget.width).toBeGreaterThanOrEqual(44)
  expect(identityTarget.height).toBeGreaterThanOrEqual(44)
  if (testInfo.project.use.hasTouch) await identity.tap()
  else await identity.click()
  await expect(explanation).toBeVisible()
  await expect(page).toHaveURL(homeUrl)
  await explanation.getByRole('button', { name: 'Close', exact: true }).click()
  await expect(explanation).toBeHidden()

  // Health remains an independent green marker, not a renamed resync marker;
  // the labeled Resyncing cue has a visually distinct warning color.
  const healthyColor = await active.getByRole('img', { name: 'Healthy', exact: true }).evaluate(el => getComputedStyle(el).backgroundColor)
  const normalColor = await normal.getByRole('img', { name: 'Healthy', exact: true }).evaluate(el => getComputedStyle(el).backgroundColor)
  const resyncColor = await cue.getByText('Resyncing', { exact: true }).evaluate(el => getComputedStyle(el).color)
  expect(healthyColor).toBe(normalColor)
  expect(resyncColor).not.toBe(healthyColor)

  // A Resyncing cue is content, not a reason to equalize heights or region tops
  // across all grid rows. Every region stays readable and contained, and only
  // cards which actually share a row must coordinate their bottom edge.
  await page.mouse.move(0, 0)
  const geometry = await cardContentGeometry(page.locator('[data-slot="node-card"]'))
  expect(geometry).toHaveLength(4)
  console.log(`[home-resync] ${testInfo.project.name}: natural identity content includes live Resyncing and independent unhealthy diagnostic; ${JSON.stringify(geometry)}`)
  const activeBox = await box(active)
  for (const bounds of [await box(active.getByRole('heading', { name: 'A Resync', exact: true })), await box(cue), await box(active.getByLabel('Node process and host network resources', { exact: true }))]) {
    expect(bounds.x).toBeGreaterThanOrEqual(activeBox.x - 1)
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(activeBox.x + activeBox.width + 1)
  }
  await expectNoHorizontalOverflow(page)
}

async function progressDialog(page: Page, testInfo: TestInfo, name: string) {
  const active = nodeCard(page, 'A Resync')
  const trigger = active.getByRole('button', { name: RESYNC_NAME })
  const dialog = page.getByRole('dialog', { name: 'Resync progress', exact: true })
  const homeUrl = page.url()
  // Traverse from the card link, rather than focusing the new trigger directly:
  // this catches a control missing from the ordinary keyboard tab order.
  await active.getByRole('link', { name: 'A Resync', exact: true }).focus()
  for (let step = 0; step < 8; step++) {
    await page.keyboard.press('Tab')
    if (await trigger.evaluate(el => el === document.activeElement)) break
  }
  await expect(trigger).toBeFocused()
  await page.keyboard.press('Enter')
  await expect(dialog).toBeVisible()
  await expect(dialog.getByText('6,920,136 / 159,311,799', { exact: true })).toBeVisible()
  await expect(dialog.getByText(UTC_LABEL)).toBeVisible()
  await expect(dialog.locator('time')).toHaveAttribute('datetime', UTC_PROGRESS)
  await expect(page).toHaveURL(homeUrl)
  await expectNoHorizontalOverflow(page)
  await evidence(page, testInfo, name + '-dialog')
  await page.keyboard.press('Escape')
  await expect(dialog).toBeHidden()
  await expect(trigger).toBeFocused()
  const target = await box(trigger)
  expect(target.width).toBeGreaterThanOrEqual(44)
  expect(target.height).toBeGreaterThanOrEqual(44)
  if (testInfo.project.use.hasTouch) await trigger.tap()
  else await trigger.click()
  await expect(dialog).toBeVisible()
  await expect(page).toHaveURL(homeUrl)
  await dialog.getByRole('button', { name: 'Close', exact: true }).click()
  await expect(dialog).toBeHidden()
}

test('minimal resync status and accessible last progress preserve Home in light and dark', async ({ page }, testInfo) => {
  await fixture(page)
  const viewports = [page.viewportSize()!]
  // Reuse the approved matrix; cover extra-wide within one desktop project.
  if (testInfo.project.name === 'desktop-1440') viewports.push({ width: 1920, height: 1080 })
  for (const viewport of viewports) {
    await page.setViewportSize(viewport)
    for (const dark of [false, true]) {
      await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
      const name = 'home-resync-' + viewport.width + (dark ? '-dark' : '-light')
      await statusAndGeometry(page, testInfo)
      await evidence(page, testInfo, name)
      await progressDialog(page, testInfo, name)
    }
  }
})
