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
 * The suite covers the Add Agent enrollment entry, the Server-owned
 * display name and notes, and the retained identity, liveness, credential,
 * inventory, and diagnostic evidence surface. Recovery and rotation remain
 * outside this slice.
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
  test('shows the exact five-column inventory summary and accessible identity controls', async ({ page }, testInfo) => {
    await openAgents(page)

    const row = page.locator('tr').filter({ has: page.locator('a[href="/admin/agents/' + AGENT_ID + '"]') })
    await expect(row).toBeVisible({ timeout: 15_000 })
    const headers = await page.locator('table[data-slot="agent-table"] thead th').allTextContents()
    expect(headers).toEqual(['Agent', 'Reporting', 'Last received', 'Nodes', 'Diagnostics'])
    expect(headers).not.toContain('Credentials')
    expect(headers).not.toContain('Epoch')
    expect(headers).not.toContain('View')

    // Five real cells per row in header order, and at table widths every cell
    // box must align with its column header (regression guard for the flex
    // <td> layout collapse).
    const structure = await row.evaluate((element) => {
      const row = element as HTMLElement
      const table = row.closest('table') as HTMLTableElement
      const box = (cell: Element) => {
        const rect = cell.getBoundingClientRect()
        return { left: Math.round(rect.left), width: Math.round(rect.width) }
      }
      return {
        labels: [...row.children].map((cell) => cell.getAttribute('data-label')),
        cellCount: row.children.length,
        cells: [...row.children].map(box),
        headers: [...table.querySelectorAll('thead th')].map(box),
      }
    })
    expect(structure.cellCount).toBe(5)
    expect(structure.labels).toEqual(['Agent', 'Reporting', 'Last received', 'Nodes', 'Diagnostics'])
    if ((page.viewportSize()?.width ?? 1280) >= 768) {
      for (let index = 0; index < structure.cells.length; index += 1) {
        expect(Math.abs(structure.cells[index].left - structure.headers[index].left)).toBeLessThanOrEqual(1)
        expect(Math.abs(structure.cells[index].width - structure.headers[index].width)).toBeLessThanOrEqual(1)
      }
    }

    await expect(row).toContainText('Current', { timeout: 15_000 })
    // Dimension sources are explained once in the header, not per row.
    await expect(row).not.toContainText(/Server liveness|Server receipt time|Server validity/)
    const receipt = row.locator('td[data-label="Last received"]')
    await expect(receipt).not.toContainText(/Never received|Unknown/)
    await expect(row.locator('td[data-label="Nodes"]')).toHaveText(/^\d+ retained$/)
    await expect(row).not.toContainText(/Active Nodes|healthy Nodes/)
    // Current attention and recorded history stay distinct summaries.
    const diagnosticCell = row.locator('td[data-label="Diagnostics"]')
    await expect(diagnosticCell.locator('[data-group="recorded"]')).toContainText('Recorded ·')
    await expect(row).not.toContainText(/\d+ active · \d+ revoked/)
    // Long evidence stays behind the details, never in the summary cell.
    await expect(row).not.toContainText('Dropped sequence range')
    await expect(row).not.toContainText('Host snapshot')

    const diagnosticsToggle = row.locator('button[aria-controls^="agent-diagnostics-"]')
    await diagnosticsToggle.click()
    await expect(diagnosticsToggle).toHaveAttribute('aria-expanded', 'true')
    const diagnosticsRow = row.locator('xpath=following-sibling::tr[1]')
    await expect(diagnosticsRow).toHaveAttribute('data-slot', 'detail-row')
    await expect(diagnosticsRow.locator('td')).toHaveAttribute('colspan', '5')
    // Shared Server: assert the credential summary's shape rather than assuming
    // the seeded credential is still active.
    await expect(diagnosticsRow).toContainText(/\d+ active · \d+ revoked · \d+ inactive \(not revoked\) · \d+ total/)
    await expect(diagnosticsRow).toContainText(/\d+ retained · \d+ Active · \d+ Retired/)
    await expect(diagnosticsRow).toContainText('Recorded gap intervals')
    await expect(diagnosticsRow).toContainText('Accumulated recorded security events')
    await expect(diagnosticsRow).toContainText('Dropped sequence range')
    await expect(diagnosticsRow).toContainText('#40–#42 recorded')
    await expect(diagnosticsRow).toContainText('Delivery error')
    await expect(diagnosticsRow).toContainText('Host snapshot')
    await expect(diagnosticsRow).not.toContainText('delivery timeout retained after bounded retry')
    await expect(row.locator('> *')).toHaveCount(5)
    await diagnosticsToggle.focus()
    await page.keyboard.press('Escape')
    await expect(diagnosticsToggle).toHaveAttribute('aria-expanded', 'false')
    await expect(page.locator('table[data-slot="agent-table"] tr[data-slot="detail-row"]')).toHaveCount(0)
    if ((page.viewportSize()?.width ?? 1280) < 768) {
      await expect(diagnosticCell).toHaveCSS('flex-direction', 'column')
      await expect(diagnosticsToggle).toBeVisible()
    }

    const noHostRow = page.locator('tr').filter({ has: page.locator('a[href="/admin/agents/' + NO_HOST_AGENT_ID + '"]') })
    await expect(noHostRow).toBeVisible({ timeout: 15_000 })
    await expect(noHostRow).toContainText('Host not observed yet')
    const noHostToggle = noHostRow.locator('button[aria-controls^="agent-diagnostics-"]')
    await noHostToggle.click()
    const noHostDetail = noHostRow.locator('xpath=following-sibling::tr[1]')
    await expect(noHostDetail).toContainText('Host observation')
    await expect(noHostDetail).toContainText('Not observed yet')
    await expect(noHostDetail).toContainText('No Host snapshot')
    await expect(noHostDetail).not.toContainText('Spool observation')
    await noHostToggle.click()

    if (page.viewportSize()?.width === 768) {
      const headersReadable = await page.locator('table[data-slot="agent-table"] thead th').evaluateAll((cells) =>
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
      expect(cardOrder).toEqual(['Agent', 'Reporting', 'Last received', 'Nodes', 'Diagnostics'])
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

    // SCN-AGENT-ENROLL-GUIDANCE: the Add Agent entry is available and
    // opens the enrollment workflow; it is never an unavailable action.
    await expect(page.getByRole('link', { name: 'Add Agent' }).first()).toBeVisible()
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
    await expect(page.locator('table[data-slot="agent-table"] thead th')).toHaveCount(5)
    await expectNoHorizontalOverflow(page)
  })
})

test.describe('Agent detail (PAGE-ADMIN-AGENT-DETAIL)', () => {
  test('keeps identity, liveness, credentials, inventory, diagnostics, and audit independent', async ({ page }) => {
    await openAgents(page)
    await page.locator('a[href="/admin/agents/' + AGENT_ID + '"]').click()
    await expect(page.getByRole('heading', { level: 1, name: /Agent 0195f2a1/ })).toBeVisible()

    // Header dimensions, then four sections (webui.md §8.7). Data-dependent;
    // the Server is shared by all parallel projects, so allow for load.
    await expect(page.getByLabel('Current Agent dimensions')).toBeVisible({ timeout: 15_000 })
    await expect(page.getByRole('tab', { name: 'Overview' })).toHaveAttribute('aria-selected', 'true')
    await expect(page.getByRole('heading', { level: 2, name: 'Reporting and runtime' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Inventory' })).toBeVisible()
    await expect(page.getByText('Report sequence', { exact: true })).toBeVisible()
    await expect(page.getByText('#42', { exact: true })).toBeVisible()
    // Inventory stays per-Node, never merged at Agent level.
    await expect(page.getByText('Node A')).toBeVisible()
    await expect(page.getByText('Node D')).toBeVisible()
    // Diagnostics, history and management are one section away, not stacked.
    await expect(page.getByText('Full active boot ID', { exact: true })).toHaveCount(0)
    await expect(page.getByRole('heading', { name: 'Audit trail' })).toHaveCount(0)
    await expect(page.getByRole('heading', { name: 'Danger zone' })).toHaveCount(0)
    await expect(page.getByText(/^0[1-8]$/)).toHaveCount(0)

    await page.getByRole('button', { name: /Show full\s*Agent ID/ }).click()
    await expect(page.getByText(AGENT_ID, { exact: true }).first()).toBeVisible()
    await expect(page.getByRole('button', { name: 'Copy Agent ID' })).toBeVisible()

    await page.getByRole('tab', { name: /^Diagnostics/ }).click()
    const diagnostics = page.locator('[data-slot="admin-detail-section"][data-section="diagnostics"]')
    for (const heading of ['Identity', 'Boot and report state', 'Host and component evidence']) {
      await expect(diagnostics.getByRole('heading', { level: 2, name: heading })).toBeVisible()
    }
    await expect(diagnostics.getByText('Agent Epoch').first()).toBeVisible()
    await expect(diagnostics.getByText('sequence #42')).toBeVisible()
    await expect(diagnostics.getByText('Full active boot ID', { exact: true })).toBeVisible()
    await expect(diagnostics.getByText('Shutdown state')).toBeVisible()
    await expect(diagnostics.getByText('running', { exact: true })).toBeVisible()
    await expect(diagnostics.getByText('Dropped sequence range')).toBeVisible()
    await expect(diagnostics.getByText('Last delivery error', { exact: true })).toBeVisible()
    await expect(diagnostics.getByText('delivery timeout retained after bounded retry', { exact: true })).toBeVisible()
    await expect(
      diagnostics.getByText(/Recorded evidence from the latest Host observation/),
    ).toBeVisible()
    for (const label of ['Host CPU', 'Host memory used / total', 'Host network RX / TX']) {
      // Chromium exposes no accessible name for role=term, so the evidence is matched as the
      // exact <dt> text plus its own <dd>, and that value cell must render a recorded value.
      const evidence = diagnostics.locator('dt:text-is(' + JSON.stringify(label) + ') + dd')
      await expect(evidence).toBeVisible()
      await expect(evidence).toHaveText(/\S/)
    }

    await page.getByRole('tab', { name: 'History' }).click()
    await expect(page.getByRole('heading', { level: 2, name: 'Audit trail' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Host resource history' })).toBeVisible()

    await page.getByRole('tab', { name: /^Manage/ }).click()
    await expect(page.getByRole('heading', { level: 2, name: 'Credential records' })).toBeVisible()
    await expect(page.getByText(CREDENTIAL_ID, { exact: true })).toBeVisible()
    // The seeded credential may already have been revoked by an earlier spec in the
    // shared Server; the dimension must render its Server-owned state either way.
    const manage = page.locator('[data-slot="admin-detail-section"][data-section="manage"]')
    await expect(
      manage.getByRole('button', { name: 'Revoke' }).or(manage.getByText(/revoked/i)).first(),
    ).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Danger zone' })).toBeVisible()
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
    await page.getByRole('tab', { name: /^Manage/ }).click()

    // The seeded credential is revoked through an explicit confirmation.
    const item = page.locator('[data-slot="credential-item"]', { hasText: CREDENTIAL_ID })
    await item.getByRole('button', { name: 'Revoke' }).click()
    await expect(item.getByText(/Revoke now\?/)).toBeVisible()
    await item.getByRole('button', { name: 'Confirm revoke' }).click()

    await expect(page.getByText(/Credential revoked at/)).toBeVisible()
    // No optimistic state: the Server refetch shows the revoked dimension.
    await expect(item.getByText('Revoked', { exact: true })).toBeVisible({ timeout: 15_000 })
    await expect(item.getByRole('button', { name: 'Revoke' })).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })

  test('metadata mutation saves the Server-owned name and notes', async ({ page }) => {
    await page.locator('a[href="/admin/agents/' + AGENT_ID + '"]').click()
    await expect(
      page.getByRole('heading', { level: 1, name: /Agent 0195f2a1/ }),
    ).toBeVisible()
    await page.getByRole('tab', { name: /^Manage/ }).click()

    try {
      await page.getByLabel('Display name').fill('E2E Host A Agent')
      await page.getByLabel('Notes').fill('E2E notes for the primary Host')
      await page.getByRole('button', { name: 'Save name and notes' }).click()
      await expect(page.getByText(/Saved\. The name and notes are Server-owned/)).toBeVisible({
        timeout: 15_000,
      })
      // The refetched detail renders the Server-owned name while the stable
      // Agent ID stays visible and copyable.
      await expect(page.getByRole('heading', { level: 1, name: /E2E Host A Agent/ })).toBeVisible({
        timeout: 15_000,
      })
      await page.getByRole('button', { name: /Show full\s*Agent ID/ }).click()
      await expect(page.getByText(AGENT_ID, { exact: true }).first()).toBeVisible()
      await expect(page.getByRole('button', { name: 'Copy Agent ID' })).toBeVisible()
    } finally {
      // Restore the enrolled baseline: no Server-owned name or notes.
      await page.getByLabel('Display name').fill('')
      await page.getByLabel('Notes').fill('')
      await page.getByRole('button', { name: 'Save name and notes' }).click()
      await expect(
        page.getByRole('heading', { level: 1, name: /Agent 0195f2a1/ }),
      ).toBeVisible({ timeout: 15_000 })
    }
  })
})
