import { expect, test, type Page } from '@playwright/test'
import {
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
  loginAs,
} from './helpers'

/**
 * Final cross-route acceptance for the converged WebUI (issue #95).
 *
 * The suite runs against ONE shared dev-mode platpulse-server serving the
 * production WebUI build (e2e/start-server.sh) with a single Playwright
 * worker (playwright.config.ts), so the four fixed viewport projects never
 * race mutations or lock SQLite. This spec is read-only: it walks the Public
 * monitoring flow, every retained Admin MVP section, the removed-route
 * fallback, and the semantic state vocabulary at every fixed viewport.
 */

const PUBLIC_NETWORK_NAME = 'PlatON E2E Network'
const PUBLIC_NETWORK_KEY = 'platon-e2e'
const PUBLIC_NODE_NAME = 'Node A'
const PUBLIC_NODE_ID = '0195f2a1-0014-4014-8014-000000000014'

/** The retained Admin MVP page groups (issue #92), in navigation order. The
 * security/configuration sections additionally declare the primary operation
 * affordance that must stay present on every viewport (their mutation flows
 * are exercised by configuration.spec.ts and access.spec.ts). */
const MVP_ADMIN_SECTIONS: Array<{
  link: string
  url: string
  heading: string
  primaryButton?: string | RegExp
  primaryHeading?: string
}> = [
  { link: 'Overview', url: '/admin', heading: 'Overview' },
  { link: 'Agents', url: '/admin/agents', heading: 'Agents' },
  { link: 'Nodes', url: '/admin/nodes', heading: 'Nodes' },
  { link: 'Networks', url: '/admin/networks', heading: 'Networks' },
  {
    link: 'History Window',
    url: '/admin/history-window',
    heading: 'History Window',
    primaryButton: 'Save History Window',
  },
  {
    link: 'Site Access',
    url: '/admin/site-access',
    heading: 'Site Access',
    primaryButton: /Make Home (Private|Public)/,
  },
  {
    link: 'Sessions',
    url: '/admin/access/sessions',
    heading: 'Sessions',
    primaryHeading: 'Active Sessions',
  },
  {
    link: 'Audit',
    url: '/admin/access/audit',
    heading: 'Audit log',
    primaryHeading: 'Events',
  },
]

/** Deferred/legacy Admin nav labels (issue #92): none may be linked from the
 * MVP navigation. Mirrors the removed-labels list in App.test.tsx. */
const REMOVED_ADMIN_LABELS = [
  'Validators',
  'People',
  'Alert Rules',
  'Incidents',
  'Silences',
  'Maintenance',
  'Deliveries',
  'Channels',
  'Operations',
  'Data',
  'Retention',
  'Backups',
  'Restore',
  'Doctor',
  'Enroll',
  'Recover',
  'Rotate',
]

/** Real legacy/deferred Admin routes that existed before issue #92 (same
 * paths the routed-seam unit test pins in App.test.tsx). Each drives the
 * direct-URL fallback check; a representative legacy heading proves the old
 * page is not rendered behind the safe fallback. */
const REMOVED_ADMIN_ROUTES: Array<{ route: string; heading?: string | RegExp }> = [
  { route: '/admin/validators', heading: 'Validators' },
  { route: '/admin/validators/v-1', heading: /Validators/ },
  { route: '/admin/access/people', heading: 'People' },
  { route: '/admin/alerts/rules', heading: 'Alert Rules' },
  { route: '/admin/alerts/incidents', heading: 'Incidents' },
  { route: '/admin/alerts/silences', heading: 'Silences' },
  { route: '/admin/alerts/maintenance', heading: 'Maintenance Windows' },
  { route: '/admin/alerts/deliveries', heading: 'Deliveries' },
  { route: '/admin/alerts/channels', heading: 'Channels' },
  { route: '/admin/operations', heading: 'Operations' },
  { route: '/admin/operations/o-1', heading: /Operations/ },
  { route: '/admin/data', heading: 'Data and maintenance' },
  { route: '/admin/data/retention', heading: 'Retention' },
  { route: '/admin/data/backups', heading: 'Backups' },
  { route: '/admin/data/restore', heading: 'Restore a backup' },
  { route: '/admin/data/doctor', heading: 'Doctor' },
  { route: '/admin/nodes/node-1/visibility', heading: /Node visibility/ },
  { route: '/admin/nodes/node-1/transfer', heading: 'Transfer Node ownership' },
  { route: '/admin/agents/agent-1/recover', heading: /Recover Agent/ },
  { route: '/admin/agents/agent-1/rotate', heading: /Rotate credential/ },
]

/** Sign in and land on the Admin Overview shell. */
async function openAdmin(page: Page) {
  await loginAs(page)
  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible({
    timeout: 15_000,
  })
}

/**
 * Open the tablet/mobile drawer unless it is already open (desktop keeps the
 * persistent sidebar). Toggling is driven by the button's own aria-expanded
 * state so a half-closed drawer animation can never flip it twice.
 */
async function openAdminNav(page: Page) {
  const menu = page.getByRole('button', { name: 'Menu' })
  if (await menu.isVisible()) {
    const expanded = await menu.getAttribute('aria-expanded')
    if (expanded !== 'true') await menu.click()
  }
}

/**
 * Ensure the drawer is open, then activate the Admin nav link by name
 * (desktop clicks the persistent sidebar directly).
 */
async function openAdminNavLink(page: Page, linkName: string) {
  await openAdminNav(page)
  await page.getByRole('link', { name: linkName, exact: true }).click()
}

test.describe('Converged WebUI acceptance (issue #95)', () => {
  test('Public Home → Node Detail → Network works at every fixed viewport', async ({ page }) => {
    await loginAs(page)
    await expect(page.getByRole('heading', { level: 1, name: 'Home' })).toBeVisible()

    // Home → Node Detail via the one whole-card link (issue #97).
    await page.getByRole('link', { name: new RegExp(PUBLIC_NODE_NAME) }).click()
    await expect(page).toHaveURL(new RegExp(`/nodes/${PUBLIC_NODE_ID}$`))
    await expect(page.getByRole('heading', { level: 1, name: PUBLIC_NODE_NAME })).toBeVisible({
      timeout: 15_000,
    })
    await expect(page.getByRole('tab', { name: 'Details' })).toHaveAttribute('aria-selected', 'true')
    await page.getByRole('tab', { name: 'Network' }).click()
    await expect(page.getByRole('tabpanel', { name: 'Network' })).toBeVisible()
    await page.getByRole('tab', { name: 'Details' }).click()
    await expect(page.getByRole('tabpanel', { name: 'Details' })).toBeVisible()

    // Node Detail → Network overview via the breadcrumb (the Home card
    // Network display name is plain text, not a nested link).
    await page.getByRole('link', { name: new RegExp(`← ${PUBLIC_NETWORK_KEY}`) }).click()
    await expect(page).toHaveURL(new RegExp(`/networks/${PUBLIC_NETWORK_KEY}$`))
    await expect(page.getByRole('heading', { level: 1, name: PUBLIC_NETWORK_NAME })).toBeVisible({
      timeout: 15_000,
    })
    await expectNoHorizontalOverflow(page)

    // Network → Node Detail.
    await page.getByRole('link', { name: PUBLIC_NODE_NAME }).click()
    await expect(page).toHaveURL(new RegExp(`/nodes/${PUBLIC_NODE_ID}$`))
    await expect(page.getByRole('heading', { level: 1, name: PUBLIC_NODE_NAME })).toBeVisible({
      timeout: 15_000,
    })

    // Responsive and keyboard/semantic contract at the fixed viewport.
    await expectVisibleInteractiveTargets(page)
    await expectNoHorizontalOverflow(page)

    // The flow returns home; the shell stays intact.
    await page.getByRole('link', { name: 'PlatPulse', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Home' })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('compact Home node cards keep one whole-card navigation target at every fixed viewport', async ({ page }) => {
    await loginAs(page)
    await expect(page.getByRole('heading', { level: 1, name: 'Home' })).toBeVisible()

    const cardLink = page.getByRole('link', { name: new RegExp(PUBLIC_NODE_NAME) }).first()
    await expect(cardLink).toBeVisible({ timeout: 15_000 })

    // Whole-card target: the link wraps the Node label and the Network name,
    // which stays plain text (no nested link), and the redundant
    // "View Node Details" affordance is absent from Home.
    await expect(cardLink).toContainText(PUBLIC_NETWORK_NAME)
    await expect(page.getByRole('link', { name: PUBLIC_NETWORK_NAME, exact: true })).toHaveCount(0)
    await expect(page.getByRole('link', { name: /View Node Details/ })).toHaveCount(0)

    // Touch target: the whole card is the interactive target, at least 44px.
    const box = await cardLink.boundingBox()
    expect(box!.width).toBeGreaterThanOrEqual(44)
    expect(box!.height).toBeGreaterThanOrEqual(44)

    // Keyboard: tab to the first whole-card Node link and activate with
    // Enter. The card order is Server-driven (attention first), so the
    // activated Node is the first Node link in tab order, not necessarily
    // the seeded Node A card; the URL it opens must be the focused card's.
    await page.keyboard.press('Tab')
    let activeHref = ''
    for (let i = 0; i < 25; i++) {
      activeHref = await page.evaluate(() => document.activeElement?.getAttribute('href') ?? '')
      if (activeHref.startsWith('/nodes/')) break
      await page.keyboard.press('Tab')
    }
    expect(activeHref).toMatch(/^\/nodes\//)
    await expectFocusedElementHasVisibleFocus(page)
    const escapedHref = activeHref.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
    await page.keyboard.press('Enter')
    await expect(page).toHaveURL(new RegExp(`${escapedHref}$`))
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible({ timeout: 15_000 })
    // The fixture-specific Node A card is still a single whole-card target
    // reachable from Home.
    await page.getByRole('link', { name: 'PlatPulse', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Home' })).toBeVisible()
    const nodeACard = page.getByRole('link', { name: new RegExp(PUBLIC_NODE_NAME) })
    await expect(nodeACard).toHaveCount(1)
    expect(await nodeACard.getAttribute('href')).toBe(`/nodes/${PUBLIC_NODE_ID}`)

    // Back Home; the compact shell must not overflow at the fixed viewport.
    await page.getByRole('link', { name: 'PlatPulse', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Home' })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('Admin MVP navigation reaches every retained section at every fixed viewport', async ({ page }) => {
    await openAdmin(page)

    for (const section of MVP_ADMIN_SECTIONS) {
      await openAdminNavLink(page, section.link)
      await expect(page).toHaveURL(section.url)
      await expect(page.getByRole('heading', { level: 1, name: section.heading })).toBeVisible({
        timeout: 15_000,
      })
      if (section.primaryButton) {
        await expect(page.getByRole('button', { name: section.primaryButton })).toBeVisible()
      }
      if (section.primaryHeading) {
        await expect(page.getByRole('heading', { level: 2, name: section.primaryHeading })).toBeVisible()
      }
      await expectNoHorizontalOverflow(page)
    }

    // Touch drawer keyboard cycle (design §10.1): Enter opens the drawer and
    // moves focus to the first Admin link; Escape closes it and restores
    // focus to the Menu opener, both with a visible focus ring.
    const adminNav = page.getByRole('navigation', { name: 'Admin' })
    const menu = page.getByRole('button', { name: 'Menu' })
    if (await menu.isVisible()) {
      await menu.focus()
      await page.keyboard.press('Enter')
      await expect(adminNav.getByRole('link').first()).toBeFocused()
      await expectFocusedElementHasVisibleFocus(page)
      await page.keyboard.press('Escape')
      await expect(menu).toBeFocused()
      await expectFocusedElementHasVisibleFocus(page)
      await openAdminNav(page)
    }

    // The MVP nav holds exactly the eight retained groups and nothing else.
    // Re-open the drawer on touch viewports so the closed navigation cannot
    // leave the accessibility tree mid-transition during the assertion.
    await expect(adminNav.getByRole('link')).toHaveCount(MVP_ADMIN_SECTIONS.length)
    for (const label of REMOVED_ADMIN_LABELS) {
      await expect(adminNav.getByRole('link', { name: label, exact: true })).toHaveCount(0)
    }
  })

  test('removed Admin routes resolve to the safe fallback and never to legacy pages', async ({ page }) => {
    await loginAs(page)

    for (const { route, heading } of REMOVED_ADMIN_ROUTES) {
      await page.goto(route)
      await expect(page.getByRole('heading', { level: 1, name: 'Section not found' })).toBeVisible({
        timeout: 15_000,
      })
      await expect(page).toHaveURL(route)
      if (heading) {
        await expect(page.getByRole('heading', { level: 1, name: heading })).toHaveCount(0)
      }
      await expectNoHorizontalOverflow(page)
    }

    // Unknown Admin paths fall back the same way.
    await page.goto('/admin/not-a-real-section')
    await expect(page.getByRole('heading', { level: 1, name: 'Section not found' })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('Unknown, Stale, Error and paused-live states remain semantically visible', async ({ page }) => {
    // A blocked Admin SSE stream surfaces the paused-live notice on every
    // Admin route while REST data keeps rendering (design §6.3).
    await page.route('**/api/admin/v1/events**', (route) => route.abort())
    await openAdmin(page)
    await openAdminNavLink(page, 'Nodes')
    await expect(page.getByRole('heading', { level: 1, name: 'Nodes' })).toBeVisible({
      timeout: 15_000,
    })
    await expect(page.getByText('Live updates paused', { exact: true })).toBeVisible()

    // Current/healthy, Stale/unhealthy and never-observed Unknown rows are
    // each their own server-owned dimension; nothing collapses into zero.
    const nodeARow = page.getByRole('row', { name: /Node A/ })
    await expect(nodeARow).toContainText('healthy', { timeout: 15_000 })
    await expect(nodeARow).toContainText('Current')

    const nodeBRow = page.getByRole('row', { name: /Node B \(private\)/ })
    await expect(nodeBRow).toContainText('unhealthy')
    await expect(nodeBRow).toContainText('Stale')

    const nodeCRow = page.getByRole('row', { name: /Node C/ })
    await expect(nodeCRow).toContainText('Unknown')
    await expectNoHorizontalOverflow(page)

    // Error keeps last-good evidence beside the failure on the detail route.
    await page.getByRole('link', { name: 'Node B (private)' }).click()
    await expect(page.getByRole('heading', { level: 1, name: /Node B \(private\)/ })).toBeVisible({
      timeout: 15_000,
    })
    await expect(page.getByText('RPC collection failed')).toBeVisible()
    await expect(page.getByText(/last-good head 12842018/)).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })
})
