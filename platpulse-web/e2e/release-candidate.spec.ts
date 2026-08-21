import { expect, test } from '@playwright/test'
import {
  E2E_PASSWORD,
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
  loginAs,
  setPageZoom,
} from './helpers'

test.describe('Phase 1 release-candidate vertical slice', () => {
  test('public projection isolates a private Node and Node detail works at fixed viewports', async ({ page }) => {
    await loginAs(page)
    await expect(page.getByRole('link', { name: 'Node A' })).toBeVisible()
    await expect(page.getByText('Node B (private)', { exact: true })).toHaveCount(0)

    await page.getByRole('link', { name: 'Node A' }).click()
    await expect(page).toHaveURL(/\/nodes\/0195f2a1-0014-4014-8014-000000000014$/)
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText('Current Head', { exact: true }).first()).toBeVisible({ timeout: 15_000 })
    await expect(page.getByRole('heading', { level: 2, name: 'Node Health Summary' })).toBeVisible()
    await expect(page.getByText('Reference Confidence', { exact: true })).toBeVisible()
    await expect(page.getByRole('tab', { name: 'Details' })).toHaveAttribute('aria-selected', 'true')
    await page.getByRole('tab', { name: 'Details' }).focus()
    await page.keyboard.press('ArrowRight')
    await expect(page.getByRole('tab', { name: 'Network' })).toHaveAttribute('aria-selected', 'true')
    await expect(page.getByRole('tabpanel', { name: 'Network' })).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Peer history' })).toBeVisible()
    await page.getByRole('tab', { name: 'Network' }).press('ArrowLeft')
    await expect(page.getByRole('tab', { name: 'Details' })).toHaveAttribute('aria-selected', 'true')
    await expect(page.getByRole('tabpanel', { name: 'Details' })).toBeVisible()
    const undersized = await page.getByRole('tablist').locator('button').evaluateAll((elements) => elements.flatMap((element) => {
      const rect = element.getBoundingClientRect()
      return rect.width < 44 || rect.height < 44 ? [element.textContent?.trim() || 'tab'] : []
    }))
    expect(undersized, 'Node tabs must be at least 44px').toEqual([])
    await expectVisibleInteractiveTargets(page)
    await setPageZoom(page, 2)
    await expectNoHorizontalOverflow(page)

    // A guessed private Node URL must be indistinguishable from a missing
    // public representation; no private label or diagnostics may leak.
    await page.goto('/nodes/0195f2a1-0015-4015-8015-000000000015')
    await expect(page.getByRole('alert')).toContainText('resource not found')
    await expect(page.getByText('Node B (private)', { exact: true })).toHaveCount(0)
  })

  test('Public Peer insight exposes bounded summaries without peer identities', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'PlatON E2E Network' }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'PlatON E2E Network' })).toBeVisible()
    const networkPeer = page.getByRole('region', { name: 'Peer insight' }).first()
    await expect(networkPeer).toContainText('Peer insight')
    await expect(networkPeer).toContainText('Current')
    await expect(networkPeer).toContainText('3')
    await expect(networkPeer).toContainText('Inbound')
    await expect(networkPeer).toContainText('Outbound')
    await expect(page.getByText('203.0.113.9')).toHaveCount(0)
    await expect(page.getByText('peer-a-inbound')).toHaveCount(0)

    const nodeLink = page.getByRole('link', { name: 'Node A' })
    await nodeLink.focus()
    await expect(nodeLink).toBeFocused()
    await page.keyboard.press('Enter')
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible()
    await page.getByRole('tab', { name: 'Network' }).click()
    const detailPeer = page.getByRole('region', { name: 'Peer insight' }).last()
    await expect(detailPeer).toContainText('Current')
    await expect(detailPeer).toContainText('Consensus')
    await expect(detailPeer).toContainText('3')
    await setPageZoom(page, 2)
    await expectNoHorizontalOverflow(page)
  })

  test('Public and Owner Geo surfaces stay explicit when the database is disabled', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'PlatON E2E Network' }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'PlatON E2E Network' })).toBeVisible()
    const publicGeo = page.getByRole('region', { name: 'Peer countries' }).first()
    await expect(publicGeo).toContainText('Peer countries')
    await expect(publicGeo).toContainText('Disabled')
    await expect(publicGeo).toContainText('Country insight is Disabled by the Server')
    await expect(page.getByText(/GeoLite|MaxMind/i)).toHaveCount(0)

    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
    const geoHeading = page.getByRole('heading', { level: 2, name: 'Geo database' })
    const geoStatus = geoHeading.locator('..').locator('..')
    await expect(geoStatus).toContainText('Disabled')
    await expect(geoStatus).toContainText('Configured')
    await expect(geoStatus).toContainText('No')

    // Owner current Peer diagnostics are available, but raw peer addresses
    // remain outside the Admin DTO as well as the Public projection.
    const menu = page.getByRole('button', { name: 'Menu' })
    if (await menu.isVisible()) await menu.click()
    await page.getByRole('link', { name: 'Nodes', exact: true }).click()
    await page.getByRole('link', { name: 'Node A' }).click()
    await expect(page.getByRole('heading', { level: 1, name: /Node A/ })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'RPC diagnostics' })).toBeVisible()
    await expect(page.getByText('Redacted RPC Endpoint')).toBeVisible()
    await expect(page.getByText('platon/1.5.1')).toBeVisible()
    await expect(page.getByText('203.0.113.9')).toHaveCount(0)
    await expect(page.getByText('peer-a-inbound')).toHaveCount(0)
    await expectVisibleInteractiveTargets(page)
    await expectNoHorizontalOverflow(page)
  })
  test('Owner diagnostics and SSE reconnect do not disturb an active form field', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
    await expect(page.getByRole('button', { name: 'Node A' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByRole('button', { name: 'Node B (private)' })).toBeVisible({ timeout: 15_000 })

    const nodeId = page.getByLabel('Node ID')
    await nodeId.fill('0195f2a1-0014-4014-8014-000000000014')
    await page.waitForTimeout(1_100)
    await expect(nodeId).toHaveValue('0195f2a1-0014-4014-8014-000000000014')
    await expectNoHorizontalOverflow(page)
  })

  test('public history export downloads the bounded JSON representation', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'Node A' }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible({ timeout: 15_000 })

    const downloadPromise = page.waitForEvent('download')
    await page.getByRole('button', { name: 'Export public history' }).click()
    const download = await downloadPromise
    expect(download.suggestedFilename()).toBe('public-history.json')
    const json = JSON.parse(await download.createReadStream().then(async (stream) => {
      if (!stream) throw new Error('history export stream was unavailable')
      const chunks: Buffer[] = []
      for await (const chunk of stream) chunks.push(Buffer.from(chunk))
      return Buffer.concat(chunks).toString('utf8')
    })) as Array<{ nodeId?: string; height?: number }>
    expect(json.length).toBeGreaterThan(0)
    expect(json.every((entry) => entry.nodeId === '0195f2a1-0014-4014-8014-000000000014')).toBe(true)
    expect(json.every((entry) => typeof entry.height === 'number')).toBe(true)
  })

  test('public history export reports a safe partial-failure state', async ({ page }) => {
    await loginAs(page)
    await page.goto('/nodes/0195f2a1-0014-4014-8014-000000000014')
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible({ timeout: 15_000 })
    await page.route('**/api/public/v1/nodes/*/history/export**', (route) => route.fulfill({
      status: 503,
      contentType: 'application/json',
      body: JSON.stringify({ error: { code: 'unavailable', message: 'history export unavailable' } }),
    }))

    await page.getByRole('button', { name: 'Export public history' }).click()
    await expect(page.getByRole('alert')).toContainText('Unable to export block history')
    await expect(page.getByRole('button', { name: 'Export public history' })).toBeEnabled()
  })

  test('reduced-motion preference and keyboard login remain honored', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' })
    await page.goto('/login')
    await expect(page.getByLabel('Username')).toBeFocused()
    await page.getByLabel('Username').fill('admin')
    await page.getByLabel('Password').fill(E2E_PASSWORD)
    await page.keyboard.press('Enter')
    await expect(page.getByRole('heading', { level: 1, name: 'Home' })).toBeVisible()
    const reduced = await page.evaluate(() => {
      const style = getComputedStyle(document.documentElement)
      return matchMedia('(prefers-reduced-motion: reduce)').matches && style.scrollBehavior !== 'smooth'
    })
    expect(reduced).toBe(true)
  })
})
