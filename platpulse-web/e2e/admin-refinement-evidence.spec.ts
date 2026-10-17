import { test, expect, type Locator, type Page } from '@playwright/test'
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { expectNoHorizontalOverflow, loginAs } from './helpers'
import { findRepoRoot } from './repo-root'

// Opt-in evidence for the Admin follow-up round, not screenshot baselines.
// Run once per revision with ADMIN_REFINEMENT_EVIDENCE=before|after against the
// local seeded e2e Server; no production service is touched and no credential
// is captured. Every state below is asserted before its capture: a missing
// fixture, button, or detail row fails the run instead of producing a PNG of
// the wrong state. The interaction and overflow assertions also run daily in
// admin-followup.spec.ts, agent-lifecycle.spec.ts, and admin-list-matrix.spec.ts.
const phase = process.env.ADMIN_REFINEMENT_EVIDENCE
test.skip(!phase, 'Set ADMIN_REFINEMENT_EVIDENCE=before or after for matched captures')

const ROOT = join(findRepoRoot(), 'docs/visual-migration/emerald/admin-followup')
const SEEDED_AGENT = '0195f2a1-0011-4011-8011-000000000011'
const SEEDED_GENESIS = '0x' + '1'.repeat(64)
const PAGES = [
  { name: 'agents', path: '/admin/agents', heading: 'Agents' },
  { name: 'networks', path: '/admin/networks', heading: 'Networks' },
  { name: 'validators', path: '/admin/validators', heading: 'Validators' },
  { name: 'sessions', path: '/admin/access/sessions', heading: 'Sessions' },
]
const WIDTHS = [360, 390, 768, 1280]
const THEMES = ['light', 'dark'] as const

// The theme comes only from the stored mode the init script sets; the last
// registered init script wins.
async function applyTheme(page: Page, theme: string) {
  await page.addInitScript((value) => localStorage.setItem('platpulse.themeMode', value), theme)
}

async function open(page: Page, path: string, heading: string) {
  await page.goto(path)
  await expect(page.getByRole('heading', { level: 1, name: heading })).toBeVisible({ timeout: 20_000 })
  await page.waitForLoadState('networkidle').catch(() => undefined)
}

async function capture(page: Page, out: string, name: string) {
  await page.screenshot({ path: out + '/' + name + '.png', fullPage: true, animations: 'disabled' })
}

/** Click the named disclosure inside one row and prove it opened the region
 *  it controls. */
async function expand(page: Page, row: Locator, showName: string | RegExp, hideName: string | RegExp) {
  const toggle = row.getByRole('button', { name: showName, exact: true })
  await expect(toggle).toHaveCount(1)
  await expect(toggle).toBeVisible()
  await expect(toggle).toHaveAttribute('aria-expanded', 'false')
  await toggle.click()
  const expanded = row.getByRole('button', { name: hideName, exact: true })
  await expect(expanded).toHaveAttribute('aria-expanded', 'true')
  const controls = await expanded.getAttribute('aria-controls')
  expect(controls, 'an expanded disclosure names the region it controls').toBeTruthy()
  const region = page.locator('[id="' + controls + '"]')
  await expect(region).toBeVisible()
  return region
}

const ERROR_BODY = '{"error":"unavailable","message":"Evidence: simulated refresh failure"}'

test('matched Admin follow-up evidence', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop-1280', 'one project drives every width')
  test.setTimeout(600_000)
  await loginAs(page)
  const out = ROOT + '/' + phase
  await mkdir(out, { recursive: true })
  const geometry: Record<string, unknown> = {}

  for (const theme of THEMES) {
    await applyTheme(page, theme)
    for (const width of WIDTHS) {
      await page.setViewportSize({ width, height: width < 768 ? 844 : 900 })
      for (const entry of PAGES) {
        await open(page, entry.path, entry.heading)
        geometry[entry.name + '-' + width + '-' + theme] = await page.evaluate(() => ({
          doc: document.documentElement.scrollWidth,
          viewport: document.documentElement.clientWidth,
        }))
        await capture(page, out, entry.name + '-' + width + '-' + theme)
      }

      // Agents: the seeded Agent's real diagnostics disclosure, not the first
      // aria-expanded button (that one is the full Agent ID toggle).
      await open(page, '/admin/agents', 'Agents')
      const agentRow = page
        .locator('[data-slot="agent-table"] tbody tr:not([data-slot="detail-row"])')
        .filter({ has: page.locator('a[href="/admin/agents/' + SEEDED_AGENT + '"]') })
      await expect(agentRow).toHaveCount(1)
      const diagnostics = await expand(page, agentRow, 'Show diagnostics', 'Hide diagnostics')
      await expect(diagnostics.getByRole('heading', { name: 'Recorded diagnostic evidence' })).toBeVisible()
      await expect(diagnostics.getByRole('list', { name: 'Recorded diagnostic evidence' })).toBeVisible()
      await expectNoHorizontalOverflow(page)
      await capture(page, out, 'agents-diagnostics-' + width + '-' + theme)

      // Agents: the full Agent ID disclosure, captured separately.
      await open(page, '/admin/agents', 'Agents')
      await expand(page, agentRow, 'Show full Agent ID', 'Hide full Agent ID')
      await expect(agentRow.locator('code[data-slot="agent-full-id"]')).toHaveText(SEEDED_AGENT)
      await expect(agentRow.getByRole('button', { name: 'Copy Agent ID' })).toBeVisible()
      await expectNoHorizontalOverflow(page)
      await capture(page, out, 'agents-full-id-' + width + '-' + theme)

      if (phase === 'after') {
        // The Genesis hash disclosure does not exist on the baseline revision.
        await open(page, '/admin/networks', 'Networks')
        const networkRow = page.locator('[data-slot="network-table"] tbody tr').filter({ hasText: 'platon-e2e' })
        await expect(networkRow).toHaveCount(1)
        const genesis = await expand(page, networkRow, 'Show full Genesis hash', 'Hide full Genesis hash')
        await expect(genesis.getByText(SEEDED_GENESIS, { exact: true })).toBeVisible()
        await expect(genesis.getByRole('button', { name: 'Copy Genesis hash' })).toBeVisible()
        await expectNoHorizontalOverflow(page)
        await capture(page, out, 'networks-genesis-' + width + '-' + theme)
      }
    }
  }

  // Remaining states in the light theme, phone and desktop.
  await applyTheme(page, 'light')
  for (const width of [390, 1280]) {
    await page.setViewportSize({ width, height: width < 768 ? 844 : 900 })

    await open(page, '/admin/nodes', 'Nodes')
    // The seeded healthy Node A row, by its stable detail link.
    const nodeRow = page
      .locator('[data-slot="node-table"] tbody tr:not([data-slot="detail-row"])')
      .filter({ has: page.locator('a[href="/admin/nodes/0195f2a1-0014-4014-8014-000000000014"]') })
    await expect(nodeRow).toHaveCount(1)
    const nodeDetail = await expand(page, nodeRow, 'Expand Node A', 'Collapse Node A')
    await expect(nodeDetail.getByRole('heading', { name: 'Node details' })).toBeVisible()
    await capture(page, out, 'nodes-expanded-' + width + '-light')

    // Signing in records a session_created Audit event with redacted details;
    // the newest one (listed first) is this run's own sign-in.
    await open(page, '/admin/access/audit', 'Audit log')
    const auditRow = page
      .locator('[data-slot="audit-table"] tbody tr:not([data-slot="audit-detail-row"])')
      .filter({ hasText: 'session_created' })
      .first()
    await expect(auditRow).toBeVisible()
    const auditDetail = await expand(page, auditRow, 'Show details', 'Hide details')
    await expect(auditDetail.getByRole('region', { name: /^Redacted details for Audit event / })).toBeVisible()
    await capture(page, out, 'audit-expanded-' + width + '-light')

    await open(page, '/admin/networks', 'Networks')
    await page.getByRole('button', { name: 'Register a Network' }).click()
    const form = page.locator('#network-create-form')
    await expect(form).toBeVisible()
    await expect(form.getByLabel('Genesis hash')).toBeVisible()
    await capture(page, out, 'networks-form-' + width + '-light')
  }

  // Initial-load failure: no last-good Node values exist yet, so the panel
  // shows the real error.
  await page.setViewportSize({ width: 1280, height: 900 })
  await page.route('**/api/admin/v1/nodes**', (route) =>
    route.fulfill({ status: 503, contentType: 'application/json', body: ERROR_BODY }),
  )
  await page.goto('/admin/nodes')
  await expect(page.getByRole('heading', { level: 1, name: 'Nodes' })).toBeVisible()
  await expect(page.locator('[data-slot="node-inventory"]').getByRole('alert')).toBeVisible({ timeout: 30_000 })
  await capture(page, out, 'nodes-error-1280-light')
  await page.unroute('**/api/admin/v1/nodes**')

  // Refresh failure: the first load succeeds, then Refresh fails; the Agent
  // panel must announce the failure and keep its last-good rows.
  await open(page, '/admin', 'Overview')
  const agentPanel = page
    .locator('article[data-slot="overview-panel"]')
    .filter({ has: page.getByRole('heading', { name: 'Agent inventory' }) })
  const lastGood = agentPanel.getByText(/^Showing \d+ of \d+ Agents$/)
  await expect(lastGood).toBeVisible()
  const lastGoodText = await lastGood.textContent()
  const overviewQueries = /\/api\/admin\/v1\/(overview|agents|nodes)(\?|$)/
  await page.route(overviewQueries, (route) =>
    route.fulfill({ status: 503, contentType: 'application/json', body: ERROR_BODY }),
  )
  await page.locator('[data-slot="overview-header"]').getByRole('button', { name: 'Refresh' }).click()
  await expect(agentPanel.getByText('Failed to refresh; showing the last successful Agent values.')).toBeVisible({
    timeout: 30_000,
  })
  await expect(agentPanel.getByText(lastGoodText!, { exact: true })).toBeVisible()
  await expect(agentPanel.getByRole('link', { name: 'View all Agents' })).toBeVisible()
  await capture(page, out, 'overview-refresh-error-1280-light')
  await page.unroute(overviewQueries)

  await writeFile(out + '/geometry.json', JSON.stringify(geometry, null, 2) + '\n')
})

// Sessions states the seeded Server cannot produce on demand (a second,
// long-named Session and a rejected revoke) come from a local route fixture.
// Only coarse client hints are shown; no token, IP, or User-Agent exists in it.
const LONG_USER = 'operator-with-an-unusually-long-username-for-review'
const SESSIONS_FIXTURE = {
  sessions: [
    {
      sessionId: 'evidence-current',
      userId: 'evidence-owner',
      username: 'admin',
      role: 'owner',
      clientHint: 'Chrome · desktop',
      createdAt: '2026-10-01T08:00:00Z',
      lastSeenAt: '2026-10-01T09:00:00Z',
      expiresAt: '2026-10-08T08:00:00Z',
      current: true,
    },
    {
      sessionId: 'evidence-other',
      userId: 'evidence-viewer',
      username: LONG_USER,
      role: 'viewer',
      clientHint: 'Firefox · mobile · a long coarse client hint that must wrap',
      createdAt: '2026-10-01T07:00:00Z',
      lastSeenAt: '2026-10-01T08:30:00Z',
      expiresAt: '2026-10-08T07:00:00Z',
      current: false,
    },
  ],
}

test('Sessions fixture states for the follow-up evidence', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop-1280', 'one project drives every width')
  test.setTimeout(600_000)
  await loginAs(page)
  const out = ROOT + '/' + phase
  await mkdir(out, { recursive: true })
  await page.route('**/api/admin/v1/sessions', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(SESSIONS_FIXTURE) }),
  )
  await page.route('**/api/admin/v1/sessions/evidence-other/revoke', (route) =>
    route.fulfill({
      status: 409,
      contentType: 'application/json',
      body: JSON.stringify({
        error: {
          code: 'session_already_revoked',
          message: 'This Session was already revoked by another operator.',
          requestId: 'evidence',
          fields: [],
        },
      }),
    }),
  )
  for (const theme of THEMES) {
    await applyTheme(page, theme)
    for (const width of WIDTHS) {
      await page.setViewportSize({ width, height: width < 768 ? 844 : 900 })
      await open(page, '/admin/access/sessions', 'Sessions')
      const row = page.locator('[data-slot="sessions-panel"] tbody tr').filter({ hasText: LONG_USER })
      await expect(row).toBeVisible()
      await capture(page, out, 'sessions-long-' + width + '-' + theme)

      await row.getByRole('button', { name: 'Revoke', exact: true }).click()
      await expect(row.getByText("Revoke now? The user's streams close immediately.")).toBeVisible()
      await expect(row.getByRole('button', { name: 'Confirm revoke' })).toBeVisible()
      await capture(page, out, 'sessions-confirm-' + width + '-' + theme)

      await row.getByRole('button', { name: 'Confirm revoke' }).click()
      await expect(page.getByRole('alert').filter({ hasText: 'already revoked by another operator' })).toBeVisible()
      await expect(row).toBeVisible()
      await capture(page, out, 'sessions-error-' + width + '-' + theme)

      await open(page, '/admin/access/sessions', 'Sessions')
      await page.getByRole('button', { name: 'Revoke all other Sessions' }).click()
      await expect(page.getByText('Revoke every other Session of your account? The current Session stays active.')).toBeVisible()
      await expect(page.getByRole('button', { name: 'Confirm revoke all others' })).toBeVisible()
      await capture(page, out, 'sessions-revoke-others-' + width + '-' + theme)
    }
  }
})