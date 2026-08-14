import { expect, test } from '@playwright/test'
import {
  E2E_VIEWER_PASSWORD,
  E2E_VIEWER_USERNAME,
  expectNoHorizontalOverflow,
  loginAs,
} from './helpers'

/**
 * Agent lifecycle operations (PAGE-ADMIN-AGENTS, PAGE-ADMIN-AGENT-DETAIL,
 * PAGE-ADMIN-ENROLL, PAGE-ADMIN-AGENT-RECOVER, PAGE-ADMIN-AGENT-ROTATE).
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
    const row = page.getByRole('row', { name: /0195f2a1/ })
    await expect(row).toContainText('Current', { timeout: 15_000 })
    await expect(row).toContainText('#42')
    await expect(row).toContainText('5 Nodes')
    await expect(row).toContainText(/active/)
    await expect(row).toContainText('0 gaps · 0 security events')
    await expect(page.getByRole('link', { name: 'Enroll a new Agent' })).toBeVisible()
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
    await expect(page.getByText(CREDENTIAL_ID)).toBeVisible()
    await expect(page.getByRole('button', { name: 'Revoke' })).toBeVisible()
    // Inventory stays per-Node, never merged at Agent level.
    await expect(page.getByText('Node A')).toBeVisible()
    // Actions are dedicated high-risk routes, not remote control.
    await expect(page.getByRole('link', { name: 'Rotate credential' })).toBeVisible()
    await expect(page.getByRole('link', { name: 'Recover agent' })).toBeVisible()
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

  test('enrollment shows the one-time secret only in the success response', async ({ page }) => {
    await page.getByRole('link', { name: 'Enroll a new Agent' }).click()
    await expect(
      page.getByRole('heading', { level: 1, name: 'Enroll a new Agent' }),
    ).toBeVisible()
    await page.getByRole('button', { name: 'Create enrollment token' }).click()

    const secretBox = page.locator('.secret-value')
    await expect(secretBox).toBeVisible({ timeout: 15_000 })
    const secret = (await secretBox.textContent()) ?? ''
    expect(secret).toMatch(/^pp_enroll_[0-9a-f-]+_[0-9a-f]{64}$/)

    // PATTERN-SECRET-ONCE: never in URL/query state or browser history.
    expect(page.url()).not.toContain(secret)
    expect(await page.evaluate(() => JSON.stringify(window.history.state))).not.toContain(secret)
    await expect(page.locator('.secret-warning')).toBeVisible()

    // Leaving the success view discards the secret; it is not recoverable.
    await page.getByRole('link', { name: 'Back to Agents' }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Agents' })).toBeVisible()
    await expect(page.locator('.secret-value')).toHaveCount(0)
    expect(page.url()).not.toContain(secret)
    await expectNoHorizontalOverflow(page)
  })

  test('recovery issues a one-time token and links to the redacted audit trail', async ({ page }) => {
    await page.getByRole('link', { name: AGENT_ID, exact: true }).click()
    await expect(
      page.getByRole('heading', { level: 1, name: /Agent 0195f2a1/ }),
    ).toBeVisible()
    await page.getByRole('link', { name: 'Recover agent' }).click()
    await expect(
      page.getByRole('heading', { level: 1, name: /Recover Agent 0195f2a1/ }),
    ).toBeVisible()
    // PATTERN-CONFIRMATION: the high-risk action requires the typed phrase.
    await expect(page.getByRole('button', { name: 'Create recovery token' })).toBeDisabled()
    await page.getByLabel(/I understand: recovery advances/).check()
    await page.getByRole('button', { name: 'Create recovery token' }).click()

    const secretBox = page.locator('.secret-value')
    await expect(secretBox).toBeVisible({ timeout: 15_000 })
    const secret = (await secretBox.textContent()) ?? ''
    expect(secret).toMatch(/^pp_recover_[0-9a-f-]+_[0-9a-f]{64}$/)
    expect(page.url()).not.toContain(secret)
    // Recovery advances the Epoch without a duplicate Agent.
    await expect(page.getByText(/Epoch advances from 1 to 2/)).toBeVisible()
    await expect(page.getByText(/never duplicated/)).toBeVisible()
    // Every security mutation carries the redacted Audit link.
    await expect(page.getByRole('link', { name: 'view Agent audit' })).toBeVisible()
  })

  test('rotation shows the new credential once with overlap context', async ({ page }) => {
    await page.getByRole('link', { name: AGENT_ID, exact: true }).click()
    await page.getByRole('link', { name: 'Rotate credential' }).click()
    await expect(
      page.getByRole('heading', { level: 1, name: /Rotate credential/ }),
    ).toBeVisible()
    // PATTERN-CONFIRMATION: the security mutation requires the typed phrase.
    await expect(page.getByRole('button', { name: 'Rotate credential' })).toBeDisabled()
    await page.getByLabel(/I understand: rotation issues/).check()
    await page.getByRole('button', { name: 'Rotate credential' }).click()

    const secretBox = page.locator('.secret-value')
    await expect(secretBox).toBeVisible({ timeout: 15_000 })
    const secret = (await secretBox.textContent()) ?? ''
    expect(secret).toMatch(/^pp_agent_[0-9a-f-]+_[0-9a-f]{64}$/)
    expect(page.url()).not.toContain(secret)
    await expect(page.getByText(/overlap 24 hours/)).toBeVisible()
    await expect(page.getByText(/stay valid until/)).toBeVisible()
    // Rotation is distinct from recovery: the Epoch is untouched.
    await expect(page.getByText(/Agent Epoch was not changed/)).toBeVisible()
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
