import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

/**
 * Issues #154 and #155: the linked-Validator cumulative block count and gross
 * cumulative rewards are visible on both Node views — the Home Node card and
 * Node detail — with identity, link role, and freshness, and they never
 * overflow the fixed viewports. The rewards value is the source's gross
 * cumulative reward, not the operator's net earnings.
 */

const PUBLIC_NODE_NAME = 'Node A'
const PUBLIC_NODE_ID = '0195f2a1-0014-4014-8014-000000000014'
const LINKED_BLOCK_COUNT = '100'
const LINKED_REWARD = '10'

test.describe('Linked Validator cumulative blocks and rewards (#154, #155)', () => {
  test('shows cumulative blocks and gross rewards on the Home card and Node detail', async ({ page }) => {
    await loginAs(page)

    // Home Node card: identity, role, and both cumulative Validator values.
    const card = page.getByRole('link', { name: new RegExp(PUBLIC_NODE_NAME) }).first()
    await expect(card).toBeVisible()
    await expect(card.getByText('Linked Validator')).toBeVisible()
    await expect(card.getByText('Cumulative blocks')).toBeVisible()
    await expect(card.getByText('Cumulative rewards', { exact: true })).toBeVisible()
    await expect(
      card.getByText('Cumulative blocks').locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_BLOCK_COUNT)
    await expect(
      card.getByText('Cumulative rewards', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_REWARD)
    await expect(card.getByText('Primary')).toBeVisible()

    // Node detail: the same linked Validator area plus the exact-value caveat
    // and provenance.
    await page.goto('/nodes/' + PUBLIC_NODE_ID)
    const detail = page.getByRole('region', { name: 'Linked Validator' })
    await expect(detail).toBeVisible()
    await expect(detail.getByText('Cumulative blocks')).toBeVisible()
    await expect(detail.getByText('Cumulative rewards', { exact: true })).toBeVisible()
    await expect(
      detail.getByText('Cumulative rewards', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_REWARD)
    await expect(detail.getByText(/not operator net earnings/)).toBeVisible()
    await expect(detail.getByText('Primary')).toBeVisible()
    await expect(detail.getByText('Last success')).toBeVisible()

    await expectNoHorizontalOverflow(page)
  })
})
