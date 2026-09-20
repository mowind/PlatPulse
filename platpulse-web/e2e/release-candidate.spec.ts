import { expect, test } from '@playwright/test'
import {
  E2E_PASSWORD,
  expectFocusedElementHasVisibleFocus,
  expectMetricRowsAligned,
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
  loginAs,
  openPeerDisclosure,
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
    await expect(page.getByText('Head', { exact: true })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText('Process uptime', { exact: true })).toBeVisible()
    await expect(page.getByLabel('PlatON process resources').getByText('CPU', { exact: true })).toBeVisible()
    // Continuous reading (issue #149) replaces the Details/Network tabs with a
    // single page and keyboard-operable disclosures.
    await expect(page.getByRole('tab')).toHaveCount(0)
    await expect(page.getByRole('heading', { level: 2, name: 'Latest 60 seconds' })).toBeVisible()
    await openPeerDisclosure(page, 'keyboard')
    await expect(page.getByRole('heading', { name: 'Peer history' })).toBeVisible()
    await expectVisibleInteractiveTargets(page)
    await setPageZoom(page, 2)
    await expectNoHorizontalOverflow(page)

    // A guessed retired Node URL must be indistinguishable from a missing
    // public representation; no retired label or diagnostics may leak.
    await page.goto('/nodes/0195f2a1-0017-4017-8017-000000000017')
    await expect(page.getByRole('alert')).toContainText('resource not found')
    await expect(page.getByText('Node D (retired)', { exact: true })).toHaveCount(0)
  })

  test('a deleted Network deep link redirects Home while the Admin action stays icon-only', async ({ page }, testInfo) => {
    await loginAs(page)
    // The Network overview route is gone: a deep link replaces itself with
    // Home, so the deleted page can never render.
    await page.goto('/networks/home-convergence')
    await expect(page).toHaveURL(/\/$/)
    await expect(page.getByRole('region', { name: 'Home' })).toBeVisible({ timeout: 15_000 })

    // An open stream is deliberately silent: the container stays mounted for the
    // transport state, but no positive transport notice is rendered. The empty
    // container has no box, so this is an existence check, not a visibility one.
    await expect(page.locator('[data-realtime-status="connected"]')).toHaveCount(1)
    await expect(page.getByText('Live updates connected')).toHaveCount(0)

    const adminLink = page.getByRole('link', { name: 'Admin', exact: true })
    // Icon-only like the Emerald reference: no visible label, still named for
    // assistive technology, still a full-size target.
    await expect(adminLink).toHaveAttribute('aria-label', 'Admin')
    expect((await adminLink.innerText()).trim(), 'Admin link is icon-only').toBe('')
    await expect(adminLink.locator('svg')).toHaveCount(1)
    const adminBox = await adminLink.boundingBox()
    expect(adminBox?.width).toBeGreaterThanOrEqual(44)
    expect(adminBox?.height).toBeGreaterThanOrEqual(44)
    await adminLink.focus()
    await expect(adminLink).toBeFocused()
    await expectFocusedElementHasVisibleFocus(page)
    await expectNoHorizontalOverflow(page)

    if (testInfo.project.use.hasTouch) {
      await adminLink.tap()
    } else {
      await adminLink.press('Enter')
    }
    await expect(page).toHaveURL(/\/admin$/)
  })

  test('Public Node Detail Peer insight exposes bounded summaries without peer identities', async ({ page }) => {
    await loginAs(page)
    // Home keeps the Network display name as plain text (issue #97), so the
    // whole-card Node link is the only public route into Node Detail.
    const nodeLink = page.getByRole('link', { name: /Node A/ }).first()
    await expect(nodeLink).toBeVisible({ timeout: 15_000 })
    await nodeLink.focus()
    await expect(nodeLink).toBeFocused()
    await page.keyboard.press('Enter')
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible()
    await openPeerDisclosure(page)
    const detailPeer = page.getByRole('region', { name: 'Peer insight' }).last()
    await expect(detailPeer).toContainText('Peer data current')
    await expect(detailPeer).toContainText('Consensus')
    await expect(detailPeer).toContainText('3')
    // Raw peer identities never cross the Public boundary.
    await expect(page.getByText('203.0.113.9')).toHaveCount(0)
    await expect(page.getByText('peer-a-inbound')).toHaveCount(0)
    await setPageZoom(page, 2)
    await expectNoHorizontalOverflow(page)
  })

  test('the Owner Overview carries no Geo panel and Admin Node diagnostics stay redacted', async ({ page }) => {
    await loginAs(page)
    // The Geo provider is shared Server state and other specs enable it. The
    // harness seeds it Disabled, so establish that precondition here instead of
    // depending on another spec's cleanup in the reused Server.
    await page.goto('/admin/settings')
    const geoCard = page
      .getByRole('article')
      .filter({ has: page.getByRole('heading', { name: 'Geo provider' }) })
    await expect(geoCard.getByRole('radio', { checked: true })).toHaveCount(1)
    const disabled = geoCard.getByRole('radio', { name: 'Disabled' })
    if (!(await disabled.isChecked())) {
      await disabled.check()
      await geoCard.getByRole('button', { name: 'Save Geo provider' }).click()
      await expect(geoCard.getByText(/Geo provider is now Disabled/)).toBeVisible()
    }

    // Geo database status is absent from the Owner Overview (issue #93);
    // the Audit/Site Access surface remains the only Admin Geo context.
    await page.goto('/admin')
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

  test('Node Detail reads continuously with grouped diagnostics and six metric charts without bounded history', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: /Node A/ }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible({ timeout: 15_000 })

    await expect(page.getByText('Process uptime')).toBeVisible()
    const observationPanel = page.locator('[data-slot="node-info-group"]').first()
    await expect.poll(() => observationPanel.evaluate((card) => getComputedStyle(card, '::before').content)).toBe('none')
    // Continuous reading (issue #149): every group is visible in one page and
    // no Details/Network tab survives.
    await expect(page.getByRole('tab')).toHaveCount(0)
    for (const label of ['Node key summary', 'Node chain and consensus observations', 'PlatON process resources', 'Node data directory', 'Shared Host resources']) {
      await expect(page.getByLabel(label)).toBeVisible()
    }
    // The accepted A container (issue #151): an uncarded identity block, four
    // summary tiles, and three parallel observation panels with the Node Data
    // directory merged into the process panel.
    await expect(page.locator('[data-slot="node-hero-card"]')).toHaveCount(0)
    await expect(page.locator('[data-slot="node-info-group"]')).toHaveCount(3)
    await expect(page.getByLabel('Node key summary').locator('[data-slot="node-summary-tile"]')).toHaveCount(4)
    const chainGroup = page.getByLabel('Node chain and consensus observations')
    await expect(page.getByLabel('Node key summary').getByText('Head', { exact: true })).toBeVisible()
    await expect(chainGroup.getByText('QC', { exact: true })).toBeVisible()
    await expect(chainGroup.getByText('Locked', { exact: true })).toBeVisible()
    await expect(chainGroup.getByText('Committed', { exact: true })).toBeVisible()
    await expect(chainGroup.getByText('Validator', { exact: true })).toBeVisible()
    await expect(chainGroup.getByText('True', { exact: true })).toHaveCount(1)
    await expect(page.getByText('Server updates arrive as invalidations; REST data stays authoritative.', { exact: true })).toHaveCount(0)
    await expect(page.getByText('RPC, sync, and consensus are current', { exact: true })).toHaveCount(0)
    const resources = page.getByLabel('PlatON process resources')
    for (const label of ['CPU', 'Memory']) {
      await expect(resources.getByText(label, { exact: true })).toBeVisible()
    }
    // Process CPU + process memory + the merged Node Data directory.
    await expect(resources.locator('[data-slot="progress-thin"]')).toHaveCount(3)
    await expectMetricRowsAligned(resources)
    await expectMetricRowsAligned(chainGroup)
    await expect(page.getByRole('heading', { level: 2, name: 'Latest 60 seconds' })).toBeVisible()
    // Six charts in the agreed order, from the fixed metrics response.
    for (const heading of ['Process CPU', 'Process memory', 'Host network', 'Peer connections', 'Block interval', 'Transactions per block']) {
      await expect(page.getByRole('heading', { level: 3, name: heading })).toBeVisible()
    }
    await expect(page.getByText('2.00 s')).toBeVisible()
    const metrics = page.locator('[data-slot="node-metrics-section"]')
    await expect(metrics.getByRole('img', { name: /line chart over the last 60 seconds/ })).toHaveCount(4)
    await expect(metrics.getByRole('img', { name: /bar chart over the last 60 seconds/ })).toHaveCount(2)
    await expect(metrics.locator('[data-slot="node-metric-chart-bar"]')).not.toHaveCount(0)
    await expect(metrics.getByText('60s', { exact: true })).toHaveCount(6)
    const cardSizes = await metrics.locator('[data-slot="node-metric-card"]').evaluateAll((cards) => cards.map((card) => {
      const box = card.getBoundingClientRect()
      return { width: Math.round(box.width), height: Math.round(box.height) }
    }))
    expect(cardSizes).toHaveLength(6)
    expect(new Set(cardSizes.map(({ width }) => width)).size).toBe(1)
    expect(new Set(cardSizes.map(({ height }) => height)).size, JSON.stringify(cardSizes)).toBe(1)
    expect(Math.max(...cardSizes.map(({ height }) => height))).toBeLessThanOrEqual(270)
    await expect(metrics.getByText('No samples in the last minute', { exact: true })).toHaveCount(0)
    await expect.poll(() => metrics.locator('[data-slot="node-metric-card"]').first().evaluate((card) => getComputedStyle(card, '::before').content)).toBe('none')
    await expect(page.getByText('Bounded Block History')).toHaveCount(0)
    await expect(page.getByRole('button', { name: 'Export public history' })).toHaveCount(0)
    await openPeerDisclosure(page)
    await expect(page.getByRole('heading', { name: 'Peer history' })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('reduced-motion preference and keyboard login remain honored', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' })
    await page.goto('/login')
    await expect(page.getByLabel('Username')).toBeFocused()
    await page.getByLabel('Username').fill('admin')
    await page.getByLabel('Password').fill(E2E_PASSWORD)
    await page.keyboard.press('Enter')
    await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
    const reduced = await page.evaluate(() => {
      const style = getComputedStyle(document.documentElement)
      return matchMedia('(prefers-reduced-motion: reduce)').matches && style.scrollBehavior !== 'smooth'
    })
    expect(reduced).toBe(true)
  })
})
