import { expect, test } from '@playwright/test'
import {
  E2E_VIEWER_PASSWORD,
  E2E_VIEWER_USERNAME,
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
  loginAs,
  setPageZoom,
} from './helpers'

/**
 * Agent inventory and detail (PAGE-ADMIN-AGENTS, PAGE-ADMIN-AGENT-DETAIL).
 * Enrollment, recovery, and rotation are outside this summary slice; this
 * suite covers the retained identity, liveness, credential, inventory, and
 * diagnostic evidence surface without adding alternate actions.
 *
 * Read-only flows run on every fixed viewport project. Mutations create
 * Server state, so each mutation runs once on desktop-1280 only (same
 * discipline as the Owner Overview visibility mutation).
 */
const AGENT_ID = '0195f2a1-0011-4011-8011-000000000011'
const NO_HOST_AGENT_ID = '0195f2a1-0021-4021-8021-000000000021'
const CREDENTIAL_ID = '0195f2a1-0021-4021-8021-000000000021'

async function openAgents(page: Parameters<typeof loginAs>[0]) {
  await loginAs(page)
  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
  // Tablet/mobile navigation lives in the drawer; desktop shows the sidebar.
  const menu = page.getByRole('button', { name: 'Menu' })
  if (await menu.isVisible()) await menu.click()
  await page.getByRole('link', { name: 'Agents', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Agents' })).toBeVisible()
}

test.describe('Agent inventory and detail (PAGE-ADMIN-AGENTS)', () => {
  test('shows the exact six-column priority summary and accessible identity controls', async ({ page }, testInfo) => {
    await openAgents(page)

    const row = page.locator('tr').filter({ has: page.locator('a[href="/admin/agents/' + AGENT_ID + '"]') })
    await expect(row).toBeVisible({ timeout: 15_000 })
    const headers = await page.locator('table.agent-table thead th').allTextContents()
    expect(headers).toEqual([
      'Agent',
      'Reporting status',
      'Last received',
      'Node Inventory',
      'Credentials',
      'Diagnostics',
    ])
    expect(headers).not.toContain('Epoch')
    expect(headers).not.toContain('Boot / shutdown')
    expect(headers).not.toContain('View')

    await expect(row).toContainText('Current', { timeout: 15_000 })
    await expect(row).toContainText('Server liveness')
    const receipt = row.locator('td[data-label="Last received"]')
    await expect(receipt).toContainText('Server receipt time')
    await expect(receipt).not.toContainText(/Never received|Unknown/)
    await expect(row).toContainText(/6 retained Nodes?/)
    await expect(row).not.toContainText(/Active Nodes|healthy Nodes/)
    await expect(row).toContainText(/1 active/)
    await expect(row).toContainText('Recorded gap intervals')
    await expect(row).toContainText('Accumulated recorded security events')
    await expect(row).toContainText('Recorded evidence')
    await expect(row).toContainText('Queued reports')
    await expect(row).toContainText('0')
    await expect(row).toContainText('Delivery state')
    await expect(row).toContainText('Idle')
    await expect(row).toContainText('Store fatal')
    await expect(row).toContainText('No')
    await expect(row).toContainText('Dropped sequence range')
    await expect(row).toContainText('#40–#42 recorded')
    await expect(row).toContainText('Delivery error')
    await expect(row).toContainText('Host snapshot')
    await expect(row).not.toContainText('delivery timeout retained after bounded retry')
    if ((page.viewportSize()?.width ?? 1280) < 768) {
      const diagnosticCell = row.locator('td[data-label="Diagnostics"]')
      await expect(diagnosticCell).toHaveCSS('flex-direction', 'column')
      await expect(diagnosticCell.locator('.agent-diagnostic-evidence')).toBeVisible()
    }

    const noHostRow = page.locator('tr').filter({ has: page.locator('a[href="/admin/agents/' + NO_HOST_AGENT_ID + '"]') })
    await expect(noHostRow).toBeVisible({ timeout: 15_000 })
    await expect(noHostRow).toContainText('Host observation')
    await expect(noHostRow).toContainText('Not observed yet')
    await expect(noHostRow).toContainText('No Host snapshot')
    await expect(noHostRow).not.toContainText('Spool observation')

    if (page.viewportSize()?.width === 768) {
      const headersReadable = await page.locator('table.agent-table thead th').evaluateAll((cells) =>
        cells.every((cell) => {
          const element = cell as HTMLElement
          return element.getBoundingClientRect().height > 0 && element.scrollWidth <= element.clientWidth + 1
        }),
      )
      const cellsReadable = await row.locator('[data-label]').evaluateAll((cells) =>
        cells.every((cell) => {
          const element = cell as HTMLElement
          return element.getBoundingClientRect().height > 0 && element.scrollWidth <= element.clientWidth + 1
        }),
      )
      expect(headersReadable).toBe(true)
      expect(cellsReadable).toBe(true)
    }
    if ((page.viewportSize()?.width ?? 1280) <= 390) {
      const cardOrder = await row.locator('[data-label]').evaluateAll((cells) =>
        cells
          .map((cell) => ({ label: cell.getAttribute('data-label'), top: cell.getBoundingClientRect().top }))
          .sort((left, right) => left.top - right.top)
          .map((cell) => cell.label),
      )
      expect(cardOrder).toEqual([
        'Agent',
        'Reporting status',
        'Last received',
        'Diagnostics',
        'Node Inventory',
        'Credentials',
      ])
    }

    const agentLink = row.locator('a[href="/admin/agents/' + AGENT_ID + '"]')
    await expect(agentLink).toContainText('0195f2a1…0011')
    const reveal = row.getByRole('button', { name: 'Show full Agent ID' })
    if (testInfo.project.name.includes('touch')) {
      await reveal.tap()
    } else {
      await agentLink.focus()
      await page.keyboard.press('Tab')
      await expect(reveal).toBeFocused()
      await expectFocusedElementHasVisibleFocus(page)
      await page.keyboard.press('Enter')
    }
    await expect(row.getByText(AGENT_ID, { exact: true })).toBeVisible()
    const copy = row.getByRole('button', { name: 'Copy Agent ID' })
    await expect(copy).toBeVisible()
    await page.evaluate(() => {
      Object.defineProperty(navigator, 'clipboard', {
        configurable: true,
        value: { writeText: async () => undefined },
      })
    })
    if (testInfo.project.name.includes('touch')) {
      await copy.tap()
    } else {
      await copy.click()
    }
    await expect(row).toContainText('Copied to clipboard.')
    await expectVisibleInteractiveTargets(page)

    // Enrollment is deferred; no unavailable action is exposed in the summary.
    await expect(page.getByRole('link', { name: 'Enroll a new Agent' })).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })

  test('keeps the summary usable at 200 percent zoom and ultrawide width', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'zoom matrix runs once')
    await openAgents(page)
    // Browser chrome is unavailable in headless Playwright: combine the
    // reflow-equivalent CSS viewport with Chromium's actual page scale.
    await setPageZoom(page, 2)
    const cdp = await page.context().newCDPSession(page)
    await cdp.send('Emulation.setPageScaleFactor', { pageScaleFactor: 2 })
    try {
      await expectNoHorizontalOverflow(page)
      await expectVisibleInteractiveTargets(page)
    } finally {
      await cdp.send('Emulation.setPageScaleFactor', { pageScaleFactor: 1 })
    }
    await page.setViewportSize({ width: 2560, height: 800 })
    await expect(page.locator('table.agent-table thead th')).toHaveCount(6)
    await expectNoHorizontalOverflow(page)
  })
})

test.describe('Agent detail (PAGE-ADMIN-AGENT-DETAIL)', () => {
  test('keeps identity, liveness, credentials, inventory, diagnostics, and audit independent', async ({ page }) => {
    await openAgents(page)
    await page.locator('a[href="/admin/agents/' + AGENT_ID + '"]').click()
    await expect(page.getByRole('heading', { level: 1, name: /Agent 0195f2a1/ })).toBeVisible()

    // Separate dimension panels (data-dependent; the Server is shared by
    // all parallel projects, so allow for load).
    await expect(page.getByRole('heading', { level: 2, name: 'Identity' })).toBeVisible({
      timeout: 15_000,
    })
    await expect(page.getByRole('heading', { level: 2, name: 'Liveness' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Boot and report state' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Inventory' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Credentials' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Diagnostics' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Audit trail' })).toBeVisible()

    await expect(page.getByText(AGENT_ID, { exact: true }).first()).toBeVisible()
    await expect(page.getByRole('button', { name: 'Copy Agent ID' })).toBeVisible()
    await expect(page.getByText('Agent Epoch')).toBeVisible()
    await expect(page.getByText('1', { exact: true }).first()).toBeVisible()
    await expect(page.getByText('Report sequence', { exact: true })).toBeVisible()
    await expect(page.getByText('sequence #42')).toBeVisible()
    await expect(page.getByText('Full active boot ID', { exact: true })).toBeVisible()
    await expect(page.getByText('Boot status', { exact: true })).toBeVisible()
    await expect(page.getByText('Shutdown state')).toBeVisible()
    await expect(page.getByText('running', { exact: true })).toBeVisible()
    await expect(page.getByText(CREDENTIAL_ID, { exact: true })).toBeVisible()
    await expect(page.getByRole('button', { name: 'Revoke' })).toBeVisible()
    // Inventory stays per-Node, never merged at Agent level.
    await expect(page.getByText('Node A')).toBeVisible()
    await expect(page.getByText('Node D')).toBeVisible()
    await expect(page.getByText('Dropped sequence range')).toBeVisible()
    await expect(page.getByText('Last delivery error', { exact: true })).toBeVisible()
    await expect(page.getByText('delivery timeout retained after bounded retry', { exact: true })).toBeVisible()
    await expect(page.getByText(/Recorded evidence from the latest Host observation/)).toBeVisible()
    await expect(page.getByText('Host CPU')).toBeVisible()
    await expect(page.getByText('Host memory used / total')).toBeVisible()
    await expect(page.getByText('Host network RX / TX')).toBeVisible()
    await expect(page.getByText('Audit trail')).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('a Viewer is refused every Agent lifecycle route', async ({ page }) => {
    await loginAs(page, E2E_VIEWER_USERNAME, E2E_VIEWER_PASSWORD)
    await page.goto('/admin/agents')
    await expect(
      page.getByRole('heading', { level: 1, name: 'Owner access required' }),
    ).toBeVisible()
    await page.goto(`/admin/agents/${AGENT_ID}/rotate`)
    await expect(
      page.getByRole('heading', { level: 1, name: 'Owner access required' }),
    ).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })
})

test.describe.serial('Agent lifecycle mutations (one run on desktop-1280)', () => {
  test.beforeEach(async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'security mutations run once')
    await openAgents(page)
  })

  test('revocation is explicit, immediate, and refetches the authoritative state', async ({ page }) => {
    await page.locator('a[href="/admin/agents/' + AGENT_ID + '"]').click()
    await expect(
      page.getByRole('heading', { level: 1, name: /Agent 0195f2a1/ }),
    ).toBeVisible()

    // The seeded credential is revoked through an explicit confirmation.
    const item = page.locator('.credential-item', { hasText: CREDENTIAL_ID })
    await item.getByRole('button', { name: 'Revoke' }).click()
    await expect(item.getByText(/Revoke now\?/)).toBeVisible()
    await item.getByRole('button', { name: 'Confirm revoke' }).click()

    await expect(page.getByText(/Credential revoked at/)).toBeVisible()
    // No optimistic state: the Server refetch shows the revoked dimension.
    await expect(item.getByText('Revoked', { exact: true })).toBeVisible({ timeout: 15_000 })
    await expect(item.getByRole('button', { name: 'Revoke' })).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })
})
