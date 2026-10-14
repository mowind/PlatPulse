import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

// The real Server fixture deduplicates linked Validators before exposing groups:
// platon-e2e = 100 blocks / 10 rewards; home-convergence = 255 / 32.
const groups = [
  { key: 'platon-e2e', blocks: '100', rewards: '10', coverage: '1/1 reporting', unlinked: '5 unlinked Nodes' },
  { key: 'home-convergence', blocks: '255', rewards: '32', coverage: '3/3 reporting', unlinked: '3 unlinked Nodes' },
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
      // The tile face carries its own scope and its own valued/expected
      // reporting count directly; the Breakdown keeps the full per-metric
      // explanation.
      const caption = card.locator('[data-slot="summary-caption"]')
      await expect(caption).toContainText('2 Networks')
      await expect(caption).toContainText(/4\/4 reporting/)
      await card.getByRole('button', { name: label + ' breakdown and exact values' }).click()
      const dialog = page.getByRole('dialog', { name: label + ' — Breakdown' })
      await expect(dialog).toBeVisible()
      await expect(dialog.getByText('Exact known-value sum: ' + sum, { exact: true })).toBeVisible()
      // The stale contributor count depends on Provider freshness across the
      // long-lived fixture, so the shape is asserted while its exact number is
      // not pinned; the per-metric coverage denominator still is.
      const facts = dialog.locator('[data-slot="validator-overview-scope"]')
      await expect(facts).toContainText('2 Networks')
      await expect(facts).toContainText(/4\/4 reporting( · \d+ stale)?/)
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
      const caption = card.locator('[data-slot="summary-caption"]')
      await expect(caption).toContainText('PlatON E2E Network')
      await expect(caption).toContainText(/1\/1 reporting/)
      await card.getByRole('button', { name: label + ' breakdown and exact values' }).click()
      const dialog = page.getByRole('dialog')
      const facts = dialog.locator('[data-slot="validator-overview-scope"]')
      await expect(facts).toContainText('PlatON E2E Network')
      await expect(facts).toContainText(/1\/1 reporting( · \d+ stale)?/)
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
    await expect(blocks.locator('[data-slot="summary-caption"]')).toHaveText('2 Networks · 4/6 reporting · 1 stale')
    await expect(rewards.locator('[data-slot="summary-caption"]')).toHaveText('2 Networks · 1/6 reporting')
    for (const [label, exact, coverage] of [
      ['Cumulative blocks', '9,007,199,254,741,000', '4/6 reporting · 1 stale'],
      ['Cumulative rewards', '0.123456789012345678', '1/6 reporting'],
    ] as const) {
      await page.getByRole('button', { name: label + ' breakdown and exact values' }).click()
      const dialog = page.getByRole('dialog')
      await expect(dialog.getByText('Exact known-value subtotal: ' + exact, { exact: true })).toBeVisible()
      await expect(dialog.locator('[data-slot="validator-overview-scope"]')).toContainText(coverage)
      await expect(dialog.locator('[data-slot="validator-summary-rewards"] dd').nth(1)).toHaveText('Unknown')
      await expect(dialog.getByText('0/3 reporting', { exact: true })).toBeVisible()
      await expectNoHorizontalOverflow(page)
      await dialog.getByRole('button', { name: 'Close', exact: true }).click()
    }
  })

  test('names a missing Network summary instead of letting the denominator stand for the selection', async ({ page }) => {
    await loginAs(page)
    let dropAll = false
    await page.route('**/api/public/v1/networks*', async route => {
      const response = await route.fetch()
      const networks = await response.json()
      // The Server publishes every summary it has; this fixture withholds one,
      // then both, to read what the tiles do about coverage they cannot claim.
      await route.fulfill({ response, json: networks.map((network: { validatorSummary?: object }, index: number) => (
        dropAll || index > 0 ? { ...network, validatorSummary: null } : network)) })
    })
    await page.goto('/')
    const blocks = page.getByRole('article', { name: 'Cumulative blocks', exact: true })
    const rewards = page.getByRole('article', { name: 'Cumulative rewards', exact: true })
    // One reporting Network keeps its own subtotal, and the face says the
    // denominator is that Network's alone — the missing summary is neither zero
    // nor covered. The stale contributor count follows Provider freshness across
    // this long-lived fixture, so the shape is asserted while its number is not.
    const reporting = /^2 Networks · [0-9]+\/[0-9]+ reporting · 1 Network summary missing( · [0-9]+ stale)?$/
    await expect(blocks.locator('[data-slot="summary-caption"]')).toHaveText(reporting)
    await expect(rewards.locator('[data-slot="summary-caption"]')).toHaveText(reporting)
    // The tile prints the reporting Network's own last-good subtotal rather than
    // a selection total, so the value is read back from the same Server payload.
    const kept = await page.evaluate(async () => {
      const response = await fetch('/api/public/v1/networks')
      const networks = (await response.json()) as Array<{ validatorSummary?: { blocks?: { knownSum?: string | null } } }>
      return networks[0]?.validatorSummary?.blocks?.knownSum ?? null
    })
    expect(kept, 'the live fixture keeps one reporting Network').not.toBeNull()
    const digits = (text: string) => text.replace(/[^0-9]/g, '')
    expect(digits(await blocks.locator('[data-slot="summary-value"]').innerText())).toBe(digits(String(kept)))
    await blocks.getByRole('button', { name: 'Cumulative blocks breakdown and exact values' }).click()
    let dialog = page.getByRole('dialog')
    await expect(dialog.locator('[data-slot="validator-overview-scope"]')).toContainText('reporting · 1 Network summary missing')
    await expect(dialog.getByText('1 Network summary missing from this coverage.')).toBeVisible()
    await expect(dialog.getByText('Summary unavailable; values and coverage are unknown.')).toBeVisible()
    // The reporting vocabulary is explained where the reader can reach it.
    await expect(dialog.getByText(/“5\/8 reporting” counts one eligible Validator once per Network/)).toBeVisible()
    await expectNoHorizontalOverflow(page)
    await dialog.getByRole('button', { name: 'Close', exact: true }).click()
    // No summary at all is coverage unknown — never a complete 0/0 reading and
    // never a zero sum.
    dropAll = true
    await page.reload()
    await expect(blocks.locator('[data-slot="summary-caption"]')).toHaveText('2 Networks · Coverage unknown')
    await expect(rewards.locator('[data-slot="summary-caption"]')).toHaveText('2 Networks · Coverage unknown')
    await expect(blocks.locator('[data-slot="summary-value"]')).toHaveText('Unknown')
    await expect(rewards.locator('[data-slot="summary-value"]')).toHaveText('Unknown')
    await blocks.getByRole('button', { name: 'Cumulative blocks breakdown and exact values' }).click()
    dialog = page.getByRole('dialog')
    await expect(dialog.locator('[data-slot="validator-overview-scope"]')).toContainText('Coverage unknown')
    await expect(dialog.getByText('2 Network summaries missing from this coverage.')).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('reaches both cumulative explanations by keyboard and by touch', async ({ page }, testInfo) => {
    await loginAs(page)
    await page.goto('/')
    const blocks = page.getByRole('article', { name: 'Cumulative blocks', exact: true })
    const rewards = page.getByRole('article', { name: 'Cumulative rewards', exact: true })
    const blocksName = 'Cumulative blocks breakdown and exact values'
    const rewardsName = 'Cumulative rewards breakdown and exact values'
    // The shared marker slot moved both controls; the ordinary keyboard walk from
    // the top of the page still reaches them, and Enter still opens the complete
    // explanation.
    let reached = false
    for (let press = 0; press < 40 && !reached; press++) {
      await page.keyboard.press('Tab')
      reached = await page.evaluate(name => document.activeElement?.getAttribute('aria-label') === name, blocksName)
    }
    expect(reached, 'Tab reaches the cumulative control').toBe(true)
    await page.keyboard.press('Enter')
    const blocksDialog = page.getByRole('dialog', { name: 'Cumulative blocks — Breakdown' })
    await expect(blocksDialog).toBeVisible()
    await expect(blocksDialog.getByText(/counts one eligible Validator once per Network/)).toBeVisible()
    // Escape closes it and hands focus back to the control that opened it.
    await page.keyboard.press('Escape')
    await expect(blocksDialog).toBeHidden()
    await expect(blocks.getByRole('button', { name: blocksName })).toBeFocused()
    // Touch: a genuine tap on the 44px target the marker slot gives the second
    // cumulative tile (the desktop projects carry no touch input).
    const rewardsTrigger = rewards.getByRole('button', { name: rewardsName })
    if (testInfo.project.use.hasTouch) await rewardsTrigger.tap()
    else await rewardsTrigger.click()
    const rewardsDialog = page.getByRole('dialog', { name: 'Cumulative rewards — Breakdown' })
    await expect(rewardsDialog).toBeVisible()
    await expect(rewardsDialog.getByText(/counts one eligible Validator once per Network/)).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(rewardsDialog).toBeHidden()
    await expect(rewardsTrigger).toBeFocused()
    await expectNoHorizontalOverflow(page)
  })
})
