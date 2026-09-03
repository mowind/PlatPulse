import { expect, test, type Page } from '@playwright/test'
import {
  E2E_VIEWER_PASSWORD,
  E2E_VIEWER_USERNAME,
  expectNoHorizontalOverflow,
  loginAs,
} from './helpers'

/**
 * SCN-ACCESS-ROLE-CHANGE, SCN-AUTH-SESSION-REVOKED, Guest enable/disable,
 * and Audit review (issue #47, webui.md §12). The People page is deferred
 * beyond the MVP WebUI (issue #92), so user and role mutations run
 * through the Admin API; the Server policies (session revocation on role
 * change, Audit immutability) are unchanged. The suite runs against one
 * shared Server, so every test that mutates access state (users,
 * Sessions, Guest access) runs only on the desktop project, exactly like
 * the shared Node E metadata mutation in admin-overview.spec.ts; the
 * remaining projects still exercise the read-only workflows and the
 * fixed-viewport matrix.
 */
test.describe.configure({ mode: 'serial' })

const OPS_USERNAME = 'ops'
const OPS_PASSWORD = 'platpulse-e2e-ops-2026'
const ALICE_USERNAME = 'alice'
const ALICE_PASSWORD = 'platpulse-e2e-alice-2026'

/** Change a user's role through the real Admin API. The People page is
 * deferred beyond the MVP WebUI (issue #92); the Server capability and
 * the session-revocation policy are unchanged. The mutation runs inside
 * the page (like createUserAs) so the browser sends a same-origin Origin
 * header; the Server's mutation guard requires it alongside the CSRF
 * token (design §12.4). */
async function setPersonRole(page: Page, username: string, role: string): Promise<void> {
  const csrf = await page.evaluate(async () => {
    const response = await fetch('/api/public/v1/session')
    const body = (await response.json()) as { csrfToken: string }
    return body.csrfToken
  })
  const result = await page.evaluate(
    async ({ username, role, csrf }) => {
      const list = await fetch('/api/admin/v1/people')
      const payload = (await list.json()) as {
        users: Array<{ userId: string; username: string }>
      }
      const user = payload.users.find((entry) => entry.username === username)
      if (!user) return { status: 404, body: { missing: username } }
      const response = await fetch(`/api/admin/v1/people/${user.userId}/role`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': csrf },
        body: JSON.stringify({ role }),
      })
      return { status: response.status, body: await response.json() }
    },
    { username, role, csrf },
  )
  expect(result.status, JSON.stringify(result.body)).toBe(200)
}

/** Create a user through the real Admin API from an authenticated page. */
async function createUserAs(
  page: Page,
  username: string,
  password: string,
  role: string,
): Promise<void> {
  const csrf = await page.evaluate(async () => {
    const response = await fetch('/api/public/v1/session')
    const body = (await response.json()) as { csrfToken: string }
    return body.csrfToken
  })
  const result = await page.evaluate(
    async ({ username, password, role, csrf }) => {
      const response = await fetch('/api/admin/v1/people', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': csrf },
        body: JSON.stringify({ username, password, role }),
      })
      return { status: response.status, body: await response.json() }
    },
    { username, password, role, csrf },
  )
  expect(result.status, JSON.stringify(result.body)).toBe(200)
}

test.describe('PAGE-ACCESS-SESSIONS (Human Sessions)', () => {
  test('revoking a Session closes the other tab and never flashes Admin data', async (
    { browser },
    testInfo,
  ) => {
    // Mutates Session state; only the desktop project runs it so parallel
    // projects never lose their own admin Sessions.
    test.skip(testInfo.project.name !== 'desktop-1280', 'session revoke runs once')

    // A dedicated Owner keeps this scenario hermetic: only alice's two
    // Sessions exist, so the row-level revoke is deterministic and no
    // other suite's Session is touched.
    const ownerPage = await browser.newPage()
    await loginAs(ownerPage)
    await createUserAs(ownerPage, ALICE_USERNAME, ALICE_PASSWORD, 'owner')
    await ownerPage.close()

    const contextA = await browser.newContext()
    const pageA = await contextA.newPage()
    await loginAs(pageA, ALICE_USERNAME, ALICE_PASSWORD)
    await pageA.goto('/admin')
    await expect(pageA.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()

    const contextB = await browser.newContext()
    const pageB = await contextB.newPage()
    await loginAs(pageB, ALICE_USERNAME, ALICE_PASSWORD)
    await pageB.goto('/admin/access/sessions')
    await expect(pageB.getByRole('heading', { level: 1, name: 'Sessions' })).toBeVisible()

    // Exactly two alice Sessions; the current one is marked, so revoke
    // the other (context A's) row.
    const aliceRows = pageB.getByRole('row').filter({ hasText: ALICE_USERNAME })
    await expect(aliceRows).toHaveCount(2)
    const otherRow = aliceRows.filter({ hasNotText: 'This session' })
    await otherRow.getByRole('button', { name: 'Revoke' }).click()
    await otherRow.getByRole('button', { name: 'Confirm revoke' }).click()
    await expect(pageB.getByText(new RegExp(`Session for ${ALICE_USERNAME} revoked`))).toBeVisible()

    // Context A's Admin stream closes; it lands on the non-leaking login
    // path with the revoked explanation and no Admin data flash.
    await expect(pageA).toHaveURL(/\/login$/, { timeout: 10_000 })
    await expect(pageA.getByText('Your session expired or was revoked.')).toBeVisible()
    await expect(pageA.getByRole('heading', { level: 1, name: 'Sign in to PlatPulse' })).toBeVisible()
    await expect(pageA.getByText('Overview')).toHaveCount(0)
    await expectNoHorizontalOverflow(pageB)

    // Re-login restores full access under a fresh generation.
    await loginAs(pageA, ALICE_USERNAME, ALICE_PASSWORD)
    await pageA.goto('/admin')
    await expect(pageA.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
    await contextA.close()
    await contextB.close()
  })
})

test.describe('SCN-ACCESS-ROLE-CHANGE', () => {
  test('a role change revokes the live Session and re-authorizes on re-login', async (
    { browser },
    testInfo,
  ) => {
    // Depends on the desktop-only ops user and mutates roles.
    test.skip(testInfo.project.name !== 'desktop-1280', 'role change runs once')

    // Create ops as a Viewer through the Admin API, then promote them:
    // the People page is deferred beyond the MVP WebUI (issue #92) while
    // the Server capability is unchanged. The Server revokes ops's live
    // Session immediately on the role change.
    const ownerContext = await browser.newContext()
    const ownerPage = await ownerContext.newPage()
    await loginAs(ownerPage)
    await createUserAs(ownerPage, OPS_USERNAME, OPS_PASSWORD, 'viewer')

    // ops signs in as a Viewer first, with an open Admin attempt.
    const opsContext = await browser.newContext()
    const opsPage = await opsContext.newPage()
    await loginAs(opsPage, OPS_USERNAME, OPS_PASSWORD)
    await expect(opsPage.getByRole('region', { name: 'Home' })).toBeVisible()
    await opsPage.goto('/admin')
    await expect(opsPage.getByRole('heading', { level: 1, name: 'Owner access required' })).toBeVisible()

    await setPersonRole(ownerPage, OPS_USERNAME, 'owner')

    // The Viewer's blocked panel is intentionally static and non-leaking,
    // so the next navigation re-validates and lands on the login path (no
    // prior data ever flashes).
    await opsPage.goto('/')
    await expect(opsPage).toHaveURL(/\/login$/, { timeout: 10_000 })
    await expect(opsPage.getByRole('heading', { level: 1, name: 'Sign in to PlatPulse' })).toBeVisible()

    // A fresh login carries the Owner role and reaches Admin.
    await loginAs(opsPage, OPS_USERNAME, OPS_PASSWORD)
    await opsPage.goto('/admin')
    await expect(opsPage.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
    await expectNoHorizontalOverflow(opsPage)

    // Cleanup: restore the Viewer role so later runs see the seeded state.
    // (ownerPage is still authenticated; no re-login is needed.)
    await setPersonRole(ownerPage, OPS_USERNAME, 'viewer')
    await opsContext.close()
    await ownerContext.close()
  })
})


test.describe('PAGE-ACCESS-AUDIT (Audit review)', () => {
  test('lists immutable redacted events and filters by kind', async ({ page }, testInfo) => {
    // The asserted event counts include the desktop-only mutations.
    test.skip(testInfo.project.name !== 'desktop-1280', 'audit listing runs once')
    await loginAs(page)
    await page.goto('/admin/access/audit')
    await expect(page.getByRole('heading', { level: 1, name: 'Audit log' })).toBeVisible()

    const auditItems = page.locator('.audit-item')
    // The unfiltered listing is the newest 50 events; the whole parallel
    // suite generates hundreds of session_created events, so the seeded
    // and suite mutations are asserted through the Server-side filters
    // (which apply before the page limit). The suite runs against a
    // single-connection SQLite Server, so waits are generous.
    await expect(auditItems.first()).toBeVisible({ timeout: 15_000 })

    // Each filter change keeps the current page mounted while the new
    // query runs, so the waits assert CONTENT, not
    // counts: every rendered item must match the selected kind before the
    // next step reads the list.
    await page.getByLabel('Event kind').selectOption('viewer_created')
    await expect(
      auditItems.filter({ hasNotText: 'viewer_created' }),
    ).toHaveCount(0, { timeout: 15_000 })
    // The seeded Viewer plus the created ops user: exactly two creations.
    await expect(auditItems).toHaveCount(2, { timeout: 15_000 })

    // The promote/demote pair from the role-change scenario.
    await page.getByLabel('Event kind').selectOption('user_role_changed')
    await expect(
      auditItems.filter({ hasNotText: 'user_role_changed' }),
    ).toHaveCount(0, { timeout: 15_000 })
    await expect(auditItems).toHaveCount(2, { timeout: 15_000 })

    // The row-level revoke filter returns only revoke events (other
    // suites also sign out, so the exact count is not asserted — only
    // that the Server-side filter restricts every item to the kind).
    await page.getByLabel('Event kind').selectOption('session_revoked')
    await expect(
      auditItems.filter({ hasNotText: 'session_revoked' }),
    ).toHaveCount(0, { timeout: 15_000 })
    await expect(auditItems.first()).toBeVisible({ timeout: 15_000 })
    const filteredCount = await auditItems.count()
    expect(filteredCount).toBeGreaterThan(0)
    await expect(auditItems.filter({ hasText: 'session_revoked' })).toHaveCount(filteredCount)

    // Redacted details expand without secret material. Some revoke events
    // (e.g. sign-out revocations) legitimately carry no detail body, so
    // either the redacted key/value list or the explicit no-detail note
    // must render — never a token or hash.
    await auditItems.first().getByRole('button', { name: 'Show details' }).click()
    const details = auditItems.first().locator('.audit-details')
    await expect(details).toBeVisible()
    await expect(details.getByText(/pp_session_/)).toHaveCount(0)
    await expect(details.getByText(/\$argon2id/)).toHaveCount(0)
    const hasValues = await details.getByText(/username|sessionId/).count()
    const hasNote = await details
      .getByText('No redacted detail was recorded for this event.')
      .count()
    expect(hasValues + hasNote).toBeGreaterThan(0)
    await expectNoHorizontalOverflow(page)

    // Clearing the filter restores the full newest-first listing.
    await page.getByLabel('Event kind').selectOption('')
    await expect(auditItems.first()).toBeVisible()
  })
})

test.describe('Anonymous Home (Guest) toggle', () => {
  test('enabling Guest access opens Home anonymously; disabling closes it', async (
    { page, browser },
    testInfo,
  ) => {
    // Mutates the shared Guest setting; only the desktop project runs it
    // and always restores the default (disabled) state afterwards.
    test.skip(testInfo.project.name !== 'desktop-1280', 'guest toggle runs once')

    // Default: anonymous visits are guided to login.
    const guest = await browser.newContext()
    const guestPage = await guest.newPage()
    await guestPage.goto('/')
    await expect(guestPage).toHaveURL(/\/login$/)
    await guest.close()

    await loginAs(page)
    await page.goto('/admin/site-access')
    await expect(page.getByRole('heading', { level: 1, name: 'Site Access' })).toBeVisible()

    try {
      page.on('dialog', (dialog) => void dialog.accept())
      await page.getByRole('button', { name: 'Make Home Public' }).click()
      await expect(page.getByText('Site Access Mode is now Public. Audit was recorded.')).toBeVisible()

      // A fresh anonymous context can read the Public projection.
      const guest2 = await browser.newContext()
      const guestPage2 = await guest2.newPage()
      await guestPage2.goto('/')
      await expect(guestPage2.getByRole('region', { name: 'Home' })).toBeVisible()
      await expect(guestPage2.getByText('Node A')).toBeVisible()
      // Site Access Mode gates Home as a whole; the legacy private value on
      // an Active Node must not create a second visibility layer.
      await expect(guestPage2.getByText('Node B (private)')).toBeVisible()
      await expect(guestPage2.getByText('Active Nodes', { exact: true })).toBeVisible({ timeout: 15_000 })
      await expectNoHorizontalOverflow(guestPage2)
      // Guests never see Admin or Sign out.
      await expect(guestPage2.getByRole('link', { name: 'Admin' })).toHaveCount(0)
      await expect(guestPage2.getByRole('button', { name: 'Sign out' })).toHaveCount(0)
      // Guests never enter the Admin shell, even with Guest Home enabled:
      // /admin answers the stable, non-leaking Owner-required panel.
      await guestPage2.goto('/admin')
      await expect(
        guestPage2.getByRole('heading', { level: 1, name: 'Owner access required' }),
      ).toBeVisible()
      await expect(guestPage2.getByRole('navigation', { name: 'Admin' })).toHaveCount(0)
      await expectNoHorizontalOverflow(guestPage2)
      await guestPage2.goto('/')
      await expect(guestPage2.getByText('Active Nodes', { exact: true })).toBeVisible({ timeout: 15_000 })

      // Disabling closes the open Guest stream: the anonymous tab lands on
      // the login page without flashing prior data.
      await page.getByRole('button', { name: 'Make Home Private' }).click()
      await expect(page.getByText('Site Access Mode is now Private. Audit was recorded.')).toBeVisible()
      await expect(guestPage2).toHaveURL(/\/login$/, { timeout: 10_000 })
      await expect(
        guestPage2.getByRole('heading', { level: 1, name: 'Sign in to PlatPulse' }),
      ).toBeVisible()
      await expect(guestPage2.getByText('Node A')).toHaveCount(0)
      await guest2.close()

      // The Owner session is unaffected by the Guest toggle.
      await page.goto('/admin/site-access')
      await expect(page.getByRole('heading', { level: 1, name: 'Site Access' })).toBeVisible()
    } finally {
      // Always restore the default so parallel suites keep their contract.
      const access = await page.request.get('/api/admin/v1/access-mode')
      if (access.ok()) {
        const settings = (await access.json()) as { mode: 'public' | 'private' }
        if (settings.mode === 'public') {
          const csrf = await page.evaluate(async () => {
            const response = await fetch('/api/public/v1/session')
            const body = (await response.json()) as { csrfToken: string }
            return body.csrfToken
          })
          await page.request.put('/api/admin/v1/access-mode', {
            headers: { 'X-CSRF-Token': csrf },
            data: { mode: 'private', confirmed: true },
          })
        }
      }
    }
  })
})

test.describe('Audit access follows the Owner policy', () => {
  test('a Viewer is refused the Audit API without leaking content', async ({ page }) => {
    await loginAs(page, E2E_VIEWER_USERNAME, E2E_VIEWER_PASSWORD)
    const response = await page.request.get('/api/admin/v1/audit')
    expect(response.status()).toBe(403)
    const body = (await response.json()) as { error: { code: string } }
    expect(body.error.code).toBe('owner_required')
    await expectNoHorizontalOverflow(page)
  })
})
