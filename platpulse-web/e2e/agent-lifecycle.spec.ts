import { expect, test } from '@playwright/test'
import {
  E2E_VIEWER_PASSWORD,
  E2E_VIEWER_USERNAME,
  expectNoHorizontalOverflow,
  loginAs,
} from './helpers'

/**
 * Agent inventory and detail (PAGE-ADMIN-AGENTS, PAGE-ADMIN-AGENT-DETAIL).
 * Enrollment, recovery and credential rotation are deferred beyond the
 * MVP WebUI (issue #92): their routes are not registered, so this suite
 * covers the retained identity, liveness, credential and diagnostics
 * surface only.
 *
 * Read-only flows run on every fixed viewport project. Mutations create
 * Server state, so each mutation runs once on desktop-1280 only (same
 * discipline as the Owner Overview visibility mutation).
 */
const AGENT_ID = '0195f2a1-0011-4011-8011-000000000011'
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
  test('lists identity, liveness, boot/report, inventory, credentials, and diagnostics separately', async ({ page }) => {
    await openAgents(page)

    // Liveness and report state are separate from credentials and Inventory.
    const row = page.getByRole('row', { name: /0195f2a1-0011/ })
    await expect(row).toContainText('Current', { timeout: 15_000 })
    await expect(row).toContainText('#42')
    await expect(row).toContainText('6 Nodes')
    await expect(row).toContainText(/active/)
    await expect(row).toContainText('0 gaps · 0 security events')
    // Enrollment is deferred; no unavailable action is exposed in the MVP shell.
    await expect(page.getByRole('link', { name: 'Enroll a new Agent' })).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })
})

test.describe('Agent detail (PAGE-ADMIN-AGENT-DETAIL)', () => {
  test('keeps identity, liveness, credentials, inventory, diagnostics, and audit independent', async ({ page }) => {
    await openAgents(page)
    await page.getByRole('link', { name: AGENT_ID, exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: /Agent 0195f2a1/ })).toBeVisible()

    // Separate dimension panels (data-dependent; the Server is shared by
    // all parallel projects, so allow for load).
    await expect(page.getByRole('heading', { level: 2, name: 'Identity' })).toBeVisible({
      timeout: 15_000,
    })
    await expect(page.getByRole('heading', { level: 2, name: 'Liveness' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Boot and report state' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Inventory' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Credentials' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Diagnostics' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Audit trail' })).toBeVisible()

    // Credential dimension shows the non-sensitive id and active state.
    await expect(page.getByText(CREDENTIAL_ID, { exact: true })).toBeVisible()
    await expect(page.getByRole('button', { name: 'Revoke' })).toBeVisible()
    // Inventory stays per-Node, never merged at Agent level.
    await expect(page.getByText('Node A')).toBeVisible()
    // Actions are dedicated high-risk routes, not remote control.
    // Recovery and credential rotation are deferred; no unavailable actions
    // are exposed from the retained Agent detail surface.
    await expect(page.getByRole('link', { name: 'Rotate credential' })).toHaveCount(0)
    await expect(page.getByRole('link', { name: 'Recover agent' })).toHaveCount(0)
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
    await page.getByRole('link', { name: AGENT_ID, exact: true }).click()
    await expect(
      page.getByRole('heading', { level: 1, name: /Agent 0195f2a1/ }),
    ).toBeVisible()

    // The seeded credential is revoked through an explicit confirmation.
    const item = page.locator('.credential-item', { hasText: CREDENTIAL_ID })
    await item.getByRole('button', { name: 'Revoke' }).click()
    await expect(item.getByText(/Revoke now\?/)).toBeVisible()
    await item.getByRole('button', { name: 'Confirm revoke' }).click()

    await expect(page.getByText(/Credential revoked at/)).toBeVisible()
    // No optimistic state: the Server refetch shows the revoked dimension.
    await expect(item.getByText('Revoked', { exact: true })).toBeVisible({ timeout: 15_000 })
    await expect(item.getByRole('button', { name: 'Revoke' })).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })
})
