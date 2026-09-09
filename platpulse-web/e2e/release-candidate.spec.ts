import { expect, test } from '@playwright/test'
import {
  E2E_PASSWORD,
  expectFocusedElementHasVisibleFocus,
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
    await expect(page.getByText('Head', { exact: true })).toBeVisible({ timeout: 15_000 })
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

  test('Public Network header separates live transport from observation status', async ({ page }, testInfo) => {
    await loginAs(page)
    await page.goto('/networks/home-convergence')

    await expect(page.getByRole('heading', { level: 1, name: 'Home Convergence Network With An Extremely Long Display Name' })).toBeVisible()
    const metadata = page.getByLabel('Network identity and live updates')
    await expect(metadata).toContainText('Network key')
    await expect(metadata).toContainText('home-convergence')
    await expect(metadata.getByRole('status', { name: 'Live updates connected' })).toBeVisible()

    const adminLink = page.getByRole('link', { name: 'Admin', exact: true })
    await expect(adminLink).toContainText('Admin', { useInnerText: true })
    const adminBox = await adminLink.boundingBox()
    expect(adminBox?.width).toBeGreaterThanOrEqual(44)
    expect(adminBox?.height).toBeGreaterThanOrEqual(44)
    await adminLink.focus()
    await expect(adminLink).toBeFocused()
    await expectNoHorizontalOverflow(page)

    if (page.viewportSize()?.width === 360) {
      const keyBox = await metadata.getByText('Network key', { exact: false }).boundingBox()
      const statusBox = await metadata.getByRole('status', { name: 'Live updates connected' }).boundingBox()
      expect(keyBox).not.toBeNull()
      expect(statusBox).not.toBeNull()
      expect(statusBox!.y).toBeGreaterThan(keyBox!.y + keyBox!.height - 1)
    }

    if (testInfo.project.use.hasTouch) {
      await adminLink.tap()
    } else {
      await adminLink.press('Enter')
    }
    await expect(page).toHaveURL(/\/admin$/)
  })

  test('Public Network Node cards keep compact facts, UTC receipt time, and touch-safe links', async ({ page }, testInfo) => {
    await loginAs(page)
    const longNodeName = 'Node H — Producing Card With A Very Long Display Name That Must Not Overflow'
    const longHealthReason = 'ServerOwnedHealthReasonThatMustWrapWithoutTruncationAtNarrowWidths0123456789abcdefghijklmnopqrstuvwxyz'
    await page.route('**/api/public/v1/networks/home-convergence', async (route) => {
      const response = await route.fetch()
      const payload = (await response.json()) as {
        nodes: Array<{ nodeId: string; healthReason?: string | null }>
      }
      await route.fulfill({
        response,
        body: JSON.stringify({
          ...payload,
          nodes: payload.nodes.map((node) =>
            node.nodeId === '0195f2a1-0062-4062-8062-000000000062'
              ? { ...node, healthReason: longHealthReason }
              : node,
          ),
        }),
      })
    })
    await page.route('**/api/public/v1/nodes/0195f2a1-0060-4060-8060-000000000060', async (route) => {
      const response = await route.fetch()
      const payload = (await response.json()) as Record<string, unknown>
      await route.fulfill({
        response,
        body: JSON.stringify({
          ...payload,
          displayName: longNodeName,
          health: 'unhealthy',
          healthReason: longHealthReason,
        }),
      })
    })
    await page.goto('/networks/home-convergence')

    await expect(page.getByRole('heading', { level: 1, name: 'Home Convergence Network With An Extremely Long Display Name' })).toBeVisible()
    const hCard = page.getByRole('article', { name: /Node H/ })
    await expect(hCard).toBeVisible()
    const identityGroup = hCard.getByRole('group', { name: 'Node identity and health' })
    const summaryGroup = hCard.getByRole('group', { name: 'Node summary facts' })
    const componentGroup = hCard.getByRole('group', { name: 'Node component status' })
    await expect(identityGroup).toContainText(longNodeName)
    await expect(identityGroup).toContainText('Healthy')
    await expect(summaryGroup).toHaveText(/Head[\s\S]*Peers[\s\S]*Oldest component update/)
    await expect(componentGroup).toHaveText(/RPC[\s\S]*Sync[\s\S]*Consensus/)
    const [identityBox, summaryBox, componentBox, detailsBox] = await Promise.all([
      identityGroup.boundingBox(),
      summaryGroup.boundingBox(),
      componentGroup.boundingBox(),
      hCard.getByRole('link', { name: 'View details' }).boundingBox(),
    ])
    expect(identityBox).not.toBeNull()
    expect(summaryBox).not.toBeNull()
    expect(componentBox).not.toBeNull()
    expect(detailsBox).not.toBeNull()
    expect(summaryBox!.y).toBeGreaterThanOrEqual(identityBox!.y)
    expect(componentBox!.y).toBeGreaterThanOrEqual(summaryBox!.y)
    expect(detailsBox!.y).toBeGreaterThanOrEqual(componentBox!.y)

    const hTitleLink = hCard.getByRole('link', { name: /Node H/ })
    await expect(hTitleLink).toHaveText(longNodeName)
    await expect(hTitleLink).toHaveAttribute('href', '/nodes/0195f2a1-0060-4060-8060-000000000060')
    const detailsLink = hCard.getByRole('link', { name: 'View details' })
    await expect(detailsLink).toHaveAttribute('href', '/nodes/0195f2a1-0060-4060-8060-000000000060')
    await expect(detailsLink).toHaveText(/View details →/)

    await expect(hCard).toContainText('Head')
    await expect(hCard).toContainText('Peers')
    await expect(hCard).toContainText(/inbound/i)
    await expect(hCard).toContainText(/outbound/i)
    await expect(hCard).toContainText('Oldest component update')
    await expect(hCard).toContainText('Earliest Server receipt across RPC, Sync, and Consensus')
    const timeGroup = hCard.getByText('Earliest Server receipt across RPC, Sync, and Consensus', { exact: true }).locator('..')
    await expect(timeGroup).not.toContainText('Current')
    await expect(hCard.getByLabel('Node component status')).toContainText('RPC')
    await expect(hCard.getByLabel('Node component status')).toContainText('Sync')
    await expect(hCard.getByLabel('Node component status')).toContainText('Consensus')
    await expect(hCard.getByText('Current observation')).toHaveCount(0)
    await expect(hCard.getByText('Last observed')).toHaveCount(0)
    await expect(hCard.locator('time')).toHaveCount(2)
    await expect(hCard.locator('time').first()).toHaveAttribute('dateTime', /T\d{2}:\d{2}:\d{2}Z$/)
    await expect(hCard.locator('time').first()).toHaveAttribute('aria-label', /UTC/)
    await expect(hCard.locator('time').nth(1)).toContainText(/\d{1,2} \w+ \d{4}.*UTC/)

    // The API fixture deliberately gives RPC the oldest receipt while the
    // Agent report is newer; the public aggregate must retain that ordering.
    const networkResponse = await page.request.get('/api/public/v1/networks/home-convergence')
    expect(networkResponse.ok()).toBe(true)
    const networkPayload = (await networkResponse.json()) as {
      nodes: Array<{ nodeId: string; freshness?: string | null; lastReportAt?: string | null }>
    }
    const hNode = networkPayload.nodes.find((node) => node.nodeId === '0195f2a1-0060-4060-8060-000000000060')
    if (!hNode?.freshness || !hNode.lastReportAt) throw new Error('Node H receipt timestamps are missing')
    const reportLeadMs = Date.parse(hNode.lastReportAt) - Date.parse(hNode.freshness)
    expect(reportLeadMs).toBeGreaterThan(40_000)
    await expect(hCard.locator('time').first()).toHaveAttribute('dateTime', hNode.freshness)

    // The long public Node name remains visible inside its card without
    // creating page overflow at any fixed viewport.
    const hCardBox = (await hCard.boundingBox())!
    const hTitleBox = (await hTitleLink.boundingBox())!
    expect(hTitleBox.x).toBeGreaterThanOrEqual(hCardBox.x)
    expect(hTitleBox.x + hTitleBox.width).toBeLessThanOrEqual(hCardBox.x + hCardBox.width)

    const lCard = page.getByRole('article', { name: /Node L/ })
    await expect(lCard).toBeVisible()
    const lReason = lCard.getByText(longHealthReason, { exact: true })
    await expect(lReason).toBeVisible()
    const lReasonLayout = await lReason.evaluate((element) => ({
      clientWidth: element.clientWidth,
      scrollWidth: element.scrollWidth,
      clientHeight: element.clientHeight,
      scrollHeight: element.scrollHeight,
    }))
    expect(lReasonLayout.clientWidth).toBeGreaterThan(0)
    expect(lReasonLayout.scrollWidth).toBeLessThanOrEqual(lReasonLayout.clientWidth)
    expect(lReasonLayout.scrollHeight).toBeLessThanOrEqual(lReasonLayout.clientHeight)
    await expect(lCard.getByText('Oldest component update', { exact: true })).toBeVisible()
    await expect(lCard.locator('time')).toHaveCount(2)

    const pCard = page.getByRole('article', { name: /Node P/ })
    await expect(pCard).toBeVisible()
    await expect(pCard.getByText('Oldest component update', { exact: true })).toBeVisible()
    await expect(pCard.getByText('RPC, Sync, and Consensus receipt time is unavailable.', { exact: true })).toBeVisible()
    await expect(pCard).toContainText('Unknown')
    await expect(pCard.locator('time')).toHaveCount(0)

    await expectVisibleInteractiveTargets(page)
    await expectNoHorizontalOverflow(page)

    const activate = async (link: ReturnType<typeof hCard.getByRole>) => {
      await link.focus()
      await expectFocusedElementHasVisibleFocus(page)
      if (testInfo.project.use.hasTouch) {
        await link.tap()
      } else {
        await page.keyboard.press('Enter')
      }
    }

    await activate(hTitleLink)
    await expect(page).toHaveURL(/\/nodes\/0195f2a1-0060-4060-8060-000000000060$/)
    await expect(page.getByRole('heading', { level: 1, name: /Node H/ })).toHaveText(longNodeName)
    const detailReason = page.getByText(longHealthReason, { exact: true })
    await expect(detailReason).toBeVisible()
    const detailReasonLayout = await detailReason.evaluate((element) => ({
      clientWidth: element.clientWidth,
      scrollWidth: element.scrollWidth,
      clientHeight: element.clientHeight,
      scrollHeight: element.scrollHeight,
    }))
    expect(detailReasonLayout.clientWidth).toBeGreaterThan(0)
    expect(detailReasonLayout.scrollWidth).toBeLessThanOrEqual(detailReasonLayout.clientWidth)
    expect(detailReasonLayout.scrollHeight).toBeLessThanOrEqual(detailReasonLayout.clientHeight)
    const lastReport = page.getByText('Last report', { exact: true }).locator('..')
    await expect(lastReport).toContainText('UTC')
    await expectNoHorizontalOverflow(page)

    await page.goto('/networks/home-convergence')
    const reloadedCard = page.getByRole('article', { name: /Node H/ })
    await expect(reloadedCard).toBeVisible()
    await activate(reloadedCard.getByRole('link', { name: 'View details' }))
    await expect(page).toHaveURL(/\/nodes\/0195f2a1-0060-4060-8060-000000000060$/)
    await expect(page.getByRole('heading', { level: 1, name: /Node H/ })).toHaveText(longNodeName)
    await expectNoHorizontalOverflow(page)
  })

  test('Public Peer insight exposes bounded summaries without peer identities', async ({ page }) => {
    await loginAs(page)
    // Home's Network display name is plain text (issue #97), so reach the
    // Network overview through the Node Detail breadcrumb.
    await page.goto('/networks/platon-e2e')
    await expect(page.getByRole('heading', { level: 1, name: 'PlatON E2E Network' })).toBeVisible()
    const networkPeer = page.getByRole('region', { name: 'Peer insight' }).first()
    await expect(networkPeer).toContainText('Peer insight')
    await expect(networkPeer.getByRole('group', { name: 'Primary peer counts' })).toContainText('Peers')
    await expect(networkPeer.getByRole('group', { name: 'Primary peer counts' })).toContainText('Inbound')
    await expect(networkPeer.getByRole('group', { name: 'Primary peer counts' })).toContainText('Outbound')
    await expect(networkPeer.getByRole('group', { name: 'Secondary peer counts' })).toContainText('Trusted')
    await expect(networkPeer.getByRole('group', { name: 'Secondary peer counts' })).toContainText('Static')
    await expect(networkPeer.getByRole('group', { name: 'Secondary peer counts' })).toContainText('Consensus')
    // Network aggregation is Unknown when any Active Node has never produced
    // a successful Peer Snapshot; Node A's known value is not a complete total.
    await expect(networkPeer).toContainText('Unknown')
    await expect(networkPeer).toContainText('No successful Peer snapshot is available')
    await expect(networkPeer.getByText('3', { exact: true })).toHaveCount(0)
    await expect(page.getByText('203.0.113.9')).toHaveCount(0)
    await expect(page.getByText('peer-a-inbound')).toHaveCount(0)

    const nodeLink = page.getByRole('link', { name: 'Node A' })
    await nodeLink.focus()
    await expect(nodeLink).toBeFocused()
    await page.keyboard.press('Enter')
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible()
    await page.getByRole('tab', { name: 'Network' }).click()
    const detailPeer = page.getByRole('region', { name: 'Peer insight' }).last()
    await expect(detailPeer).toContainText('Peer data current')
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
    await expect(page.getByText('Head')).toBeVisible()
    await expect(page.getByText('QC')).toBeVisible()
    await expect(page.getByText('Locked')).toBeVisible()
    await expect(page.getByText('Committed')).toBeVisible()
    await expect(page.getByText('Validator')).toBeVisible()
    const validatorRole = page.getByLabel('Validator role')
    await expect(validatorRole.getByText('True', { exact: true })).toHaveCount(1)
    await expect(page.getByText('Server updates arrive as invalidations; REST data stays authoritative.', { exact: true })).toHaveCount(0)
    await expect(page.getByText('RPC, sync, and consensus are current', { exact: true })).toHaveCount(0)
    const resources = page.getByLabel('Node process and storage resources')
    for (const label of ['CPU', 'Memory', 'Node data']) {
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
    await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
    const reduced = await page.evaluate(() => {
      const style = getComputedStyle(document.documentElement)
      return matchMedia('(prefers-reduced-motion: reduce)').matches && style.scrollBehavior !== 'smooth'
    })
    expect(reduced).toBe(true)
  })
})
