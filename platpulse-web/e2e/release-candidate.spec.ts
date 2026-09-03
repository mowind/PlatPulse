import { expect, test } from '@playwright/test'
import {
  E2E_PASSWORD,
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
  loginAs,
  setPageZoom,
} from './helpers'

test.describe('Phase 1 release-candidate vertical slice', () => {
  test('site-wide projection includes every Active Node and Node detail works at fixed viewports', async ({ page }) => {
    await loginAs(page)
    await expect(page.getByRole('link', { name: /Node A/ })).toBeVisible()
    await expect(page.getByRole('link', { name: /Node B \(private\)/ })).toBeVisible()

    await page.getByRole('link', { name: /Node A/ }).click()
    await expect(page).toHaveURL(/\/nodes\/0195f2a1-0014-4014-8014-000000000014$/)
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText('HEAD', { exact: true })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText('Process uptime', { exact: true })).toBeVisible()
    await expect(page.getByLabel('Node process and storage resources').getByText('CPU', { exact: true })).toBeVisible()
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

    // A guessed retired Node URL must be indistinguishable from a missing
    // public representation; no retired label or diagnostics may leak.
    await page.goto('/nodes/0195f2a1-0017-4017-8017-000000000017')
    await expect(page.getByRole('alert')).toContainText('resource not found')
    await expect(page.getByText('Node D (retired)', { exact: true })).toHaveCount(0)
  })

  test('Public Peer insight exposes bounded summaries without peer identities', async ({ page }) => {
    await loginAs(page)
    // Home's Network display name is plain text (issue #97), so reach the
    // Network overview through the Node Detail breadcrumb.
    await page.goto('/networks/platon-e2e')
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

  test('Public Geo surface stays explicit when the database is disabled; the Owner Overview carries no Geo panel', async ({ page }) => {
    await loginAs(page)
    await page.goto('/networks/platon-e2e')
    await expect(page.getByRole('heading', { level: 1, name: 'PlatON E2E Network' })).toBeVisible()
    const publicGeo = page.getByRole('region', { name: 'Peer countries' }).first()
    await expect(publicGeo).toContainText('Peer countries')
    await expect(publicGeo).toContainText('Disabled')
    await expect(publicGeo).toContainText('Country insight is Disabled by the Server')
    await expect(page.getByText(/GeoLite|MaxMind/i)).toHaveCount(0)

    // Geo database status is absent from the Owner Overview (issue #93);
    // the Audit/Site Access surface remains the only Admin Geo context.
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Geo database' })).toHaveCount(0)
    await expect(page.getByText('Cached countries')).toHaveCount(0)

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
    // The Overview itself carries no forms (issue #93); the active-field
    // contract is exercised on the Admin Node detail rename form, which
    // stays mounted while SSE reconnects and REST refetches run.
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
    const menu = page.getByRole('button', { name: 'Menu' })
    if (await menu.isVisible()) await menu.click()
    await page.getByRole('link', { name: 'Nodes', exact: true }).click()
    await page.getByRole('link', { name: 'Node A' }).click()
    await expect(page.getByRole('heading', { level: 1, name: /Node A/ })).toBeVisible({ timeout: 15_000 })
    await page.getByRole('button', { name: 'Edit' }).click()

    const displayName = page.getByLabel('Display name')
    await displayName.fill('Node A (field stays)')
    await page.waitForTimeout(1_100)
    await expect(displayName).toHaveValue('Node A (field stays)')
    await expectNoHorizontalOverflow(page)
  })

  test('Node Detail keeps resources in the large card and presents four equal metric charts without bounded history', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: /Node A/ }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible({ timeout: 15_000 })

    await expect(page.getByText('Process uptime')).toBeVisible()
    const heroCard = page.locator('.node-hero-card')
    await expect.poll(() => heroCard.evaluate((card) => getComputedStyle(card, '::before').content)).toBe('none')
    const heroLayout = await heroCard.evaluate((card) => ({
      height: Math.round(card.getBoundingClientRect().height),
      children: [...card.children].map((child) => ({ className: child.className, height: Math.round(child.getBoundingClientRect().height) })),
    }))
    expect(heroLayout.height, JSON.stringify(heroLayout)).toBeLessThanOrEqual(500)
    await expect(page.getByText('HEAD')).toBeVisible()
    await expect(page.getByText('QC')).toBeVisible()
    await expect(page.getByText('LOCKED')).toBeVisible()
    await expect(page.getByText('COMMITTED')).toBeVisible()
    await expect(page.getByText('VALIDATOR')).toBeVisible()
    await expect(page.getByText('True', { exact: true })).toBeVisible()
    await expect(page.getByText('Yes', { exact: true })).toHaveCount(0)
    await expect(page.getByText('Server updates arrive as invalidations; REST data stays authoritative.', { exact: true })).toHaveCount(0)
    await expect(page.getByText('RPC, sync, and consensus are current', { exact: true })).toHaveCount(0)
    const resources = page.getByLabel('Node process and storage resources')
    for (const label of ['CPU', 'MEMORY', 'NODE DATA']) {
      await expect(resources.getByText(label, { exact: true })).toBeVisible()
    }
    await expect(resources.locator('.node-hero-resource-progress')).toHaveCount(3)
    for (const heading of ['Network', 'Connections', 'Block time', 'Transactions']) {
      await expect(page.getByRole('heading', { level: 3, name: heading })).toBeVisible()
    }
    await expect(page.getByText('2.00 s')).toBeVisible()
    const detailsPanel = page.getByRole('tabpanel', { name: 'Details' })
    await expect(detailsPanel.getByRole('img', { name: /line chart over the last minute/ })).toHaveCount(2)
    await expect(detailsPanel.getByRole('img', { name: /bar chart over the last minute/ })).toHaveCount(2)
    await expect(detailsPanel.locator('.node-metric-chart-bar')).not.toHaveCount(0)
    await expect(detailsPanel.getByText('1m', { exact: true })).toHaveCount(4)
    const cardSizes = await detailsPanel.locator('.node-metric-card').evaluateAll((cards) => cards.map((card) => {
      const box = card.getBoundingClientRect()
      return { width: Math.round(box.width), height: Math.round(box.height) }
    }))
    expect(cardSizes).toHaveLength(4)
    expect(new Set(cardSizes.map(({ width }) => width)).size).toBe(1)
    expect(new Set(cardSizes.map(({ height }) => height)).size, JSON.stringify(cardSizes)).toBe(1)
    expect(Math.max(...cardSizes.map(({ height }) => height))).toBeLessThanOrEqual(270)
    await expect(detailsPanel.getByText('No samples in the last minute', { exact: true })).toHaveCount(0)
    await expect.poll(() => detailsPanel.locator('.node-metric-card').first().evaluate((card) => getComputedStyle(card, '::before').content)).toBe('none')
    await expect(page.getByText('Bounded Block History')).toHaveCount(0)
    await expect(page.getByRole('button', { name: 'Export public history' })).toHaveCount(0)
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
