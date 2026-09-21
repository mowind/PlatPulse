import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test'
import type { PublicNetwork, PublicNode } from '../src/api/generated'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

// Exercise the routed Public Home seam. Keep the real authenticated Server and
// its response shape; only replace the Public Nodes needed for this scenario.
const LAST_PROGRESS = '2026-08-25T02:00:00+02:00'
const UTC_PROGRESS = '2026-08-25T00:00:00.000Z'
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

async function statusAndGeometry(page: Page) {
  // Move away from the card before measuring: its existing hover lift must not
  // be mistaken for different grid alignment after a dialog interaction.
  await page.mouse.move(0, 0)
  const active = nodeCard(page, 'A Resync')
  const normal = nodeCard(page, 'B Normal')
  const progress = active.getByRole('group', { name: 'Resync progress', exact: true })
  await expect(progress.getByText('Resyncing', { exact: true })).toBeVisible()
  await expect(progress.getByText('4.34%', { exact: true })).toBeVisible()
  await expect(progress.getByText('6,920,136 / 159,311,799', { exact: true })).toBeVisible()
  await expect(progress.getByRole('button', { name: 'Last progress: 2 minutes ago', exact: true })).toBeVisible()
  await expect(active).not.toContainText(/2026-08-25|T00:00:00|T02:00:00/)
  await expect(active.getByRole('img', { name: 'Healthy', exact: true })).toBeVisible()
  await expect(normal.getByRole('img', { name: 'Healthy', exact: true })).toBeVisible()
  await expect(normal.getByRole('group', { name: 'Resync progress' })).toHaveCount(0)
  await expect(normal.getByRole('button', { name: /Last progress/ })).toHaveCount(0)
  await expect(normal).not.toContainText(/Resyncing|No active resync|last progress/i)
  for (const label of ['CPU', 'Memory', 'Head', 'Txs', 'Peers']) {
    await expect(normal.getByText(label, { exact: true })).toBeVisible()
  }
  const unknown = nodeCard(page, 'C Unknown progress').getByRole('group', { name: 'Resync progress' })
  await expect(unknown.getByText('Last progress: Unknown', { exact: true })).toBeVisible()
  await expect(unknown.getByRole('button', { name: /Last progress/ })).toHaveCount(0)
  await expect(unknown).not.toContainText(/2026-08-25/)

  // Health remains an independent green marker, not a renamed resync marker;
  // the labeled Resyncing cue has a visually distinct warning color.
  const healthyColor = await active.getByRole('img', { name: 'Healthy', exact: true }).evaluate(el => getComputedStyle(el).backgroundColor)
  const normalColor = await normal.getByRole('img', { name: 'Healthy', exact: true }).evaluate(el => getComputedStyle(el).backgroundColor)
  const resyncColor = await progress.getByText('Resyncing', { exact: true }).evaluate(el => getComputedStyle(el).color)
  expect(healthyColor).toBe(normalColor)
  expect(resyncColor).not.toBe(healthyColor)

  const heading = await box(active.getByRole('heading', { name: 'A Resync', exact: true }))
  const identity = await box(active.getByRole('button', { name: 'Node identity details', exact: true }))
  const status = await box(progress)
  const resources = await box(active.getByLabel('Node process and host network resources', { exact: true }))
  for (const headerPart of [heading, identity]) {
    expect(status.y).toBeGreaterThanOrEqual(headerPart.y + headerPart.height - 1)
  }
  expect(resources.y).toBeGreaterThanOrEqual(status.y + status.height - 1)
  const activeBox = await box(active)
  const normalBox = await box(normal)
  expect(activeBox.height).toBeGreaterThan(normalBox.height + 10)
  if (page.viewportSize()!.width >= 640) {
    await expect.poll(async () => Math.abs((await box(active)).y - (await box(normal)).y)).toBeLessThanOrEqual(1)
  }
  for (const bounds of [heading, identity, status, resources]) {
    expect(bounds.x).toBeGreaterThanOrEqual(activeBox.x - 1)
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(activeBox.x + activeBox.width + 1)
  }
  await expectNoHorizontalOverflow(page)
}

async function progressDialog(page: Page, testInfo: TestInfo, name: string) {
  const active = nodeCard(page, 'A Resync')
  const trigger = active.getByRole('button', { name: 'Last progress: 2 minutes ago', exact: true })
  const dialog = page.getByRole('dialog', { name: 'Last resync progress', exact: true })
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
  await expect(dialog.getByText('25 Aug 2026, 00:00:00 UTC', { exact: true })).toBeVisible()
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
