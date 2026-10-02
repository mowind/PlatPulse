import { expect, test, type Page } from '@playwright/test'
import { expectNoHorizontalOverflow } from './helpers'
import {
  HARNESS_OWNER_PASSWORD,
  HARNESS_OWNER_USERNAME,
  HARNESS_SECOND_OWNER_PASSWORD,
  HARNESS_SECOND_OWNER_USERNAME,
  startDisposableServer,
  type DisposableServer,
} from './server-harness'

/**
 * Rule and inheritance-override management acceptance (issue #204, part of
 * #202 Stage 1).
 *
 * A dedicated throwaway Server is booted per test: own port, own state
 * directory, own SQLite, seeded while the database is closed (the harness
 * seedSql option) with one typed Rule, one Node subject, one open Incident
 * opened under Rule version 1, and its durable acknowledgment. Every
 * assertion then travels production WebUI -> real Server HTTP -> SQLite. The
 * second Owner signs in as its own identity in a separate browser context and
 * writes through the same real HTTP routes with the real CSRF and Origin
 * guards, proving the stale-writer rejection is a Server decision, not a
 * client-only illusion.
 */

const RULE_KEY = 'node.rpc_unreachable'
const INCIDENT_ID = '0195f2a1-0400-4400-8400-000000000400'
const SUBJECT_KEY = 'node-e2e-rules'

function isoMinutesAgo(minutes: number): string {
  return new Date(Date.now() - minutes * 60_000).toISOString()
}

function seedRules(): string {
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
    "INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-e2e-rules', 0, '" +
      opened +
      "', '" +
      opened +
      "');",
    "INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('" +
      SUBJECT_KEY +
      "', 'agent-e2e-rules', 'platon-mainnet', 'E2E Rules Node', 'http://127.0.0.1:1', 'active', 'private', 1, '" +
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
    "INSERT INTO incident_acknowledgments (incident_id, acknowledged_by_user_id, acknowledged_by_username, acknowledged_at) VALUES ('" +
      INCIDENT_ID +
      "', NULL, '" +
      HARNESS_OWNER_USERNAME +
      "', '" +
      isoMinutesAgo(10) +
      "');",
  ].join('\n')
}

async function incidentTotal(server: DisposableServer): Promise<number> {
  const body = (await server.expectAdminGet('/api/admin/v1/alerts/incidents', 200)) as {
    total: number
  }
  return body.total
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

test.describe('Disposable Server Rule management acceptance', () => {
  test('reads, edits, previews, and conflict-checks a Rule without rewriting an Incident', async ({
    page,
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the disposable Server flow runs once')
    const server = await startDisposableServer({ seedSql: seedRules() })
    try {
      await loginToDisposableServer(page, server.baseUrl)

      // Story 17: the typed catalog lists the Rule and its composed version.
      await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/rules')
      await expect(page.getByRole('heading', { level: 1, name: 'Alert Rules' })).toBeVisible()
      await expect(page.getByRole('link', { name: RULE_KEY })).toBeVisible()

      // Story 17/18: the detail reads the baseline and states how inheritance
      // resolves for Node subjects.
      await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/rules/' + RULE_KEY)
      await expect(
        page.getByRole('heading', { level: 1, name: 'Alert Rule ' + RULE_KEY }),
      ).toBeVisible()
      await expect(page.getByText(/unset override fields inherit the layer below/).first()).toBeVisible()

      // Story 22: preview evaluates the draft and promises nothing. It writes
      // no Incident.
      const totalBeforePreview = await incidentTotal(server)
      await page.getByRole('button', { name: 'Preview unsaved changes' }).click()
      await expect(page.getByText(/It is not a save: it creates, resolves, and acknowledges nothing/)).toBeVisible()
      expect(await incidentTotal(server)).toBe(totalBeforePreview)

      // Story 18: a Network override narrows one field and is written through
      // the real HTTP route.
      await page.getByLabel('Override scope value').fill('platon-mainnet')
      await page.getByLabel('Override severity').selectOption('critical')
      await page.getByRole('button', { name: 'Save override' }).click()
      await expect(
        page.getByText(
          'Override saved as composed version 2. A reader holding the previous version must reload before saving.',
        ),
      ).toBeVisible()
      const overrides = page.getByRole('table', { name: 'Rule overrides' })
      await expect(overrides.getByText('platon-mainnet')).toBeVisible()
      await expect(overrides.getByText('Critical')).toBeVisible()
      // Unset override fields read as inherited, never as defaulted values.
      await expect(overrides.getByText('Inherited').first()).toBeVisible()

      // A second Owner — a different identity in its own browser context, not
      // the first Owner's CSRF token — saves first through the same real
      // mutation route with the real Origin and CSRF guards. The first Owner's
      // page still holds the composed version it reviewed (2), so the Server,
      // not the browser, decides the next save is stale.
      const secondOwner = await browser.newContext()
      let externalSaveStatus = 0
      try {
        const secondOwnerPage = await secondOwner.newPage()
        await loginToDisposableServer(
          secondOwnerPage,
          server.baseUrl,
          HARNESS_SECOND_OWNER_USERNAME,
          HARNESS_SECOND_OWNER_PASSWORD,
        )
        const sessionResponse = await secondOwner.request.get(
          server.baseUrl + '/api/public/v1/session',
        )
        expect(sessionResponse.status()).toBe(200)
        const sessionBody = (await sessionResponse.json()) as { csrfToken: string }
        const externalSave = await secondOwner.request.put(
          server.baseUrl + '/api/admin/v1/alerts/rules/' + RULE_KEY,
          {
            headers: { origin: server.baseUrl, 'x-csrf-token': sessionBody.csrfToken },
            data: {
              expectedVersion: 2,
              enabled: true,
              severity: 'warning',
              condition: { for_secs: 60, recovery_for_secs: 60 },
            },
          },
        )
        externalSaveStatus = externalSave.status()
      } finally {
        await secondOwner.close()
      }
      expect(externalSaveStatus).toBe(200)

      // Story 19: the stale draft is rejected by the Server; the UI refuses to
      // overwrite and demands a reload and re-review.
      await page.getByLabel('Rule severity').selectOption('critical')
      await page.getByRole('button', { name: 'Save Rule' }).click()
      await expect(page.getByText('This Rule changed since you read it')).toBeVisible()
      await expect(page.getByRole('button', { name: 'Save Rule' })).toBeDisabled()

      await page.getByRole('button', { name: /Reload current configuration/ }).click()
      await expect(page.getByText(/composed version 3/)).toBeVisible()
      await expect(page.getByRole('button', { name: 'Save Rule' })).toBeEnabled()

      // The reviewed save now succeeds and composes version 3.
      await page.getByLabel('Rule severity').selectOption('critical')
      await page.getByRole('button', { name: 'Save Rule' }).click()
      await expect(page.getByText('Saved as version 4.')).toBeVisible()

      // Story 20/21: the Incident keeps its opening rule version and evidence,
      // and its acknowledgment. Only the current effective configuration moved.
      const incident = (await server.expectAdminGet(
        '/api/admin/v1/alerts/incidents/' + INCIDENT_ID,
        200,
      )) as {
        state: string
        ruleVersion: number
        openedEvidence: unknown
        acknowledgment: { acknowledgedByUsername: string } | null
        currentRule: { version: number; enabled: boolean; severity: string } | null
      }
      expect(incident.state).toBe('open')
      expect(incident.ruleVersion).toBe(1)
      expect(incident.openedEvidence).toBeTruthy()
      expect(incident.acknowledgment?.acknowledgedByUsername).toBe(HARNESS_OWNER_USERNAME)
      expect(incident.currentRule).toMatchObject({ version: 4, enabled: true, severity: 'critical' })

      // A Rule edit opens no Incident and resolves none.
      expect(await incidentTotal(server)).toBe(totalBeforePreview)

      await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/incidents/' + INCIDENT_ID)
      await expect(page.getByText('Current Rule configuration')).toBeVisible()
      await expect(page.getByText('Current effective version')).toBeVisible()
      await expect(page.getByText(/Acknowledged by /)).toBeVisible()

      // The Admin surface stays behind the Owner gate: an anonymous context
      // never reads the Rules.
      const anonymous = await browser.newContext()
      try {
        const anonymousPage = await anonymous.newPage()
        await anonymousPage.goto(server.baseUrl + '/admin/alerts/rules')
        await expect(anonymousPage.getByRole('heading', { level: 1 })).toContainText(
          /Sign in|Owner access required/,
        )
      } finally {
        await anonymous.close()
      }
    } finally {
      await server.dispose()
    }
  })

  // The fixed 360×800, 390×844, 768×1024, 1280×800, and 1440×900 matrix is
  // covered by the five Playwright projects: each project is its own exact
  // geometry (and its own disposable Server), so no CSS viewport zoom or
  // setViewportSize emulation is used to fake a viewport (issue #204 review,
  // Spec FINDING 3). Light and dark are checked in every project; a touch
  // project additionally operates the preview control by tap.
  test('holds the Rules surfaces at the project geometry and theme', async ({ page }, testInfo) => {
    const isTouchProject = testInfo.project.name.includes('touch')
    const server = await startDisposableServer({ seedSql: seedRules() })
    try {
      await loginToDisposableServer(page, server.baseUrl)
      for (const colorScheme of ['light', 'dark'] as const) {
        await page.emulateMedia({ colorScheme })

        await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/rules')
        await expect(page.getByRole('heading', { level: 1, name: 'Alert Rules' })).toBeVisible()
        await expectNoHorizontalOverflow(page)

        await gotoAuthenticated(page, server.baseUrl, '/admin/alerts/rules/' + RULE_KEY)
        await expect(
          page.getByRole('heading', { level: 1, name: 'Alert Rule ' + RULE_KEY }),
        ).toBeVisible()
        await expectNoHorizontalOverflow(page)

        // Keyboard: the preview control is reachable and operable without a
        // pointer, and the statement it produces is visible at this geometry.
        const preview = page.getByRole('button', { name: 'Preview unsaved changes' })
        await preview.focus()
        await expect(preview).toBeFocused()
        await preview.press('Enter')
        await expect(
          page.getByText(/It is not a save: it creates, resolves, and acknowledges nothing/),
        ).toBeVisible()
        await expectNoHorizontalOverflow(page)

        if (isTouchProject) {
          // Touch: the same control is operable by tap at this geometry.
          await preview.tap()
          await expect(
            page.getByText(/It is not a save: it creates, resolves, and acknowledges nothing/),
          ).toBeVisible()
          await expectNoHorizontalOverflow(page)
        }
      }
    } finally {
      await server.dispose()
    }
  })
})
