import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

// Read-only fixture: a browser-local response override cannot rename shared Nodes.
const LONG_NAME = 'Overview regression ' + 'very-long-node-name-'.repeat(12)

// The Overview previews (webui.md §8.4.3, §8.4.4, §8.7) keep every item's
// name, link and independent status badges inside its panel, wrap a long name
// instead of widening anything, and never cause page-level overflow.
test('Overview preview items stay bounded and keep their status visible with long Node names', async ({ page }) => {
  await loginAs(page)
  await page.route('**/api/admin/v1/nodes', async (route) => {
    const response = await route.fetch()
    const nodes = await response.json()
    expect(Array.isArray(nodes)).toBe(true)
    await route.fulfill({ response, json: nodes.map((node: Record<string, unknown>) => ({ ...node, display_name: LONG_NAME })) })
  })
  await page.getByRole('link', { name: 'Admin', exact: true }).click()

  for (const name of ['Highest-priority Active Nodes', 'Highest-priority Agents']) {
    const list = page.getByRole('list', { name })
    await expect(list.getByRole('listitem').first()).toBeVisible({ timeout: 15_000 })
    const items = await list.getByRole('listitem').evaluateAll((elements) =>
      elements.map((item) => {
        const panel = item.closest('article')!.getBoundingClientRect()
        const rect = item.getBoundingClientRect()
        const link = item.querySelector('a')!.getBoundingClientRect()
        return {
          fits: rect.left >= panel.left - 1 && rect.right <= panel.right + 1,
          contentFits: item.scrollWidth <= item.clientWidth + 1,
          linkHeight: link.height,
          badges: item.querySelectorAll('[data-slot="status-badge"]').length,
        }
      }),
    )
    expect(items.length).toBeLessThanOrEqual(5)
    for (const item of items) {
      expect(item.fits, JSON.stringify(item)).toBe(true)
      expect(item.contentFits, JSON.stringify(item)).toBe(true)
      expect(item.linkHeight).toBeGreaterThanOrEqual(44)
      expect(item.badges).toBeGreaterThanOrEqual(1)
    }
  }

  // Each Node item keeps Health and Freshness as two separate badges and
  // links straight to the Node detail.
  const nodeItem = page.getByRole('list', { name: 'Highest-priority Active Nodes' }).getByRole('listitem').first()
  await expect(nodeItem.locator('[data-slot="status-badge"]')).toHaveCount(2)
  await expect(nodeItem.getByRole('link', { name: LONG_NAME })).toHaveAttribute('href', /^\/admin\/nodes\//)
  await expectNoHorizontalOverflow(page)
})
