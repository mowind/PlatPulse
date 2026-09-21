import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test'
import type { PublicNetwork, PublicNode } from '../src/api/generated'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

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
    ]
    await route.fulfill({ response, json: [{ ...network, nodes }] })
  })
  await page.goto('/')
  await page.getByLabel('Sort', { exact: true }).selectOption('name')
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

/** The offset of every reserved region from its own card's top, so a resync
 *  chip or an empty Validator state cannot shift one card's regions relative
 *  to another's (and a phone's single column is not mistaken for a shift). */
async function regionTops(card: Locator) {
  return card.evaluate(el => {
    const base = el.getBoundingClientRect().top
    const top = (selector: string) => {
      const node = el.querySelector(selector)
      return node ? Math.round(node.getBoundingClientRect().top - base) : null
    }
    return {
      identity: top('[data-slot="card-x-header"]'),
      resource: top('[data-slot="node-resource-region"]'),
      chain: top('[data-slot="node-business-metrics"]'),
      validator: top('[data-slot="linked-validator"]'),
    }
  })
}

async function statusAndGeometry(page: Page) {
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
  for (const label of ['CPU', 'Memory', 'Head', 'Txs', 'Peers']) {
    await expect(normal.getByText(label, { exact: true })).toBeVisible()
  }
  const unknown = nodeCard(page, 'C Unknown progress')
  await unknown.getByRole('button', { name: /Resync progress: 4\.34% toward/ }).click()
  const unknownDialog = page.getByRole('dialog', { name: 'Resync progress' })
  await expect(unknownDialog.getByText('Last progress: Unknown', { exact: true })).toBeVisible()
  await expect(unknownDialog).not.toContainText(/2026-08-25/)
  await page.keyboard.press('Escape')
  await expect(unknownDialog).toHaveCount(0)
  await expect(unknown).not.toContainText(/2026-08-25/)

  // Health remains an independent green marker, not a renamed resync marker;
  // the labeled Resyncing cue has a visually distinct warning color.
  const healthyColor = await active.getByRole('img', { name: 'Healthy', exact: true }).evaluate(el => getComputedStyle(el).backgroundColor)
  const normalColor = await normal.getByRole('img', { name: 'Healthy', exact: true }).evaluate(el => getComputedStyle(el).backgroundColor)
  const resyncColor = await cue.getByText('Resyncing', { exact: true }).evaluate(el => getComputedStyle(el).color)
  expect(healthyColor).toBe(normalColor)
  expect(resyncColor).not.toBe(healthyColor)

  // Cross-row equal heights and identical region starts: the resync chip lives
  // in the identity status row and reserves the same height in every card.
  const activeBox = await box(active)
  const normalBox = await box(normal)
  expect(Math.abs(activeBox.height - normalBox.height), 'every Home Node card shares one height').toBeLessThanOrEqual(1)
  if (page.viewportSize()!.width >= 640) {
    await expect.poll(async () => Math.abs((await box(active)).y - (await box(normal)).y)).toBeLessThanOrEqual(1)
  }
  const activeTops = await regionTops(active)
  const normalTops = await regionTops(normal)
  for (const region of ['identity', 'resource', 'chain', 'validator'] as const) {
    expect(activeTops[region], region + ' region').not.toBeNull()
    expect(Math.abs((activeTops[region] ?? 0) - (normalTops[region] ?? 0)), region + ' region starts at the same offset').toBeLessThanOrEqual(1)
  }
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
      await statusAndGeometry(page)
      await evidence(page, testInfo, name)
      await progressDialog(page, testInfo, name)
    }
  }
})
