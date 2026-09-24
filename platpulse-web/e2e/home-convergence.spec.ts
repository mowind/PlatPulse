import { expect, test, type Page } from '@playwright/test'
import {
  expectFocusedElementHasVisibleFocus,
  expectMetricRowsAligned,
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

const nodeCard = (page: Page, name: RegExp) => page.getByRole('link', { name }).locator('xpath=ancestor::article[1]')

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
    await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()

    const hCard = nodeCard(page, /Node H/)
    await expect(hCard).toBeVisible({ timeout: 15_000 })

    // Header: the two-state Node Health marker precedes the Node/Network
    // identity and the Validator Activity badge (ADR 0006) sits at the
    // top-right. The removed Health badge stays absent; the title is the
    // semantic link and the independent marker and sibling controls keep their
    // own accessible names.
    await expect(hCard.locator('[data-slot="status-badge"]')).toHaveCount(0)
    await expect(
      page.getByRole('link', { name: /^Node H — Producing Card/ }),
    ).toHaveCount(1)

    // Business rows carry full labels. The header's top-right badge is PlatScan
    // Validator Activity (ADR 0006), not consensus membership, so no business
    // row label collides with it.
    for (const label of ['Head', 'Txs', 'Peers', 'QC', 'Locked', 'Committed']) {
      await expect(hCard.getByText(label, { exact: true })).toBeVisible()
    }
    await expect(hCard.getByRole('img', { name: 'Healthy' })).toBeVisible()
    // Resources/counts remain inline; chain/consensus units stack label over value.
    // The shared geometry helper checks each unit's declared presentation.
    await expectMetricRowsAligned(hCard)
    // Head, QC, and Committed share the height; each appears once per row.
    await expect(hCard.getByText('12,842,025', { exact: true })).toHaveCount(3)
    await expect(hCard.getByText('12,842,024', { exact: true })).toHaveCount(1)
    await expect(hCard.getByText('21', { exact: true })).toHaveCount(1)
    await expect(hCard.getByText('3', { exact: true })).toHaveCount(1)
    await expect(hCard.locator('[data-slot="validator-activity"]')).toHaveAttribute('data-activity', 'producing')
    await expect(hCard.locator('[data-slot="validator-activity"] [data-slot="validator-activity-label"]')).toHaveText('Producing')

    // The identity block keeps the name row and the Network · Uptime helper
    // about 6px apart without the 44px controls inflating either line. Its
    // reserved region height then starts the resource region at the same
    // offset in every card, so only the header-to-resource gap is asserted.
    const identityGap = await hCard.evaluate(card => {
      const link = card.querySelector('h2 a') as HTMLElement
      const helper = card.querySelector('[data-slot="card-x-header"] p') as HTMLElement
      const header = card.querySelector('[data-slot="card-x-header"]') as HTMLElement
      const resources = card.querySelector('[aria-label="Node process and host network resources"]') as HTMLElement
      const linkBox = link.getBoundingClientRect()
      const helperBox = helper.getBoundingClientRect()
      // The 44px anchor centres its 24px name line, so the visible name bottom
      // is 10px above the anchor's border box.
      return { nameToHelper: helperBox.top - (linkBox.bottom - 10), headerToResources: resources.getBoundingClientRect().top - header.getBoundingClientRect().bottom }
    })
    expect(identityGap.nameToHelper, 'name-to-helper gap').toBeGreaterThanOrEqual(4)
    expect(identityGap.nameToHelper, 'name-to-helper gap').toBeLessThanOrEqual(10)
    expect(Math.abs(identityGap.headerToResources), 'the resource region starts at the reserved identity boundary').toBeLessThanOrEqual(1)

    await expectNoVerboseHomeSurface(page)

    // Healthy Nodes stay compact: no diagnostic placeholder is rendered, and
    // exactly one whole-card link carries the Node identity.
    await expect(hCard.locator('[data-slot="node-diagnostic"]')).toHaveCount(0)
    await expect(page.getByRole('link', { name: /Node H/ })).toHaveCount(1)

    // CONTEXT.md: the reason for an abnormal Node Health state stays visible as
    // text, on its own reserved line.
    const lCard = nodeCard(page, /Node L/)
    await expect(lCard.getByText('one or more observations are stale or unknown')).toHaveCount(1)

    // The four non-cumulative summary cards stay marker, title and number;
    // only the two cumulative cards add their scope/coverage stat line.
    const summaryFacts: Array<{ label: string; value: string }> = [
      { label: 'Active Nodes', value: '12' },
      { label: 'Healthy Nodes', value: '5' },
      { label: 'Attention', value: '7' },
      { label: 'Networks', value: '2' },
    ]
    for (const { label, value } of summaryFacts) {
      const card = page.getByRole('article', { name: label, exact: true })
      await expect(card).toHaveCount(1)
      // The card exposes marker, title, and number only - no explanatory
      // footer text.
      await expect(card).toHaveText(`${label} ${value}`, { useInnerText: true })
      const height = (await card.boundingBox())!.height
      // The tiles keep the compact height of the original four-card 2x2 grid:
      // a 44px control-sized header row, the value and the shared stat slot.
      // The two cumulative tiles add their scope/coverage line to that slot,
      // and auto-rows-fr gives all six the same (possibly wrapped) height.
      expect(height, 'summary card keeps the restored compact height').toBeGreaterThanOrEqual(80)
      expect(height, 'summary card stays compact').toBeLessThanOrEqual(160)
    }

    // The cumulative tiles print their own Network scope, coverage and Partial
    // marker directly on the face; coverage is no longer tooltip-only.
    for (const label of ['Cumulative blocks', 'Cumulative rewards']) {
      const caption = page.getByRole('article', { name: label, exact: true }).locator('[data-slot="summary-caption"]')
      await expect(caption).toContainText('2 Networks')
      await expect(caption).toContainText(/known|Coverage unknown/)
    }

    // The Node grid fits as many 360px columns as the content width allows:
    // several at 1280px, two at 768px, one on a phone. Read the first two
    // cards after the active Health sort instead of naming a pair: adding
    // another Active Node may legitimately shift row pairing.
    const activeNodeCards = page.locator('[aria-label="Active Nodes"] [data-slot="node-card"]')
    // Park the pointer before measuring: a hovered card lifts by 2px, which
    // would otherwise read as a different grid row.
    await page.mouse.move(4, 4)
    await page.waitForTimeout(250)
    const firstBox = (await activeNodeCards.first().boundingBox())!
    const secondBox = (await activeNodeCards.nth(1).boundingBox())!
    if (testInfo.project.name === 'phone-360-touch' || testInfo.project.name === 'phone-390-touch') {
      expect(Math.abs(firstBox.y - secondBox.y), 'a phone renders one Node column').toBeGreaterThan(1)
    } else {
      expect(Math.abs(firstBox.y - secondBox.y), 'desktop and tablet render at least two Node columns').toBeLessThanOrEqual(1)
    }

    // The whole card is a 44px+ touch target and every visible control stays
    // touch-sized; long names and Unknown/Stale labels never overflow.
    const box = (await hCard.boundingBox())!
    expect(box.width).toBeGreaterThanOrEqual(44)
    expect(box.height).toBeGreaterThanOrEqual(44)

    // The independent Node Health marker stays inside the card at every viewport.
    const badgeCard = nodeCard(page, /Node A/)
    await expect(badgeCard).toBeVisible({ timeout: 15_000 })
    const badgeCardBox = (await badgeCard.boundingBox())!
    await expect(badgeCard.locator('[data-slot="status-badge"]')).toHaveCount(0)
    const healthMarker = badgeCard.getByRole('img', { name: 'Healthy' })
    await expect(healthMarker).toBeVisible()
    const markerBox = (await healthMarker.boundingBox())!
    expect(markerBox.x).toBeGreaterThanOrEqual(badgeCardBox.x - 1)
    expect(markerBox.x + markerBox.width).toBeLessThanOrEqual(badgeCardBox.x + badgeCardBox.width + 1)
    await expectVisibleInteractiveTargets(page)
    await expectNoHorizontalOverflow(page)
  })

  test('production-like states stay explicit with readable text across viewports', async ({ page }) => {
    await loginAs(page)

    // Node K: missing Current Head Block Summary keeps Txs Unknown, an
    // authoritative empty peer set stays 0, and a Node without an effective
    // Link presents the unified Observing Activity with its real reason.
    const kCard = nodeCard(page, /Node K/)
    await expect(kCard).toBeVisible({ timeout: 15_000 })
    // Txs and resource values remain Unknown; uptime now belongs to the identity line.
    for (const label of ['Txs', 'CPU', 'Memory', 'Node data']) {
      const row = kCard.locator('[data-slot="metric-row"]').filter({ has: page.getByText(label, { exact: true }) })
      await expect(row.locator('[data-slot="metric-row-value"]')).toHaveText('Unknown')
    }
    await expect(kCard.getByLabel('Upload Unknown')).toBeVisible()
    await expect(kCard.getByLabel('Download Unknown')).toBeVisible()
    await expect(kCard.getByText('Uptime Unknown', { exact: true })).toBeVisible()
    await expect(kCard.getByText('12,842,024', { exact: true })).toHaveCount(3)
    await expect(kCard.getByText('12,842,023', { exact: true })).toHaveCount(1)
    await expect(kCard.getByText('0', { exact: true })).toHaveCount(1)
    await expect(kCard.getByText('Empty; authoritative zero')).toBeVisible()
    await expect(kCard.locator('[data-slot="validator-activity"]')).toHaveAttribute('data-activity', 'observing')
    await expect(kCard.getByRole('img', { name: 'Healthy' })).toBeVisible()

    // Node L: stale last-good consensus keeps the values and marks them.
    const lCard = nodeCard(page, /Node L/)
    await expect(lCard.getByText('13', { exact: true })).toHaveCount(1)
    await expect(lCard.getByText('12,842,023', { exact: true })).toHaveCount(3)
    await expect(lCard.getByText('12,842,022', { exact: true })).toHaveCount(1)
    await expect(lCard.locator('[data-slot="validator-activity"]')).toHaveAttribute('data-activity', 'observing')
    // The three stale consensus values (QC, Locked, Committed) print the Stale
    // detail; the removed Node-role chip used to carry a fourth.
    await expect(lCard.getByText('Stale', { exact: true })).toHaveCount(3)

    // Node M: effective Link with an authoritative no-live-validator result.
    // The Activity badge unifies to Observing while the explanation names the
    // real reason (ADR 0006).
    const mCard = nodeCard(page, /Node M/)
    await expect(mCard.locator('[data-slot="status-badge"]')).toHaveCount(0)
    await expect(mCard.locator('[data-slot="validator-activity"]')).toHaveAttribute('data-activity', 'observing')
    await expect(
      page.getByRole('link', { name: /^Node M — Validator Observing/ }),
    ).toHaveCount(1)

    // Node N: a Provider error keeps the last-good Activity and marks it stale,
    // and the independent Node Health marker is unchanged by it.
    await expect(
      page.getByRole('link', { name: /^Node N — Stale Last-Good/ }),
    ).toHaveCount(1)
    const nCard = nodeCard(page, /Node N/)
    await expect(nCard.locator('[data-slot="validator-activity"]')).toHaveAttribute('data-activity', 'locked')
    await expect(nCard.locator('[data-slot="validator-activity"]')).toHaveAttribute('data-stale', 'true')
    await expect(mCard.getByRole('img', { name: 'Healthy' })).toBeVisible()
    await expect(nCard.getByRole('img', { name: 'Healthy' })).toBeVisible()
    await expect(nCard.locator('[data-slot="status-badge"]')).toHaveCount(0)

    // Node P has no Node observation; only the Agent-shared Host network
    // observation is known, and missing Node values never become 0 or No.
    const pCard = nodeCard(page, /Node P/)
    // All absent metrics are explicit, including resource and uptime values.
    for (const label of ['CPU', 'Memory', 'Node data', 'Head', 'Txs', 'Peers', 'QC', 'Locked', 'Committed']) {
      const row = pCard.locator('[data-slot="metric-row"]').filter({ has: page.getByText(label, { exact: true }) })
      await expect(row.locator('[data-slot="metric-row-value"]')).toHaveText('Unknown')
    }
    await expect(pCard.getByText('Uptime Unknown', { exact: true })).toBeVisible()
    await expect(pCard.locator('[data-slot="validator-activity"]')).toHaveAttribute('data-activity', 'observing')
    await expect(pCard.locator('[data-slot="validator-activity"]')).toHaveAttribute('data-stale', 'false')
    await expect(pCard.getByText('0', { exact: true })).toHaveCount(0)
    await expect(pCard.getByText('Non-validator', { exact: true })).toHaveCount(0)
    await expect(pCard.getByText('one or more observations are stale or unknown')).toHaveCount(1)

    // Node A: the exact Current Head Block Summary proves Txs while the
    // current process, data-directory, and shared Host metrics stay explicit.
    const aCard = nodeCard(page, /Node A/)
    await expect(aCard.getByText('7', { exact: true })).toHaveCount(1)
    await expect(aCard.getByRole('img', { name: 'Healthy' })).toBeVisible()

    await expectNoHorizontalOverflow(page)
  })

  test('Network filtering and Current Head sorting remain operable with the final card structure', async ({ page }) => {
    await loginAs(page)

    // Filter to the convergence Network: only its cards remain, and the long
    // display name never creates a nested link or overflow.
    await page.getByRole('tab', { name: CONVERGENCE_NETWORK_NAME, exact: true }).click()
    await expect(nodeCard(page, /Node H/)).toBeVisible({ timeout: 15_000 })
    await expect(nodeCard(page, /Node A/)).toHaveCount(0)
    await expect(nodeCard(page, /Node P/)).toBeVisible()
    await expectNoHorizontalOverflow(page)

    await page.getByRole('tab', { name: 'All Networks', exact: true }).click()
    await expect(nodeCard(page, /Node A/)).toBeVisible()
    // An authenticated Owner sees every Active Node on Home. The legacy
    // per-Node visibility value must not hide Node B from the site-wide Home
    // projection (Site Access Mode controls access to Home as a whole).
    await expect(nodeCard(page, /Node B \(private\)/)).toBeVisible()

    // Current Head sorting descends by the projected Head; never-observed
    // Nodes (Unknown) sort last instead of fabricating zero.
    await page.getByRole('combobox', { name: 'Sort' }).selectOption('head')
    let names = await nodeCardNames(page)
    expect(names[0]).toContain('Node H — Producing Card')
    const observedNodeIndex = names.findIndex((name) => name.includes('Node A'))
    expect(observedNodeIndex).toBeGreaterThanOrEqual(0)
    // Multiple Active Nodes can have an Unknown Head. Their relative order
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
    await page.getByRole('tab', { name: CONVERGENCE_NETWORK_NAME, exact: true }).click()
    await expect(nodeCard(page, /Node H/)).toBeVisible({ timeout: 15_000 })
    await page.getByRole('tab', { name: 'All Networks', exact: true }).focus()

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
    await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('keeps the converged Home intact in the dark theme (issue #147)', async ({ page }) => {
    await loginAs(page)
    const hCard = nodeCard(page, /Node H/)
    await expect(hCard).toBeVisible({ timeout: 15_000 })

    // Auto → Light → Dark through the shared theme control.
    const theme = page.getByRole('button', { name: /^Theme: / })
    await theme.click()
    await theme.click()
    expect(await page.evaluate(() => document.documentElement.classList.contains('dark'))).toBe(true)

    // The compact card contract, long names, and explicit Unknown/Stale states
    // are unchanged by the theme.
    await expect(hCard).toBeVisible()
    await expect(hCard.getByText('Head', { exact: true })).toBeVisible()
    await expect(nodeCard(page, /Node P — Never Observed/)).toBeVisible()
    await expect(nodeCard(page, /Node L/).getByText('Stale', { exact: true })).toHaveCount(3)

    // Filtering stays operable on the dark surface and bounds the tab order.
    await page.getByRole('tab', { name: CONVERGENCE_NETWORK_NAME, exact: true }).click()
    await expect(nodeCard(page, /Node H/)).toBeVisible()

    // The whole-card link keeps an independent visible focus ring in Dark.
    await page.getByRole('tab', { name: CONVERGENCE_NETWORK_NAME, exact: true }).focus()
    let activeHref = ''
    for (let index = 0; index < 30; index++) {
      await page.keyboard.press('Tab')
      activeHref = await page.evaluate(() => document.activeElement?.getAttribute('href') ?? '')
      if (activeHref === `/nodes/${NODE_H_ID}`) break
    }
    expect(activeHref).toBe(`/nodes/${NODE_H_ID}`)
    await expectFocusedElementHasVisibleFocus(page)

    // Sorting stays operable and the full list comes back.
    await page.getByRole('combobox', { name: 'Sort' }).selectOption('head')
    await page.getByRole('tab', { name: 'All Networks', exact: true }).click()
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
