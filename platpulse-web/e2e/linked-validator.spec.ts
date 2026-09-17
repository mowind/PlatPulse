import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

/**
 * Issue #154: the linked-Validator cumulative block count is visible on both
 * Node views — the Home Node card and Node detail — with its identity, link
 * role, and freshness, and it never overflows the fixed viewports.
 */

const PUBLIC_NODE_NAME = 'Node A'
const PUBLIC_NODE_ID = '0195f2a1-0014-4014-8014-000000000014'
const LINKED_BLOCK_COUNT = '100'

test.describe('Linked Validator cumulative block count (#154)', () => {
  test('shows the linked Validator on the Home card and Node detail', async ({ page }) => {
    await loginAs(page)

    // Home Node card: identity, role, and the cumulative Validator count.
    const card = page.getByRole('link', { name: new RegExp(PUBLIC_NODE_NAME) }).first()
    await expect(card).toBeVisible()
    await expect(card.getByText('Linked Validator')).toBeVisible()
    await expect(card.getByText('Cumulative blocks')).toBeVisible()
    await expect(card.getByText(LINKED_BLOCK_COUNT, { exact: true })).toBeVisible()
    await expect(card.getByText('Primary')).toBeVisible()

    // Node detail: the same linked Validator area plus provenance.
    await page.goto('/nodes/' + PUBLIC_NODE_ID)
    const detail = page.getByRole('region', { name: 'Linked Validator' })
    await expect(detail).toBeVisible()
    await expect(detail.getByText('Cumulative blocks')).toBeVisible()
    await expect(detail.getByText(LINKED_BLOCK_COUNT, { exact: true })).toBeVisible()
    await expect(detail.getByText('Primary')).toBeVisible()
    await expect(detail.getByText('Last success')).toBeVisible()

    await expectNoHorizontalOverflow(page)
  })
})
