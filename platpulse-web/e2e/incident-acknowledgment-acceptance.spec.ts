import { expect, test, type Page } from '@playwright/test'
import { expectNoHorizontalOverflow } from './helpers'
import {
  HARNESS_OWNER_PASSWORD,
  HARNESS_OWNER_USERNAME,
  HARNESS_SECOND_OWNER_PASSWORD,
  HARNESS_SECOND_OWNER_USERNAME,
  startDisposableServer,
} from './server-harness'

/**
 * Incident history and durable acknowledgment acceptance (issue #203, part of
 * #202 Stage 1).
 *
 * The fixed-viewport suite shares one long-lived Server whose SQLite carries
 * no Incidents. This suite boots its own throwaway Server (own port, own
 * state directory, own SQLite), seeds Alert catalog + Incident rows through
 * the harness' `seedSql` option while the database is still closed, and then
 * drives the production WebUI through the real Server HTTP routes. The seed
 * runs before boot on purpose: an external SQLite connection that closes
 * against a live WAL Server unlinks its `-wal`/`-shm` sidecars, so the Server
 * can lose un-checkpointed rows (Sessions) and fail closed with 401/503 at
 * random. Only the read-side seed touches storage; every assertion under test
 * travels WebUI -> HTTP -> SQLite.
 *
 * Process-heavy flows (seed, acknowledge, restart) run once on desktop-1280;
 * the viewport/theme/keyboard/touch matrix is exercised with one logged-in
 * context inside that second test so it does not multiply Server boots or
 * logins.
 */

const OPEN_INCIDENT_ID = '0195f2a1-0300-4300-8300-000000000300'
const RESOLVED_INCIDENT_ID = '0195f2a1-0300-4300-8300-000000000301'
const SUBJECT_KEY = 'node-e2e-acceptance'

const VIEWPORTS = [
  { width: 360, height: 800 },
  { width: 390, height: 844 },
  { width: 768, height: 1024 },
  { width: 1280, height: 800 },
  { width: 1440, height: 900 },
]

function isoMinutesAgo(minutes: number): string {
  return new Date(Date.now() - minutes * 60_000).toISOString()
}

function seedIncidents(): string {
  const opened = isoMinutesAgo(30)
  const older = isoMinutesAgo(90)
  // Long enough that the evidence block must scroll inside itself rather than
  // growing the page (issue #203 review N4).
  const longEvidence = 'seeded acceptance evidence '.repeat(300).trim()
  return [
    "INSERT OR IGNORE INTO alert_rules (rule_key, enabled, severity, version, condition_json, created_at, updated_at) VALUES ('node.rpc_unreachable', 1, 'warning', 1, '{}', '" + opened + "', '" + opened + "');",
    'INSERT INTO alert_incidents (incident_id, rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, resolved_at, opened_evidence_json, resolved_evidence_json) VALUES',
    "  ('" + OPEN_INCIDENT_ID + "', 'node.rpc_unreachable', 1, 'node', '" + SUBJECT_KEY + "', 'warning', 'open', 1, '" + opened + "', NULL, '{\"observedAt\":\"" + opened + "\",\"message\":\"" + longEvidence + "\"}', NULL),",
    "  ('" + RESOLVED_INCIDENT_ID + "', 'node.rpc_unreachable', 1, 'node', '" + SUBJECT_KEY + "', 'warning', 'resolved', 2, '" + older + "', '" + opened + "', '{\"observedAt\":\"" + older + "\",\"message\":\"seeded acceptance evidence\"}', '{\"observedAt\":\"" + opened + "\",\"message\":\"seeded recovery evidence\"}');",
  ].join('\n')
}

test.describe('Disposable Server Incident acceptance', () => {
  test('lists, inspects, and durably acknowledges a real Incident through the production WebUI', async ({ page, browser }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the disposable Server flow runs once')
    const server = await startDisposableServer({ seedSql: seedIncidents() })
    try {
      await loginToDisposableServer(page, server.baseUrl)
      // Acknowledgment must change no derived health, so snapshot the Rule
      // summary before any confirmation and compare it afterwards.
      const rulesBefore = (await server.expectAdminGet(
        '/api/admin/v1/alerts/rules',
        200,
      )) as { ruleKey: string; openIncidents: number }[]
      const openIncidentsBefore = rulesBefore.find(
        (rule) => rule.ruleKey === 'node.rpc_unreachable',
      )?.openIncidents
      expect(openIncidentsBefore).toBe(1)

      let streamConnected = false
      page.on('response', (response) => {
        if (response.url().includes('/api/admin/v1/events')) streamConnected = true
      })

      // Real Server projection populates the list.
      await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/incidents')
      await expect(page.getByText(/Incident history · 2 of 2/)).toBeVisible()
      await expect(page.getByText('Awaiting acknowledgment', { exact: true })).toHaveCount(2)

      // URL rule filter is honored end-to-end (regression: rule_key was ignored).
      await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/incidents?rule=node.rpc_unreachable')
      await expect(page.getByText(/Incident history · 2 of 2/)).toBeVisible()
      await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/incidents?rule=agent.offline')
      // A fresh Server route load can take a moment to settle; the empty state
      // is the point of this assertion, not the first paint.
      await expect(page.getByText('No Incidents match these filters.')).toBeVisible({
        timeout: 15_000,
      })

      // Contextual Node/Agent shortcut (issue #202 Story 2): the bound exact
      // subject_key filter narrows to this subject's occurrences, and the
      // Subject key chip can be cleared back to the broader history.
      await gotoAuthenticated(
        page,
        server.baseUrl,
        '/admin/alerts/incidents?subject=node&subject_key=' + encodeURIComponent(SUBJECT_KEY),
      )
      await expect(page.getByText(/Incident history · 2 of 2/)).toBeVisible()
      await expect(page.getByText('Subject key ' + SUBJECT_KEY)).toBeVisible()
      await page.getByRole('button', { name: 'Clear subject' }).click()
      await expect(page.getByText('Subject key ' + SUBJECT_KEY)).toHaveCount(0)
      await expect(page.getByText(/Incident history · 2 of 2/)).toBeVisible()
      await gotoAuthenticated(
        page,
        server.baseUrl,
        '/admin/alerts/incidents?subject=node&subject_key=node-absent',
      )
      await expect(page.getByText('No Incidents match these filters.')).toBeVisible({
        timeout: 15_000,
      })

      // Two-step acknowledgment of one occurrence from the expanded row.
      await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/incidents')
      await page.getByRole('button', { name: 'Expand Incident ' + OPEN_INCIDENT_ID }).click()
      await page.getByRole('button', { name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID }).click()
      await expect(page.getByRole('group', { name: 'Confirm Incident acknowledgment' })).toBeVisible()
      await page.getByRole('button', { name: 'Confirm acknowledgment' }).click()
      await expect(page.getByText('Acknowledgment recorded. This identity and time are authoritative.')).toBeVisible()
      await expect(page.getByText(/Acknowledged by admin/)).toHaveCount(1)

      // The incident detail page shows the same authoritative acknowledgment.
      await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/incidents/' + OPEN_INCIDENT_ID)
      await expect(page.getByText(/Acknowledged by admin/)).toBeVisible()
      const adminAckText = (await page.locator('[data-slot="incident-acknowledgment"]').innerText())
        .split('\n')[0]
        .trim()
      expect(adminAckText).toMatch(/^Acknowledged by admin at /)

      // Auth/health policy unchanged: acknowledgment never resolves or heals.
      const detail = (await server.expectAdminGet(
        '/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID,
        200,
      )) as {
        state: string
        acknowledgment: { acknowledgedByUsername: string; acknowledgedAt: string } | null
      }
      expect(detail.state).toBe('open')
      expect(detail.acknowledgment?.acknowledgedByUsername).toBe('admin')
      const acknowledgedAt = detail.acknowledgment?.acknowledgedAt
      expect(acknowledgedAt).toBeTruthy()

      const rulesAfter = (await server.expectAdminGet(
        '/api/admin/v1/alerts/rules',
        200,
      )) as { ruleKey: string; openIncidents: number }[]
      expect(
        rulesAfter.find((rule) => rule.ruleKey === 'node.rpc_unreachable')?.openIncidents,
      ).toBe(openIncidentsBefore)

      // Refresh and Server restart both keep the shared confirmation.
      await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/incidents')
      await reloadAuthenticated(page, server.baseUrl)
      await expect(page.getByText('Acknowledged', { exact: true })).toHaveCount(1)
      await server.restart()
      await loginToDisposableServer(page, server.baseUrl)
      await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/incidents')
      await expect(page.getByText('Acknowledged', { exact: true })).toHaveCount(1)
      const detailAfterRestart = (await server.expectAdminGet(
        '/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID,
        200,
      )) as { acknowledgment: { acknowledgedAt: string } | null }
      expect(detailAfterRestart.acknowledgment?.acknowledgedAt).toBe(acknowledgedAt)

      // First-write-wins at the real HTTP boundary; a repeat is a no-op.
      const repeat = await server.acknowledgeIncident(OPEN_INCIDENT_ID)
      expect(repeat.status).toBe(200)
      const body = repeat.body as {
        recorded: boolean
        acknowledgment: { acknowledgedByUsername: string; acknowledgedAt: string }
      }
      expect(body.recorded).toBe(false)
      expect(body.acknowledgment.acknowledgedByUsername).toBe('admin')

      // A second Owner is an independent identity: their confirmation is a
      // no-op and the first identity/time is what every Owner sees.
      const secondOwner = await server.acknowledgeIncidentAs(
        HARNESS_SECOND_OWNER_USERNAME,
        HARNESS_SECOND_OWNER_PASSWORD,
        OPEN_INCIDENT_ID,
      )
      expect(secondOwner.status).toBe(200)
      const secondOwnerBody = secondOwner.body as {
        recorded: boolean
        acknowledgment: { acknowledgedByUsername: string; acknowledgedAt: string }
      }
      expect(secondOwnerBody.recorded).toBe(false)
      expect(secondOwnerBody.acknowledgment.acknowledgedByUsername).toBe('admin')
      expect(secondOwnerBody.acknowledgment.acknowledgedAt).toBe(acknowledgedAt)

      // The second Owner's own browser session shows that shared confirmation.
      const secondContext = await browser.newContext()
      const secondPage = await secondContext.newPage()
      try {
        await loginToDisposableServer(
          secondPage,
          server.baseUrl,
          HARNESS_SECOND_OWNER_USERNAME,
          HARNESS_SECOND_OWNER_PASSWORD,
        )
        await gotoAuthenticated(secondPage, server.baseUrl, '/admin/alerts/incidents')
        await expect(secondPage.getByText('Acknowledged', { exact: true })).toHaveCount(1)
        await secondPage
          .getByRole('button', { name: 'Expand Incident ' + OPEN_INCIDENT_ID })
          .click()
        await expect(secondPage.getByText(/Acknowledged by admin/)).toBeVisible()
        // The second Owner sees the same first identity and the same displayed time.
        await expect(secondPage.locator('[data-slot="incident-acknowledgment"]')).toContainText(
          adminAckText,
        )
      } finally {
        await secondContext.close()
      }

      // SSE convergence: a write from outside this tab renders without a reload.
      // Wait for the live admin stream to reconnect after the restart first.
      streamConnected = false
      await reloadAuthenticated(page, server.baseUrl)
      await expect.poll(() => streamConnected, { timeout: 20_000 }).toBe(true)
      await expect(page.getByText('Acknowledged', { exact: true })).toHaveCount(1)
      const external = await server.acknowledgeIncident(RESOLVED_INCIDENT_ID)
      expect((external.body as { recorded: boolean }).recorded).toBe(true)
      await expect(page.getByText('Acknowledged', { exact: true })).toHaveCount(2, { timeout: 20_000 })
    } finally {
      await server.dispose()
    }
  })

  test('Incident history and acknowledgment hold across the fixed viewport and theme matrix', async ({ browser }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the matrix runs once against one disposable Server')
    const server = await startDisposableServer({ seedSql: seedIncidents() })
    try {
      // One logged-in context (hasTouch keeps the small-viewport tap path
      // available); the viewport and colour scheme change per iteration so the
      // matrix does not multiply logins.
      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        for (const viewport of VIEWPORTS) {
          for (const colorScheme of ['light', 'dark'] as const) {
            await page.setViewportSize(viewport)
            await page.emulateMedia({ colorScheme })
            await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/incidents')
            await expect(page.getByText(/Incident history · 2 of 2/)).toBeVisible()
            await expectNoHorizontalOverflow(page)

            const disclosure = page.getByRole('button', {
              name: 'Expand Incident ' + OPEN_INCIDENT_ID,
            })
            await disclosure.focus()
            if (viewport.width < 768) {
              await disclosure.tap()
            } else {
              await disclosure.press('Enter')
            }
            const entry = page.getByRole('button', {
              name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID,
            })
            await expect(entry).toBeVisible()

            // Keyboard: entering the confirmation moves focus into it; Cancel restores the entry.
            await entry.focus()
            await entry.press('Enter')
            const confirm = page.getByRole('button', { name: 'Confirm acknowledgment' })
            await expect(confirm).toBeFocused()
            await page
              .getByRole('group', { name: 'Confirm Incident acknowledgment' })
              .getByRole('button', { name: 'Cancel' })
              .click()
            await expect(entry).toBeFocused()

            // Collapsing restores focus to the row disclosure.
            const collapse = page.getByRole('button', { name: /Collapse evidence/ })
            await collapse.focus()
            await collapse.press('Enter')
            await expect(disclosure).toBeFocused()

            // The detail route holds across the same viewport/theme matrix,
            // with the same interaction geometry and no horizontal overflow.
            await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/incidents/' + OPEN_INCIDENT_ID)
            const detailEntry = page.getByRole('button', {
              name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID,
            })
            await expect(detailEntry).toBeVisible()
            const detailBox = await detailEntry.boundingBox()
            expect(detailBox?.height ?? 0).toBeGreaterThanOrEqual(44)
            expect(detailBox?.width ?? 0).toBeGreaterThanOrEqual(44)
            await expect(
              page.getByRole('heading', { level: 3, name: 'Incident acknowledgment' }),
            ).toBeVisible()
            await detailEntry.focus()
            await detailEntry.press('Enter')
            await expect(page.getByRole('button', { name: 'Confirm acknowledgment' })).toBeFocused()
            await page
              .getByRole('group', { name: 'Confirm Incident acknowledgment' })
              .getByRole('button', { name: 'Cancel' })
              .click()
            await expect(detailEntry).toBeFocused()
            await expectNoHorizontalOverflow(page)

            // Long evidence scrolls inside its own block, never the page.
            const evidence = page.locator('[data-slot="incident-evidence"]').first()
            await expect(evidence).toBeVisible()
            expect(
              await evidence.evaluate((element) => element.scrollHeight > element.clientHeight),
            ).toBe(true)
          }
        }

        // Touch: the confirmation control is operable by tap, not only keyboard.
        await page.setViewportSize({ width: 360, height: 800 })
        await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/incidents/' + RESOLVED_INCIDENT_ID)
        const touchEntry = page.getByRole('button', {
          name: 'Acknowledge Incident ' + RESOLVED_INCIDENT_ID,
        })
        await touchEntry.tap()
        const touchConfirm = page.getByRole('button', { name: 'Confirm acknowledgment' })
        await expect(touchConfirm).toBeVisible()
        await touchConfirm.tap()
        await expect(
          page.getByText('Acknowledgment recorded. This identity and time are authoritative.'),
        ).toBeVisible()
        await expect(page.getByText(/Acknowledged by admin/)).toBeVisible()
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('a long unbroken Owner identity wraps without horizontal overflow (issue #203 review B1)', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'one real-browser geometry check')
    // A legal 64-character Owner username with no internal break opportunity.
    const longUsername = 'owner-' + 'a'.repeat(58)
    const server = await startDisposableServer({
      seedSql:
        seedIncidents() +
        '\n' +
        "INSERT INTO incident_acknowledgments (incident_id, acknowledged_by_user_id, acknowledged_by_username, acknowledged_at) VALUES ('" +
        RESOLVED_INCIDENT_ID +
        "', NULL, '" +
        longUsername +
        "', '" +
        isoMinutesAgo(1) +
        "');",
    })
    try {
      await loginToDisposableServer(page, server.baseUrl)

      for (const width of [360, 390]) {
        await page.setViewportSize({ width, height: 800 })

        // List identity cell.
        await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/incidents')
        const listIdentity = page.locator('small', { hasText: longUsername }).first()
        await expect(listIdentity).toBeVisible()
        await expectNoHorizontalOverflow(page)
        expect(await listIdentity.evaluate((el) => getComputedStyle(el).overflowWrap)).toBe(
          'anywhere',
        )
        const listBox = await listIdentity.boundingBox()
        expect(listBox?.x ?? -1).toBeGreaterThanOrEqual(0)
        expect((listBox?.x ?? 0) + (listBox?.width ?? 0)).toBeLessThanOrEqual(width + 1)

        // Detail identity paragraph.
        await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/incidents/' + RESOLVED_INCIDENT_ID)
        const detailIdentity = page
          .locator('[data-slot="incident-acknowledgment"] p', { hasText: longUsername })
          .first()
        await expect(detailIdentity).toBeVisible()
        await expectNoHorizontalOverflow(page)
        expect(await detailIdentity.evaluate((el) => getComputedStyle(el).overflowWrap)).toBe(
          'anywhere',
        )
        const detailBox = await detailIdentity.boundingBox()
        expect(detailBox?.x ?? -1).toBeGreaterThanOrEqual(0)
        expect((detailBox?.x ?? 0) + (detailBox?.width ?? 0)).toBeLessThanOrEqual(width + 1)
      }
    } finally {
      await server.dispose()
    }
  })
})

/**
 * Navigate to a protected Admin route. The disposable Server can drop an idle
 * keep-alive connection, and the app fails the session probe closed (showing
 * the sign-in form) on any probe error; sign in again and retry so the
 * acceptance flow tests product behavior rather than that transport race.
 */
async function gotoAuthenticated(page: Page, baseUrl: string, path: string): Promise<void> {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await page.goto(baseUrl + path)
    const heading = page.locator('h1').first()
    await heading.waitFor({ state: 'visible', timeout: 15_000 })
    if (!/^Sign in\b/i.test((await heading.innerText()).trim())) return
    await loginToDisposableServer(page, baseUrl)
  }
  await page.goto(baseUrl + path)
}

/** A fresh load of the current protected route, re-authenticating if needed. */
async function reloadAuthenticated(page: Page, baseUrl: string): Promise<void> {
  const target = page.url()
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await page.goto(target)
    const heading = page.locator('h1').first()
    await heading.waitFor({ state: 'visible', timeout: 15_000 })
    if (!/^Sign in\b/i.test((await heading.innerText()).trim())) return
    await loginToDisposableServer(page, baseUrl)
  }
}

async function loginToDisposableServer(
  page: Page,
  baseUrl: string,
  username: string = HARNESS_OWNER_USERNAME,
  password: string = HARNESS_OWNER_PASSWORD,
) {
  await page.context().clearCookies()
  await page.goto(baseUrl + '/login')
  await page.getByLabel('Username').fill(username)
  await page.getByLabel('Password').fill(password)
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
}
