import { expect, test, type Page } from '@playwright/test'
import {
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
  loginAs,
} from './helpers'

/**
 * Final Home convergence acceptance (issue #102). The shared Server is
 * seeded with production-like fixtures (e2e/start-server.sh): exact and
 * missing Current Head Block Summaries, current and stale consensus
 * membership, effective and missing Node Validator Links, current Provider
 * Activity, Observing, Unknown, and stale last-good Activity, plus long
 * Node/Network display names. This spec is read-only and runs on every fixed
 * viewport project under the repository's single-worker convention.
 *
 * Assertions cross the routed Public Home seam only: accessible roles and
 * names, visible text, and externally observable layout geometry. They never
 * reach into production CSS selectors or component-internal state.
 */

const CONVERGENCE_NETWORK_NAME = 'Home Convergence Network With An Extremely Long Display Name'
const NODE_H_ID = '0195f2a1-0060-4060-8060-000000000060'

const nodeCard = (page: Page, name: RegExp) => page.getByRole('link', { name: name })

/** The visible text order of every Node-card link on Home. */
async function nodeCardNames(page: Page): Promise<string[]> {
  const links = await page.getByRole('link').evaluateAll((elements) =>
    elements.flatMap((element) => {
      const href = element.getAttribute('href') ?? ''
      return href.startsWith('/nodes/') ? [(element.textContent ?? '').replace(/\s+/g, ' ').trim()] : []
    }),
  )
  return links
}

test.describe('Converged Public Home (issue #102)', () => {
  test('final card header, both metric rows, summary shell, and viewport grid', async ({ page }, testInfo) => {
    await loginAs(page)
    await expect(page.getByRole('heading', { level: 1, name: 'Home' })).toBeVisible()

    const hCard = nodeCard(page, /Node H/)
    await expect(hCard).toBeVisible({ timeout: 15_000 })

    // Header: status marker + Node/Network identity, then Validator Activity
    // and Node Health badges in that order. The whole card is one semantic
    // link, so its accessible name carries the badge order.
    await expect(
      page.getByRole('link', { name: /Node H — Producing Card[\s\S]*Producing[\s\S]*Healthy/ }),
    ).toHaveCount(1)
    // The reverse order (Health before Activity) never occurs.
    await expect(
      page.getByRole('link', { name: /Node H — Producing Card[\s\S]*Healthy[\s\S]*Producing/ }),
    ).toHaveCount(0)

    // Both compact metric rows carry exactly the required labels and values:
    // HEAD / TXS / PEERS and QC / LOCKED / COMMITTED / VALIDATOR.
    for (const label of ['HEAD', 'TXS', 'PEERS', 'QC', 'LOCKED', 'COMMITTED', 'VALIDATOR']) {
      await expect(hCard.getByText(label, { exact: true })).toBeVisible()
    }
    // HEAD, QC, and COMMITTED share the height; each appears once per row.
    await expect(hCard.getByText('12,842,025', { exact: true })).toHaveCount(3)
    await expect(hCard.getByText('12,842,024', { exact: true })).toHaveCount(1)
    await expect(hCard.getByText('21', { exact: true })).toHaveCount(1)
    await expect(hCard.getByText('3', { exact: true })).toHaveCount(1)
    await expect(hCard.getByText('True', { exact: true })).toHaveCount(1)

    await expectNoVerboseHomeSurface(page)

    // Healthy Nodes stay compact: no routine prose or diagnostic line, and
    // exactly one whole-card link carries the Node identity.
    await expect(hCard.getByText('one or more observations are stale or unknown')).toHaveCount(0)
    await expect(page.getByRole('link', { name: /Node H/ })).toHaveCount(1)

    // Exceptional Nodes retain exactly one short Server-sanitized reason.
    const lCard = nodeCard(page, /Node L/)
    await expect(lCard.getByText('one or more observations are stale or unknown')).toHaveCount(1)

    // Summary cards: marker, title, number only, approximately 6rem high.
    const summaryFacts: Array<{ label: string; value: string }> = [
      { label: 'Active Nodes', value: '12' },
      { label: 'Healthy Nodes', value: '5' },
      { label: 'Attention', value: '7' },
      { label: 'Networks', value: '2' },
    ]
    for (const { label, value } of summaryFacts) {
      const card = page.getByRole('article').filter({ hasText: label })
      await expect(card).toHaveCount(1)
      // The card exposes marker, title, and number only - no explanatory
      // footer text.
      await expect(card).toHaveText(`${label} ${value}`, { useInnerText: true })
      const height = (await card.boundingBox())!.height
      expect(height, 'summary card must stay approximately 6rem high').toBeGreaterThanOrEqual(80)
      expect(height, 'summary card must stay approximately 6rem high').toBeLessThanOrEqual(120)
    }

    // Desktop 1280 renders two columns; tablet and phones render one. Read
    // the first two cards after the active Health sort instead of naming a
    // pair: adding another Active Node may legitimately shift row pairing.
    const activeNodeLinks = page.getByLabel('Active Nodes', { exact: true }).getByRole('link')
    const firstBox = (await activeNodeLinks.first().boundingBox())!
    const secondBox = (await activeNodeLinks.nth(1).boundingBox())!
    if (testInfo.project.name === 'desktop-1280') {
      expect(Math.abs(firstBox.y - secondBox.y)).toBeLessThanOrEqual(1)
    } else {
      expect(Math.abs(firstBox.y - secondBox.y)).toBeGreaterThan(1)
    }

    // The whole card is a 44px+ touch target and every visible control stays
    // touch-sized; long names and Unknown/Stale labels never overflow.
    const box = (await hCard.boundingBox())!
    expect(box.width).toBeGreaterThanOrEqual(44)
    expect(box.height).toBeGreaterThanOrEqual(44)
    await expectVisibleInteractiveTargets(page)
    await expectNoHorizontalOverflow(page)
  })

  test('production-like states stay explicit with readable text across viewports', async ({ page }) => {
    await loginAs(page)

    // Node K: missing Current Head Block Summary keeps TXS Unknown, an
    // authoritative empty peer set stays 0, current non-membership stays
    // False, and a Node without an effective Link has Unknown Activity.
    const kCard = nodeCard(page, /Node K/)
    await expect(kCard).toBeVisible({ timeout: 15_000 })
    await expect(kCard.getByText('Unknown', { exact: true })).toHaveCount(1)
    await expect(kCard.getByText('12,842,024', { exact: true })).toHaveCount(3)
    await expect(kCard.getByText('12,842,023', { exact: true })).toHaveCount(1)
    await expect(kCard.getByText('0', { exact: true })).toHaveCount(1)
    await expect(kCard.getByText('Empty; authoritative zero')).toBeVisible()
    await expect(kCard.getByText('False', { exact: true })).toHaveCount(1)
    await expect(kCard.getByText('Healthy', { exact: true })).toBeVisible()

    // Node L: stale last-good consensus keeps the values and marks them.
    const lCard = nodeCard(page, /Node L/)
    await expect(lCard.getByText('13', { exact: true })).toHaveCount(1)
    await expect(lCard.getByText('12,842,023', { exact: true })).toHaveCount(3)
    await expect(lCard.getByText('12,842,022', { exact: true })).toHaveCount(1)
    await expect(lCard.getByText('True', { exact: true })).toHaveCount(1)
    await expect(lCard.getByText('Stale', { exact: true })).toHaveCount(4)

    // Node M: effective Link with an authoritative no-live-validator result.
    const mCard = nodeCard(page, /Node M/)
    await expect(mCard.getByText('Observing', { exact: true })).toHaveCount(1)
    await expect(
      page.getByRole('link', { name: /Node M — Validator Observing[\s\S]*Observing[\s\S]*Healthy/ }),
    ).toHaveCount(1)

    // Node N: Provider error retains the last-good Activity and marks it
    // Stale without changing the independent Node Health badge.
    await expect(
      page.getByRole('link', { name: /Node N — Stale Last-Good[\s\S]*Locked \(Stale\)[\s\S]*Healthy/ }),
    ).toHaveCount(1)
    await expect(nodeCard(page, /Node N/).getByText('Locked (Stale)', { exact: true })).toHaveCount(1)

    // Node P has no Node observation; only the Agent-shared Host network
    // observation is known, and missing Node values never become 0 or False.
    const pCard = nodeCard(page, /Node P/)
    await expect(pCard.getByText('Unknown', { exact: true })).toHaveCount(8)
    await expect(pCard.getByText('0', { exact: true })).toHaveCount(0)
    await expect(pCard.getByText('False', { exact: true })).toHaveCount(0)
    await expect(pCard.getByText('one or more observations are stale or unknown')).toHaveCount(1)

    // Node A: the exact Current Head Block Summary proves TXS while the
    // current process, data-directory, and shared Host metrics stay explicit.
    const aCard = nodeCard(page, /Node A/)
    await expect(aCard.getByText('7', { exact: true })).toHaveCount(1)
    await expect(aCard.getByText('Healthy', { exact: true })).toBeVisible()

    await expectNoHorizontalOverflow(page)
  })

  test('Network filtering and Current Head sorting remain operable with the final card structure', async ({ page }) => {
    await loginAs(page)

    // Filter to the convergence Network: only its cards remain, and the long
    // display name never creates a nested link or overflow.
    await page.getByRole('button', { name: CONVERGENCE_NETWORK_NAME, exact: true }).click()
    await expect(nodeCard(page, /Node H/)).toBeVisible({ timeout: 15_000 })
    await expect(nodeCard(page, /Node A/)).toHaveCount(0)
    await expect(nodeCard(page, /Node P/)).toBeVisible()
    await expectNoHorizontalOverflow(page)

    await page.getByRole('button', { name: 'All Networks', exact: true }).click()
    await expect(nodeCard(page, /Node A/)).toBeVisible()
    // An authenticated Owner sees every Active Node on Home. The legacy
    // per-Node visibility value must not hide Node B from the site-wide Home
    // projection (Site Access Mode controls access to Home as a whole).
    await expect(nodeCard(page, /Node B \(private\)/)).toBeVisible()

    // Current Head sorting descends by the projected HEAD; never-observed
    // Nodes (Unknown) sort last instead of fabricating zero.
    await page.getByRole('combobox', { name: 'Sort' }).selectOption('head')
    let names = await nodeCardNames(page)
    expect(names[0]).toContain('Node H — Producing Card')
    const observedNodeIndex = names.findIndex((name) => name.includes('Node A'))
    expect(observedNodeIndex).toBeGreaterThanOrEqual(0)
    // Multiple Active Nodes can have an Unknown HEAD. Their relative order
    // is not part of Current Head sorting, but all stay below observed Nodes.
    expect(names.findIndex((name) => name.includes('Node G (transferred)'))).toBeGreaterThan(observedNodeIndex)
    expect(names.findIndex((name) => name.includes('Node P — Never Observed'))).toBeGreaterThan(observedNodeIndex)

    // Name sorting keeps the same whole-card navigation targets.
    await page.getByRole('combobox', { name: 'Sort' }).selectOption('name')
    names = await nodeCardNames(page)
    expect(names[0]).toContain('Node A')
    expect(names.at(-1)).toContain('Node P — Never Observed')
    await expectNoHorizontalOverflow(page)
  })

  test('the whole card is keyboard-activatable to Node Detail at every fixed viewport', async ({ page }) => {
    await loginAs(page)

    // Narrow to the convergence Network first so tab order is bounded, then
    // tab to the whole-card Node H link and activate with Enter.
    await page.getByRole('button', { name: CONVERGENCE_NETWORK_NAME, exact: true }).click()
    await expect(nodeCard(page, /Node H/)).toBeVisible({ timeout: 15_000 })
    await page.getByRole('button', { name: 'All Networks', exact: true }).focus()

    let activeHref = ''
    for (let i = 0; i < 30; i++) {
      await page.keyboard.press('Tab')
      activeHref = await page.evaluate(() => document.activeElement?.getAttribute('href') ?? '')
      if (activeHref === `/nodes/${NODE_H_ID}`) break
    }
    expect(activeHref).toBe(`/nodes/${NODE_H_ID}`)
    await expectFocusedElementHasVisibleFocus(page)
    await page.keyboard.press('Enter')
    await expect(page).toHaveURL(new RegExp(`/nodes/${NODE_H_ID}$`))
    await expect(
      page.getByRole('heading', { level: 1, name: /Node H — Producing Card/ }),
    ).toBeVisible({ timeout: 15_000 })

    // Home still works for the Owner session after returning.
    await page.getByRole('link', { name: 'PlatPulse', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Home' })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })
})

/** Assert every forbidden legacy/verbose affordance is absent from Home. */
async function expectNoVerboseHomeSurface(page: Page) {
  await expect(page.getByText('IS VALIDATOR', { exact: true })).toHaveCount(0)
  await expect(page.getByText('PROPOSER', { exact: true })).toHaveCount(0)
  await expect(page.getByText('Last Observed', { exact: true })).toHaveCount(0)
  await expect(page.getByText('View Node Details', { exact: true })).toHaveCount(0)
  // The Network display name inside a card is plain text, never a nested
  // link to the Network page.
  await expect(page.getByRole('link', { name: CONVERGENCE_NETWORK_NAME, exact: true })).toHaveCount(0)
  await expect(page.getByRole('link', { name: 'PlatON E2E Network', exact: true })).toHaveCount(0)
}
