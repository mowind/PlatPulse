import { expect, test } from '@playwright/test'
import { E2E_PASSWORD, expectNoHorizontalOverflow, loginAs, setPageZoom } from './helpers'

test.describe('Phase 1 release-candidate vertical slice', () => {
  test('public projection isolates a private Node and Node detail works at fixed viewports', async ({ page }) => {
    await loginAs(page)
    await expect(page.getByRole('link', { name: 'Node A' })).toBeVisible()
    await expect(page.getByText('Node B (private)', { exact: true })).toHaveCount(0)

    await page.getByRole('link', { name: 'Node A' }).click()
    await expect(page).toHaveURL(/\/nodes\/0195f2a1-0014-4014-8014-000000000014$/)
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText('Current head')).toBeVisible({ timeout: 15_000 })
    await expectNoHorizontalOverflow(page)

    // A guessed private Node URL must be indistinguishable from a missing
    // public representation; no private label or diagnostics may leak.
    await page.goto('/nodes/0195f2a1-0015-4015-8015-000000000015')
    await expect(page.getByRole('alert')).toContainText('resource not found')
    await expect(page.getByText('Node B (private)', { exact: true })).toHaveCount(0)
  })

  test('Public Peer insight exposes bounded summaries without peer identities', async ({ page }) => {
    await loginAs(page)
    const homePeer = page.locator('.peer-insight').first()
    await expect(homePeer).toContainText('Peer insight')
    await expect(homePeer).toContainText('Current')
    await expect(homePeer).toContainText('3')
    await expect(homePeer).toContainText('Inbound')
    await expect(homePeer).toContainText('Outbound')
    await expect(page.getByText('203.0.113.9')).toHaveCount(0)
    await expect(page.getByText('peer-a-inbound')).toHaveCount(0)

    await page.getByRole('link', { name: 'Node A' }).focus()
    await expect(page.getByRole('link', { name: 'Node A' })).toBeFocused()
    await page.keyboard.press('Enter')
    const detailPeer = page.locator('.peer-insight').first()
    await expect(detailPeer).toContainText('Current')
    await expect(detailPeer).toContainText('Consensus')
    await expect(detailPeer).toContainText('3')
    await setPageZoom(page, 2)
    await expectNoHorizontalOverflow(page)
  })

  test('Public and Owner Geo surfaces stay explicit when the database is disabled', async ({ page }) => {
    await loginAs(page)
    const publicGeo = page.locator('.geo-insight').first()
    await expect(publicGeo).toContainText('Peer countries')
    await expect(publicGeo).toContainText('Disabled')
    await expect(publicGeo).toContainText('Country insight is Disabled by the Server')
    await expect(page.getByRole('link', { name: 'PlatON E2E Network' })).toBeVisible()
    await page.getByRole('link', { name: 'PlatON E2E Network' }).click()
    await expect(page.locator('.geo-insight')).toContainText('Disabled')
    await expect(page.locator('.geo-attribution')).toHaveCount(0)

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
    const peerSnapshot = page
      .getByRole('heading', { level: 2, name: 'Peer snapshot' })
      .locator('..')
      .locator('..')
    await expect(peerSnapshot).toContainText('3 peers')
    await expect(page.getByText('203.0.113.9')).toHaveCount(0)
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
