import { expect, test, type Page } from '@playwright/test'
import type {
  IncidentDetail,
  MaintenanceListResponse,
  SilenceListResponse,
  SessionResponse,
} from '../src/api/generated'
import { expectNoHorizontalOverflow, expectVisibleInteractiveTargets } from './helpers'
import {
  HARNESS_OWNER_PASSWORD,
  HARNESS_OWNER_USERNAME,
  startDisposableServer,
  type DisposableServer,
} from './server-harness'

/**
 * Silence and Maintenance Window management acceptance (issue #205, part of
 * #202 Stage 1). Each test owns one throwaway Server, port, and SQLite database.
 * Only the typed Rule, Agent, Node, and opening Incident are seeded, while the
 * database is closed; policies are created through the production WebUI and
 * asserted through real Server HTTP, never a mock or a test-only seam.
 *
 * Process-heavy flows run once on desktop-1280. The second test crosses the
 * five fixed viewports with both themes in one logged-in touch-capable context
 * so the matrix does not multiply Server boots or logins.
 */

const RULE_KEY = 'node.rpc_unreachable'
const INCIDENT_ID = '0195f2a1-0500-4500-8500-000000000500'
const SUBJECT_KEY = 'node-e2e-silence'

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
  const opened = isoMinutesAgo(20)
  const condition = '{"for_secs":60,"recovery_for_secs":60}'
  const openingEvidence = JSON.stringify({ observedAt: opened, message: 'seeded opening evidence' })
  return [
    "INSERT OR IGNORE INTO alert_rules (rule_key, enabled, severity, version, condition_json, created_at, updated_at) VALUES ('" +
      RULE_KEY +
      "', 1, 'warning', 1, '" +
      condition +
      "', '" +
      opened +
      "', '" +
      opened +
      "');",
    "INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-e2e-silence', 0, '" +
      opened +
      "', '" +
      opened +
      "');",
    "INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('" +
      SUBJECT_KEY +
      "', 'agent-e2e-silence', 'platon-mainnet', 'E2E Silence Node', 'http://127.0.0.1:1', 'active', 'private', 1, '" +
      opened +
      "', '" +
      opened +
      "');",
    'INSERT INTO alert_incidents (incident_id, rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, resolved_at, opened_evidence_json, resolved_evidence_json) VALUES',
    "  ('" +
      INCIDENT_ID +
      "', '" +
      RULE_KEY +
      "', 1, 'node', '" +
      SUBJECT_KEY +
      "', 'warning', 'open', 1, '" +
      opened +
      "', NULL, '" +
      openingEvidence +
      "', NULL);",
  ].join('\n')
}

async function loginToDisposableServer(page: Page, baseUrl: string) {
  await page.context().clearCookies()
  await page.goto(baseUrl + '/login')
  await page.getByLabel('Username').fill(HARNESS_OWNER_USERNAME)
  await page.getByLabel('Password').fill(HARNESS_OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
}

/** Retry the established fail-closed idle-connection session-probe race. */
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

/** datetime-local values are browser-local, including on non-UTC hosts. */
async function fillActiveWindow(page: Page): Promise<void> {
  const values = await page.evaluate(() => {
    const localValue = (minutes: number): string => {
      const date = new Date(Date.now() + minutes * 60_000)
      const local = new Date(date.getTime() - date.getTimezoneOffset() * 60_000)
      return local.toISOString().slice(0, 16)
    }
    return { start: localValue(-5), end: localValue(120) }
  })
  await page.getByLabel('Starts at', { exact: true }).fill(values.start)
  await page.getByLabel('Ends at', { exact: true }).fill(values.end)
}

async function createSilence(page: Page, baseUrl: string, reason: string): Promise<void> {
  await gotoAuthenticated(page, baseUrl, '/admin/alerts/silences')
  await expect(page.getByRole('heading', { level: 1, name: 'Silences', exact: true })).toBeVisible()
  await page.getByLabel('Matcher kind', { exact: true }).selectOption({ label: 'Node' })
  await page.getByLabel('Matcher value', { exact: true }).fill(SUBJECT_KEY)
  await page.getByLabel('Reason', { exact: true }).fill(reason)
  await fillActiveWindow(page)
  await page.getByRole('button', { name: 'Create Silence', exact: true }).click()
  await expect(
    page.getByRole('status').filter({ hasText: 'Silence created. The Server is authoritative for it.' }),
  ).toBeVisible()
  await expect(page.getByRole('table', { name: 'Silences', exact: true }).getByText(reason, { exact: true })).toBeVisible()
}

async function createMaintenance(page: Page, baseUrl: string, reason: string): Promise<void> {
  await gotoAuthenticated(page, baseUrl, '/admin/alerts/maintenance')
  await expect(page.getByRole('heading', { level: 1, name: 'Maintenance', exact: true })).toBeVisible()
  await page.getByLabel('Scope kind', { exact: true }).selectOption({ label: 'Node' })
  await page.getByLabel('Scope value', { exact: true }).fill(SUBJECT_KEY)
  await page.getByLabel('Expected rule keys', { exact: true }).fill(RULE_KEY)
  await page.getByLabel('Reason', { exact: true }).fill(reason)
  await fillActiveWindow(page)
  await page.getByRole('button', { name: 'Create Maintenance Window', exact: true }).click()
  await expect(
    page.getByRole('status').filter({ hasText: 'Maintenance Window created. The Server is authoritative for it.' }),
  ).toBeVisible()
  await expect(page.getByRole('table', { name: 'Maintenance Windows' }).getByText(reason, { exact: true })).toBeVisible()
}

async function readSilences(server: DisposableServer): Promise<SilenceListResponse> {
  return (await server.expectAdminGet('/api/admin/v1/alerts/silences', 200)) as SilenceListResponse
}

async function readMaintenance(server: DisposableServer): Promise<MaintenanceListResponse> {
  return (await server.expectAdminGet('/api/admin/v1/alerts/maintenance', 200)) as MaintenanceListResponse
}

async function readIncident(server: DisposableServer): Promise<IncidentDetail> {
  return (await server.expectAdminGet(
    '/api/admin/v1/alerts/incidents/' + INCIDENT_ID,
    200,
  )) as IncidentDetail
}

function displayTimestamp(timestamp: string): string {
  return timestamp.slice(0, 19).replace('T', ' ') + ' UTC'
}

test.describe('Disposable Server Silence and Maintenance acceptance', () => {
  test('creates, durably lists, inspects, and cancels policies through the production WebUI behind the Owner boundary', async ({ page, browser }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the disposable Server flow runs once')
    const server = await startDisposableServer({ seedSql: seedIncidents() })
    try {
      await loginToDisposableServer(page, server.baseUrl)
      const sessionResponse = await page.request.get(server.baseUrl + '/api/public/v1/session')
      expect(sessionResponse.status()).toBe(200)
      const session = (await sessionResponse.json()) as SessionResponse
      const ownerId = session.session.userId
      const silenceReason = 'Investigating the Node RPC connection'
      await createSilence(page, server.baseUrl, silenceReason)
      const silences = (await readSilences(server)).silences
      expect(silences).toHaveLength(1)
      const silence = silences[0]
      expect(silence).toMatchObject({
        status: 'active', matcherKind: 'node', matcherValue: SUBJECT_KEY,
        reason: silenceReason, createdBy: ownerId,
      })
      expect(Date.parse(silence.startsAt)).toBeLessThan(Date.now())
      expect(Date.parse(silence.endsAt)).toBeGreaterThan(Date.now())

      // A Silence suppresses delivery only: it never resolves or marks the Incident.
      const incident = await readIncident(server)
      expect(incident.state).toBe('open')
      expect(incident.ruleVersion).toBe(1)
      expect(incident.suppressions).toEqual([{
        id: silence.silenceId, kind: 'silence', marksIncident: false,
        reason: silenceReason, startsAt: silence.startsAt, endsAt: silence.endsAt,
      }])
      await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/incidents/' + INCIDENT_ID)
      await expect(page.getByText('Suppressions', { exact: true })).toBeVisible()
      const suppression = page.getByRole('listitem').filter({ hasText: silenceReason })
      await expect(suppression.getByText('Silence', { exact: true })).toBeVisible()
      await expect(suppression).toContainText('Does not mark the Incident suppressed')
      await expect(suppression).toContainText(
        displayTimestamp(silence.startsAt) + ' → ' + displayTimestamp(silence.endsAt),
      )

      // A fresh anonymous context must hit the real route-level auth gate, not
      // a directly mounted handler with a mocked auth provider.
      const anonymous = await browser.newContext()
      try {
        const anonymousPage = await anonymous.newPage()
        await anonymousPage.goto(server.baseUrl + '/admin/alerts/silences')
        // Never-authenticated Guests retain the attempted route and see the
        // non-leaking Owner-required Sign in surface (RequireOwner), unlike
        // an expired Session, which redirects to /login.
        await expect(anonymousPage.getByRole('heading', { level: 1, name: 'Owner access required' })).toBeVisible()
        await expect(anonymousPage.getByText(
          'This session cannot view Admin data. Sign in with an Owner account to continue.',
          { exact: true },
        )).toBeVisible()
        await expect(anonymousPage.getByRole('table', { name: 'Silences', exact: true })).toHaveCount(0)
        await expect(anonymousPage.getByText(silenceReason, { exact: true })).toHaveCount(0)
        await expect(anonymousPage.getByText(SUBJECT_KEY, { exact: true })).toHaveCount(0)
      } finally {
        await anonymous.close()
      }

      // Restart the process on its original SQLite, then authenticate afresh.
      await server.restart()
      await loginToDisposableServer(page, server.baseUrl)
      await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/silences')
      const silenceRow = page.getByRole('table', { name: 'Silences', exact: true })
        .getByRole('row').filter({ hasText: silenceReason })
      await expect(silenceRow.getByText('Active', { exact: true })).toBeVisible()
      expect((await readSilences(server)).silences).toEqual([silence])

      // Two steps: entering confirmation alone writes nothing.
      await silenceRow.getByRole('button', { name: /^Cancel Silence / }).click()
      const silenceConfirm = silenceRow.getByRole('group', { name: /^Confirm cancellation of Silence / })
      await expect(silenceConfirm).toBeVisible()
      expect((await readSilences(server)).silences[0].status).toBe('active')
      await silenceConfirm.getByRole('button', { name: 'Confirm cancellation', exact: true }).click()
      await expect(page.getByText('Silence cancelled. Suppressed messages are not replayed.', { exact: true })).toBeVisible()
      const cancelledSilence = (await readSilences(server)).silences[0]
      expect(cancelledSilence).toMatchObject({ silenceId: silence.silenceId, status: 'cancelled' })
      expect(cancelledSilence.cancelledAt).toBeTruthy()
      expect(Number.isFinite(Date.parse(cancelledSilence.cancelledAt ?? ''))).toBe(true)
      await expect(silenceRow.getByText('Cancelled', { exact: true })).toBeVisible()
      await expect(silenceRow.getByRole('button', { name: /^Cancel Silence / })).toHaveCount(0)
      expect((await readIncident(server)).suppressions).toEqual([])

      const maintenanceReason = 'Planned Node RPC maintenance'
      await createMaintenance(page, server.baseUrl, maintenanceReason)
      const windows = (await readMaintenance(server)).windows
      expect(windows).toHaveLength(1)
      const window = windows[0]
      expect(window).toMatchObject({
        status: 'active', scopeKind: 'node', scopeValue: SUBJECT_KEY,
        expectedRuleKeys: [RULE_KEY], reason: maintenanceReason, createdBy: ownerId,
      })
      expect(Date.parse(window.startsAt)).toBeLessThan(Date.now())
      expect(Date.parse(window.endsAt)).toBeGreaterThan(Date.now())
      const maintainedIncident = await readIncident(server)
      expect(maintainedIncident.state).toBe('open')
      expect(maintainedIncident.suppressions).toEqual([{
        id: window.windowId, kind: 'maintenance', marksIncident: true,
        reason: maintenanceReason, startsAt: window.startsAt, endsAt: window.endsAt,
      }])
      await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/incidents/' + INCIDENT_ID)
      const maintenanceSuppression = page.getByRole('listitem').filter({ hasText: maintenanceReason })
      await expect(maintenanceSuppression.getByText('Maintenance', { exact: true })).toBeVisible()
      await expect(maintenanceSuppression).toContainText('Marks the Incident suppressed')
      await expect(maintenanceSuppression).toContainText(
        displayTimestamp(window.startsAt) + ' → ' + displayTimestamp(window.endsAt),
      )
      await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/maintenance')
      const maintenanceRow = page.getByRole('table', { name: 'Maintenance Windows' })
        .getByRole('row').filter({ hasText: maintenanceReason })
      await maintenanceRow.getByRole('button', { name: /^Cancel Maintenance Window / }).click()
      const maintenanceConfirm = maintenanceRow.getByRole('group', { name: /^Confirm cancellation of Maintenance Window / })
      await expect(maintenanceConfirm).toBeVisible()
      expect((await readMaintenance(server)).windows[0].status).toBe('active')
      await maintenanceConfirm.getByRole('button', { name: 'Confirm cancellation', exact: true }).click()
      await expect(page.getByText('Maintenance Window cancelled. Suppressed messages are not replayed.', { exact: true })).toBeVisible()
      const cancelledWindow = (await readMaintenance(server)).windows[0]
      expect(cancelledWindow).toMatchObject({ windowId: window.windowId, status: 'cancelled' })
      expect(cancelledWindow.cancelledAt).toBeTruthy()
      expect(Number.isFinite(Date.parse(cancelledWindow.cancelledAt ?? ''))).toBe(true)
      await expect(maintenanceRow.getByText('Cancelled', { exact: true })).toBeVisible()
      await expect(maintenanceRow.getByRole('button', { name: /^Cancel Maintenance Window / })).toHaveCount(0)
      expect((await readIncident(server)).suppressions).toEqual([])

      // Include an active control row: a broken status filter must not pass just
      // because the database happens to contain only a cancelled Silence.
      const activeReason = 'Active control for the cancelled status filter'
      await createSilence(page, server.baseUrl, activeReason)
      expect((await readSilences(server)).silences).toHaveLength(2)
      await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/silences?status=cancelled')
      await expect(page.getByRole('group', { name: 'Silence filters' }).getByLabel('Status')).toHaveValue('cancelled')
      const filteredTable = page.getByRole('table', { name: 'Silences', exact: true })
      await expect(filteredTable.locator('tbody tr')).toHaveCount(1)
      await expect(filteredTable.getByText(silenceReason, { exact: true })).toBeVisible()
      await expect(filteredTable.locator('tbody').getByText('Cancelled', { exact: true })).toBeVisible()
      await expect(filteredTable.getByText(activeReason, { exact: true })).toHaveCount(0)
      const filtered = (await server.expectAdminGet(
        '/api/admin/v1/alerts/silences?status=cancelled', 200,
      )) as SilenceListResponse
      expect(filtered.silences).toEqual([cancelledSilence])
    } finally {
      await server.dispose()
    }
  })

  test('Silences and Maintenance hold across the fixed viewport and theme matrix with keyboard, touch, and long reasons', async ({ browser }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the matrix runs once against one disposable Server')
    const server = await startDisposableServer({ seedSql: seedIncidents() })
    try {
      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        const longReason = 'Planned Node maintenance keeps Incident evidence and suppresses delivery only. '.repeat(7).slice(0, 500)
        expect(longReason).toHaveLength(500)
        await createSilence(page, server.baseUrl, longReason)
        await createMaintenance(page, server.baseUrl, longReason)
        expect((await readSilences(server)).silences[0]).toMatchObject({ status: 'active', reason: longReason })
        expect((await readMaintenance(server)).windows[0]).toMatchObject({ status: 'active', reason: longReason })

        for (const viewport of VIEWPORTS) {
          for (const colorScheme of ['light', 'dark'] as const) {
            await page.setViewportSize(viewport)
            await page.emulateMedia({ colorScheme })
            for (const surface of [
              { path: '/admin/alerts/silences', heading: 'Silences', table: 'Silences', cancel: /^Cancel Silence /, confirm: /^Confirm cancellation of Silence /, keep: 'Keep Silence' },
              { path: '/admin/alerts/maintenance', heading: 'Maintenance', table: 'Maintenance Windows', cancel: /^Cancel Maintenance Window /, confirm: /^Confirm cancellation of Maintenance Window /, keep: 'Keep Window' },
            ]) {
              await gotoAuthenticated(page, server.baseUrl, surface.path)
              await expect(page.getByRole('heading', { level: 1, name: surface.heading, exact: true })).toBeVisible()
              const row = page.getByRole('table', { name: surface.table, exact: true })
                .getByRole('row').filter({ hasText: longReason })
              await expect(row.getByText('Active', { exact: true })).toBeVisible()
              const reason = row.getByText(longReason, { exact: true })
              await expect(reason).toBeVisible()
              await expectNoHorizontalOverflow(page)
              if (viewport.width <= 768) {
                await expectVisibleInteractiveTargets(page)
                // Measure actual text lines, not only a CSS wrapping declaration.
                expect(await reason.evaluate((element) => {
                  const range = document.createRange()
                  range.selectNodeContents(element)
                  return new Set(Array.from(range.getClientRects(), (rect) => Math.round(rect.top))).size
                })).toBeGreaterThan(1)
              }

              const cancel = row.getByRole('button', { name: surface.cancel })
              await cancel.focus()
              await expect(cancel).toBeFocused()
              await cancel.press('Enter')
              const confirm = row.getByRole('group', { name: surface.confirm })
              await expect(confirm).toBeVisible()
              await expectNoHorizontalOverflow(page)
              if (viewport.width <= 768) await expectVisibleInteractiveTargets(page)
              await confirm.getByRole('button', { name: surface.keep, exact: true }).press('Enter')
              await expect(confirm).toHaveCount(0)
              await expect(cancel).toBeVisible()

              if (viewport.width === 360) {
                await cancel.tap()
                await expect(confirm).toBeVisible()
                await confirm.getByRole('button', { name: surface.keep, exact: true }).tap()
                await expect(confirm).toHaveCount(0)
                await expect(cancel).toBeVisible()
                await expectNoHorizontalOverflow(page)
              }
            }
          }
        }
        // Dismissing by keyboard or touch must not have cancelled either policy.
        expect((await readSilences(server)).silences[0]).toMatchObject({ status: 'active', cancelledAt: null })
        expect((await readMaintenance(server)).windows[0]).toMatchObject({ status: 'active', cancelledAt: null })
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })
})
