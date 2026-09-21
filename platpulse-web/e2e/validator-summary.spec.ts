import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

// The real Server fixture deduplicates linked Validators before exposing groups:
// platon-e2e = 100 blocks / 10 rewards; home-convergence = 255 / 32.
const groups = [
  { key: 'platon-e2e', blocks: '100', rewards: '10', coverage: '1/1 Validators with values', unlinked: '5 unlinked Nodes' },
  { key: 'home-convergence', blocks: '255', rewards: '32', coverage: '3/3 Validators with values', unlinked: '3 unlinked Nodes' },
]

test.describe('Home Validator totals (#159)', () => {
  test('shows summed Server groups and independent coverage in overview, Network evidence in each dialog', async ({ page }) => {
    await loginAs(page)
    await expect(page.locator('[data-slot="summary-card"]')).toHaveCount(6)
    await expect(page.getByRole('heading', { name: 'Validator totals', exact: true })).toHaveCount(0)
    await expect(page.locator('[data-slot="validator-summary-network"]')).toHaveCount(0)
    for (const [label, sum] of [['Cumulative blocks', '355'], ['Cumulative rewards', '42']]) {
      const card = page.getByRole('article', { name: label, exact: true })
      await expect(card.locator('[data-slot="summary-value"]')).toHaveText(sum)
      // The tile face is number-only: scope, coverage, Partial and staleness
      // are read from the accessible Breakdown rather than printed on the card.
      await expect(card).not.toContainText(/known|Partial|Networks|stale/)
      await card.getByRole('button', { name: label + ' breakdown and exact values' }).click()
      const dialog = page.getByRole('dialog', { name: label + ' — Breakdown' })
      await expect(dialog).toBeVisible()
      await expect(dialog.getByText('Exact known-value sum: ' + sum, { exact: true })).toBeVisible()
      // The stale contributor count depends on Provider freshness across the
      // long-lived fixture, so the shape is asserted while its exact number is
      // not pinned; the per-metric coverage denominator still is.
      const facts = dialog.locator('[data-slot="validator-overview-scope"]')
      await expect(facts).toContainText('2 Networks')
      await expect(facts).toContainText(/4\/4 known( · \d+ stale)?/)
      await expect(dialog.getByText(/grouped by Network · deduplicated by Validator/)).toBeVisible()
      await expect(dialog.getByText(/native unit without asset conversion/)).toBeVisible()
      for (const group of groups) {
        const network = dialog.locator('[data-slot="validator-summary-network"][data-network-key="' + group.key + '"]')
        for (const metric of ['blocks', 'rewards'] as const) {
          const section = network.locator('[data-slot="validator-summary-' + metric + '"]')
          await expect(section.locator('dd')).toHaveText(group[metric])
          await expect(section.getByText(group.coverage)).toBeVisible()
        }
        await expect(network.locator('[data-slot="validator-summary-unlinked"]')).toHaveText(group.unlinked)
      }
      await expectNoHorizontalOverflow(page)
      await dialog.getByRole('button', { name: 'Close', exact: true }).click()
    }
  })

  test('scopes both overview and Breakdown to the selected Network filter', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('tab', { name: 'PlatON E2E Network' }).click()
    for (const [label, value] of [['Cumulative blocks', '100'], ['Cumulative rewards', '10']]) {
      const card = page.getByRole('article', { name: label, exact: true })
      await expect(card.locator('[data-slot="summary-value"]')).toHaveText(value)
      await expect(card).not.toContainText(/known|Partial|Networks|stale/)
      await card.getByRole('button', { name: label + ' breakdown and exact values' }).click()
      const dialog = page.getByRole('dialog')
      const facts = dialog.locator('[data-slot="validator-overview-scope"]')
      await expect(facts).toContainText('PlatON E2E Network')
      await expect(facts).toContainText(/1\/1 known( · \d+ stale)?/)
      await expect(dialog.locator('[data-slot="validator-summary-network"]')).toHaveCount(1)
      await expect(dialog.locator('[data-network-key="platon-e2e"]')).toBeVisible()
      await expect(dialog.locator('[data-network-key="home-convergence"]')).toHaveCount(0)
      await expectNoHorizontalOverflow(page)
      await dialog.getByRole('button', { name: 'Close', exact: true }).click()
    }
  })

  test('keeps exact decimal sums and partial, stale, unknown coverage independent of Node values', async ({ page }) => {
    await loginAs(page)
    await page.route('**/api/public/v1/networks*', async route => {
      const response = await route.fetch()
      const networks = await response.json()
      await route.fulfill({ response, json: networks.slice(0, 2).map((network: { validatorSummary: object }, index: number) => ({
        ...network,
        validatorSummary: {
          ...network.validatorSummary,
          blocks: { knownSum: index === 0 ? '9007199254740993' : '7', expectedCount: 3, valuedCount: 2, staleCount: index, state: 'partial' },
          rewards: { knownSum: index === 0 ? '0.123456789012345678' : null, expectedCount: 3, valuedCount: index === 0 ? 1 : 0, staleCount: 0, state: index === 0 ? 'partial' : 'unknown' },
        },
      })) })
    })
    await page.goto('/')
    const blocks = page.getByRole('article', { name: 'Cumulative blocks', exact: true })
    const rewards = page.getByRole('article', { name: 'Cumulative rewards', exact: true })
    await expect(blocks).not.toContainText(/known|Partial|stale/)
    await expect(rewards).not.toContainText(/known|Partial|stale/)
    for (const [label, exact, coverage] of [
      ['Cumulative blocks', '9,007,199,254,741,000', '4/6 known · Partial · 1 stale'],
      ['Cumulative rewards', '0.123456789012345678', '1/6 known · Partial'],
    ] as const) {
      await page.getByRole('button', { name: label + ' breakdown and exact values' }).click()
      const dialog = page.getByRole('dialog')
      await expect(dialog.getByText('Exact known-value subtotal: ' + exact, { exact: true })).toBeVisible()
      await expect(dialog.locator('[data-slot="validator-overview-scope"]')).toContainText(coverage)
      await expect(dialog.locator('[data-slot="validator-summary-rewards"] dd').nth(1)).toHaveText('Unknown')
      await expect(dialog.getByText('0/3 Validators with values', { exact: true })).toBeVisible()
      await expectNoHorizontalOverflow(page)
      await dialog.getByRole('button', { name: 'Close', exact: true }).click()
    }
  })
})
