import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

// Read-only fixture: a browser-local response override cannot rename shared Nodes.
const LONG_NAME = 'Overview regression ' + 'very-long-node-name-'.repeat(12)

test('Overview priority cells stay visible and long Node names stay bounded', async ({ page }) => {
  await loginAs(page)
  await page.route('**/api/admin/v1/nodes', async (route) => {
    const response = await route.fetch()
    const nodes = await response.json()
    expect(Array.isArray(nodes)).toBe(true)
    await route.fulfill({ response, json: nodes.map((node: Record<string, unknown>) => ({ ...node, display_name: LONG_NAME })) })
  })
  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  const tables = [
    { caption: 'PlatON Node health, freshness, and sync', labels: ['Node', 'Network', 'Health', 'Freshness', 'Head / Sync', 'Resync'] },
    { caption: 'Agent inventory overview with reporting, receipt, host resources, evidence, and retained Node summaries', labels: ['Agent', 'Reporting', 'Last received', 'Host resources', 'Evidence', 'Nodes'] },
  ]
  const phone = (page.viewportSize()?.width ?? 1280) < 768
  for (const { caption, labels } of tables) {
    const table = page.getByRole('table', { name: caption })
    await expect(table.locator('tbody tr').first()).toBeVisible()
    const rows = table.locator('tbody tr:not([data-slot="detail-row"])')
    for (const row of await rows.all()) {
      const cells = row.locator(':scope > th, :scope > td')
      expect.soft(await cells.evaluateAll((elements) => elements.map((cell) => cell.getAttribute('data-label')))).toEqual(labels)
      if (phone) {
        const geometry = await cells.evaluateAll((elements) => elements.map((cell) => {
          const wrapper = cell.closest('table')!.parentElement!
          const bounds = wrapper.getBoundingClientRect()
          const rect = cell.getBoundingClientRect()
          return {
            label: cell.getAttribute('data-label'),
            top: rect.top,
            fits: rect.width > 0 && rect.left >= bounds.left - 1 && rect.right <= bounds.right + 1,
            contentFits: cell.scrollWidth <= cell.clientWidth + 1,
            prefix: getComputedStyle(cell, '::before').content,
          }
        }))
        expect([...geometry].sort((a, b) => a.top - b.top).map((cell) => cell.label)).toEqual(labels)
        for (const cell of geometry) {
          expect(cell.fits, JSON.stringify(cell)).toBe(true)
          expect(cell.contentFits, JSON.stringify(cell)).toBe(true)
          expect(cell.prefix).toBe(JSON.stringify(cell.label))
        }
      }
    }
    const wrapper = table.locator('..')
    if (phone) {
      expect(await wrapper.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1)
    } else {
      await expect(rows.first()).toHaveCSS('display', 'table-row')
      // Tablet is allowed a local scroller, never a page-level one.
      await expect(wrapper).toHaveCSS('overflow-x', 'auto')
    }
  }
  const spoolLabels = page.getByRole('table', { name: tables[1].caption }).locator('dt').filter({ hasText: /^Spool$/ })
  await expect(spoolLabels).toHaveCount(3)
  for (const label of await spoolLabels.all()) {
    const dimensions = await label.evaluate((element) => ({
      height: element.getBoundingClientRect().height,
      lineHeight: parseFloat(getComputedStyle(element).lineHeight),
    }))
    expect(dimensions.height).toBeLessThanOrEqual(dimensions.lineHeight + 1)
  }

  const nodeTable = page.getByRole('table', { name: tables[0].caption })
  const toggle = nodeTable.getByRole('button', { name: LONG_NAME }).first()
  await expect(toggle).toBeVisible()
  // ADR 0003: expansion and navigation are sibling targets on one action row,
  // not a View Node link stacked beneath the identifier (or nested in a button).
  const detailLink = toggle.locator('..').getByRole('link', { name: 'View Node' })
  await expect(detailLink).toBeVisible()
  await expect(toggle.locator('a')).toHaveCount(0)
  await expect(detailLink.locator('button')).toHaveCount(0)
  const toggleBounds = (await toggle.boundingBox())!
  const detailBounds = (await detailLink.boundingBox())!
  expect(Math.abs(toggleBounds.y - detailBounds.y)).toBeLessThanOrEqual(1)
  expect(detailBounds.x).toBeGreaterThanOrEqual(toggleBounds.x + toggleBounds.width)
  expect(detailBounds.width).toBeGreaterThanOrEqual(44)
  expect(detailBounds.height).toBeGreaterThanOrEqual(44)
  const size = await toggle.evaluate((button) => {
    const cell = button.closest('th')!
    return { button: button.getBoundingClientRect().width, cell: cell.getBoundingClientRect().width, table: cell.closest('table')!.getBoundingClientRect().width }
  })
  expect(size.button).toBeLessThanOrEqual(size.cell)
  if (!phone) expect(size.cell).toBeLessThanOrEqual(size.table * 0.3)
  await toggle.click()
  await expect(toggle).toHaveAttribute('aria-expanded', 'true')
  const detail = nodeTable.locator('[data-slot="detail-row"]')
  await expect(detail).toBeVisible()
  if (phone) {
    const bounds = await detail.evaluate((row) => ({ width: row.scrollWidth, wrapper: row.closest('table')!.parentElement!.clientWidth }))
    expect(bounds.width).toBeLessThanOrEqual(bounds.wrapper + 1)
  }
  await toggle.press('Escape')
  await expect(toggle).toHaveAttribute('aria-expanded', 'false')
  await expect(toggle).toBeFocused()
  await expectNoHorizontalOverflow(page)
})
