import { expect, test, type Locator, type Page } from '@playwright/test'

import {
  expectLocalTableScroll,
  expectResolvedTheme,
  focusByKeyboard,
  gotoAuthenticated,
  loginToDisposableServer,
} from './admin-flow'
import {
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
  loginAs,
} from './helpers'
import { startDisposableServer } from './server-harness'

/**
 * Retention surface acceptance (issue #210), driven through the production
 * WebUI build against a real Server over HTTP - no mock, no jsdom.
 *
 * The first three tests own a throwaway Server (its own port, temp state dir
 * and SQLite database) and run once, on desktop-1280, because each one boots a
 * Server and queues a real retention run. The last test is viewport-driven and
 * uses the shared CI Server through baseURL, so it runs for every Playwright
 * project in playwright.config.ts.
 *
 * What the flow has to prove, in the words of the Server's own contract:
 *  1. The Server owns the catalogue, the safety floors and the safety ceiling.
 *     A value the Server refuses is refused here too and cannot be saved, and a
 *     saved value is read back from the Server, not assumed.
 *  2. A preview is the authority for a run: it binds the policy versions, the
 *     scope and a per-family cutoff, and its numbers are labelled as Server
 *     estimates (upper bounds), never as a frozen set of rows.
 *  3. A run is queued by preview id. When the Server refuses a preview that no
 *     longer matches the policies, nothing is queued, the page says so, and it
 *     recovers by composing a new preview rather than retrying the refused one.
 */

/** The read-only Admin seam this spec needs: it asserts the status for us. */
type AdminServer = { expectAdminGet(path: string, status: number): Promise<unknown> }

type PolicyView = {
  defaultDays: number
  enabled: boolean
  family: string
  label: string
  maxDays: number
  minDays: number
  policyVersion: string
  retentionDays: number
  supported: boolean
  updatedAt: string
  updatedBy?: string | null
}

type PreviewFamilyView = {
  cutoff: string
  estimatedRows: number
  family: string
  policyVersion: string
  retentionDays: number
}

type PreviewView = {
  createdAt: string
  createdBy: string
  estimatedRows: number
  expiresAt: string
  families: PreviewFamilyView[]
  notes: string[]
  policyVersion: string
  previewId: string
  scope?: string[] | null
  skipped: { code: string; family: string; message: string }[]
}

type OverviewView = {
  lastRun?: { kind: string; operationId: string; status: string } | null
  policies: PolicyView[]
  preview?: PreviewView | null
  protectedState: string[]
}

async function readOverview(server: AdminServer): Promise<OverviewView> {
  return (await server.expectAdminGet('/api/admin/v1/retention', 200)) as OverviewView
}

/** The page's own formatters, so an assertion matches exactly what is rendered. */
function shortId(value: string): string {
  return value.length <= 12 ? value : value.slice(0, 8) + '…' + value.slice(-4)
}

function observedAt(timestamp: string): string {
  return timestamp.slice(0, 19).replace('T', ' ') + ' UTC'
}

function retentionLabel(days: number): string {
  if (days === 0) return 'Keep forever'
  if (days === 1) return '1 day'
  return String(days) + ' days'
}

function slot(page: Page, name: string): Locator {
  return page.locator('[data-slot="' + name + '"]')
}

/** The catalogue row for one family. The Server's own family key is unique per
 *  row, so match on it: a label can be a prefix of a longer family name
 *  ("1-Hour Aggregates" also matches "Peer 1-Hour Aggregates"). */
function policyRow(page: Page, family: string): Locator {
  return page
    .locator('table[data-slot="retention-policies-table"] tbody tr')
    .filter({ has: page.getByText(family, { exact: true }) })
}

/** A catalogue entry the Server both implements and lets an Owner change. */
function requirePolicy(
  policies: PolicyView[],
  matches: (policy: PolicyView) => boolean,
  description: string,
): PolicyView {
  const found = policies.find(matches)
  if (!found) throw new Error('the Server catalogue exposes no ' + description)
  return found
}

/** A different value that stays inside the bounds the Server published. */
function changedDays(policy: PolicyView): number {
  if (policy.maxDays > 0 && policy.retentionDays >= policy.maxDays) return policy.retentionDays - 1
  return policy.retentionDays + 1
}

/**
 * Change a policy without going through the page under test, so a preview can
 * be made stale behind the page's back. The write runs inside the page: the
 * browser then sends a same-origin Origin header, which the Server's mutation
 * guard requires alongside the CSRF token (design 13.3).
 */
async function changePolicyOutOfBand(
  page: Page,
  family: string,
  retentionDays: number,
  expectedPolicyVersion: string,
): Promise<{ status: number; body: unknown }> {
  const csrf = await page.evaluate(async () => {
    const response = await fetch('/api/public/v1/session')
    const body = (await response.json()) as { csrfToken: string }
    return body.csrfToken
  })
  return page.evaluate(
    async (input: { csrf: string; expectedPolicyVersion: string; family: string; retentionDays: number }) => {
      const response = await fetch('/api/admin/v1/retention/policies/' + input.family, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': input.csrf },
        body: JSON.stringify({
          retentionDays: input.retentionDays,
          expectedPolicyVersion: input.expectedPolicyVersion,
        }),
      })
      return { status: response.status, body: (await response.json()) as unknown }
    },
    { csrf, family, retentionDays, expectedPolicyVersion },
  )
}

/**
 * Start the run the way the page requires it: the operator types the token the
 * run panel asks for, which names the preview the run is bound to. The token is
 * read from the page, never assumed, so a run is never started by a bare click.
 */
async function confirmRun(page: Page, previewId: string) {
  const panel = slot(page, 'retention-run')
  await expect(panel).toContainText('run ' + previewId.slice(0, 12))
  await panel.getByLabel('Type the run to confirm').fill('run ' + previewId.slice(0, 12))
  await expect(slot(page, 'retention-run-submit')).toBeEnabled()
}

async function composePreview(page: Page, server: AdminServer, previousId: string | null): Promise<PreviewView> {
  await slot(page, 'retention-preview-compose').click()
  // Composing is a Server write, so wait for the Server to report the preview
  // the page then binds to rather than trusting the button click.
  await expect
    .poll(
      async () => {
        const preview = (await readOverview(server)).preview
        return preview ? preview.previewId : null
      },
      { timeout: 60_000 },
    )
    .not.toBe(previousId)
  const preview = (await readOverview(server)).preview
  if (!preview) throw new Error('the Server reported no bound preview after composing one')
  return preview
}

test('the Server owns the retention bound: its refusals gate the save and the saved value comes back', async ({
  browser,
}, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-1280', 'the Server-heavy flow runs once')
  test.setTimeout(600_000)
  const server = await startDisposableServer()
  try {
    const context = await browser.newContext()
    const page = await context.newPage()
    try {
      await loginToDisposableServer(page, server.baseUrl)
      await gotoAuthenticated(page, server.baseUrl, '/admin/retention')
      const overview = await readOverview(server)
      expect(overview.policies.length).toBeGreaterThan(0)
      const raw = requirePolicy(
        overview.policies,
        (policy) => policy.supported && policy.enabled && policy.maxDays > 0 && policy.minDays >= 1,
        'bounded, actionable family',
      )
      const unsupported = requirePolicy(overview.policies, (policy) => !policy.supported, 'family without cleanup')

      // The table is the Server's catalogue, not a list baked into the page.
      await expect(page.getByRole('heading', { level: 1, name: 'Retention' })).toBeVisible()
      await expect(page.getByText('Retention policies · ' + String(overview.policies.length))).toBeVisible()
      const table = slot(page, 'retention-policies-table')
      await expect(table).toBeVisible()
      await expectLocalTableScroll(page, 'retention-policies-table')
      await expectNoHorizontalOverflow(page)

      const rawRow = policyRow(page, raw.family)
      await expect(rawRow).toContainText(raw.label)
      await expect(rawRow).toContainText(raw.family)
      await expect(rawRow).toContainText(retentionLabel(raw.retentionDays))
      await expect(rawRow).toContainText(retentionLabel(raw.minDays) + ' – ' + retentionLabel(raw.maxDays))

      // A family the Server implements no cleanup for claims no retention and
      // offers no edit: unsupported is not rendered as a value or as Healthy.
      const unsupportedRow = policyRow(page, unsupported.family)
      await expect(unsupportedRow).toContainText(unsupported.label)
      await expect(unsupportedRow).toContainText('The Server implements no cleanup for this family')
      await expect(unsupportedRow).toContainText('Unsupported')
      await expect(unsupportedRow.getByRole('button')).toHaveCount(0)

      await rawRow.getByRole('button', { name: 'Edit ' + raw.label + ' retention' }).click()
      const editor = slot(page, 'retention-edit')
      await expect(editor).toBeVisible()
      const days = editor.getByLabel('New retention (days)')
      await expect(days).toHaveValue(String(raw.retentionDays))
      await expect(slot(page, 'retention-save')).toBeDisabled()

      // Above the ceiling: the page names the ceiling from the Server's own
      // bounds, the Server refuses the value, and no save is offered.
      await days.fill(String(raw.maxDays + 1))
      await expect(
        editor.getByText('The safety ceiling for this family is ' + retentionLabel(raw.maxDays) + '.'),
      ).toBeVisible()
      await expect(slot(page, 'retention-impact')).toContainText('The Server refuses this value')
      await expect(slot(page, 'retention-save')).toBeDisabled()

      // Below the floor: refused the same way, never silently accepted.
      await days.fill(String(raw.minDays - 1))
      await expect(
        editor.getByText('The safety floor for this family is ' + retentionLabel(raw.minDays) + '.'),
      ).toBeVisible()
      await expect(slot(page, 'retention-save')).toBeDisabled()

      const nextDays = changedDays(raw)
      await days.fill(String(nextDays))
      const impact = slot(page, 'retention-impact')
      await expect(impact).toContainText('Impact estimate')
      await expect(impact).toContainText(
        /About \d+ rows are older than this bound now\.|The Server reports no row estimate for this family\./,
      )
      await editor
        .getByLabel('Type the change to confirm')
        .fill('retention ' + raw.family + ' ' + String(nextDays))
      const save = slot(page, 'retention-save')
      await expect(save).toBeEnabled()
      await save.click()

      // The confirmation is audited and the editor reports the Server's value.
      // Addressed by slot: the editor also carries a live "moved under draft"
      // status, so a bare role=status lookup would be ambiguous.
      const notice = editor.locator('[data-slot="retention-save-notice"]')
      await expect(notice).toContainText(raw.label + ' is now retained for ' + retentionLabel(nextDays))
      await expect(notice).toContainText('Audit #')
      await expect(days).toHaveValue(String(nextDays))

      // A saved policy is a bound, not a deletion: the Server recorded the new
      // value and a new fingerprint, and queued no run.
      const saved = await readOverview(server)
      const savedPolicy = saved.policies.find((policy) => policy.family === raw.family)
      expect(savedPolicy?.retentionDays).toBe(nextDays)
      expect(savedPolicy?.policyVersion).not.toBe(raw.policyVersion)
      expect(saved.lastRun ?? null).toBeNull()
      await expect(policyRow(page, raw.family)).toContainText(retentionLabel(nextDays))
    } finally {
      await context.close()
    }
  } finally {
    await server.dispose()
  }
})

test('a composed preview binds the plan, and the run is queued against that preview id', async ({
  browser,
}, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-1280', 'the Server-heavy flow runs once')
  test.setTimeout(600_000)
  const server = await startDisposableServer()
  try {
    const context = await browser.newContext()
    const page = await context.newPage()
    try {
      await loginToDisposableServer(page, server.baseUrl)
      await gotoAuthenticated(page, server.baseUrl, '/admin/retention')
      expect((await readOverview(server)).preview ?? null).toBeNull()
      await expect(slot(page, 'retention-preview')).toContainText('No preview is bound on this Server')
      await expect(slot(page, 'retention-run')).toContainText('Compose a preview before running retention')
      await expect(slot(page, 'retention-run-submit')).toBeDisabled()

      const preview = await composePreview(page, server, null)
      expect(preview.families.length).toBeGreaterThan(0)

      // Everything a run would execute is the preview's frozen state, and every
      // count is labelled an estimate rather than a promise about rows.
      const panel = slot(page, 'retention-preview')
      await expect(panel).toContainText(shortId(preview.previewId))
      await expect(panel).toContainText(preview.createdBy)
      await expect(panel).toContainText(observedAt(preview.createdAt))
      await expect(panel).toContainText(observedAt(preview.expiresAt))
      await expect(panel).toContainText(preview.scope ? preview.scope.join(', ') : 'Every enabled, supported family')
      await expect(panel).toContainText(String(preview.estimatedRows) + ' (Server estimate, an upper bound)')

      const first = preview.families[0]
      if (!first) throw new Error('the Server bound no family in this preview')
      const familyRow = page.locator('[data-slot="retention-preview-family"]').first()
      await expect(familyRow).toContainText(first.family)
      await expect(familyRow).toContainText(retentionLabel(first.retentionDays))
      await expect(familyRow).toContainText(observedAt(first.cutoff))
      await expect(familyRow).toContainText(String(first.estimatedRows))
      await expectLocalTableScroll(page, 'retention-preview-families')
      await expectNoHorizontalOverflow(page)

      // Families the Server will not act on are reported with its reason, not
      // hidden and not counted as zero.
      if (preview.skipped.length > 0) {
        await expect(slot(page, 'retention-preview-skipped')).toContainText(preview.skipped[0]?.code ?? '')
      } else {
        await expect(slot(page, 'retention-preview-skipped')).toHaveCount(0)
      }
      if (preview.notes.length > 0) {
        await expect(slot(page, 'retention-preview-notes')).toContainText(preview.notes[0] ?? '')
      } else {
        await expect(slot(page, 'retention-preview-notes')).toHaveCount(0)
      }

      // The run is offered for exactly this preview id, and starting it is one
      // explicit command: a bound preview alone leaves the button unavailable
      // until the typed confirmation names that preview.
      await expect(slot(page, 'retention-run-preview-id')).toHaveText(preview.previewId)
      const run = slot(page, 'retention-run-submit')
      await expect(run).toBeDisabled()
      await run.click({ force: true })
      await expect(run).toBeDisabled()
      await confirmRun(page, preview.previewId)
      await run.click()
      await expect(slot(page, 'retention-run')).toContainText('The Server queued the retention run as')

      const queued = (await readOverview(server)).lastRun
      if (!queued) throw new Error('the Server recorded no retention run')
      expect(queued.kind).toBe('retention_run')
      await expect(slot(page, 'retention-last-run')).toContainText(shortId(queued.operationId))
      await expect(page.locator('a[href="/admin/operations/' + queued.operationId + '"]')).toHaveCount(1)

      // The recorded run is durable Server state, not page state.
      await page.reload()
      await expect(slot(page, 'retention-last-run')).toContainText(shortId(queued.operationId))
      await expect(page.locator('a[href="/admin/operations/' + queued.operationId + '"]')).toHaveCount(1)
      await expect(slot(page, 'retention-preview')).toContainText(shortId(preview.previewId))
    } finally {
      await context.close()
    }
  } finally {
    await server.dispose()
  }
})

test('a policy change after a preview makes the run stale: nothing is queued and a new preview recovers', async ({
  browser,
}, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-1280', 'the Server-heavy flow runs once')
  test.setTimeout(600_000)
  const server = await startDisposableServer()
  try {
    const context = await browser.newContext()
    const page = await context.newPage()
    try {
      await loginToDisposableServer(page, server.baseUrl)
      await gotoAuthenticated(page, server.baseUrl, '/admin/retention')
      const preview = await composePreview(page, server, null)
      const bound = preview.families[0]
      if (!bound) throw new Error('the Server bound no family in this preview')

      // An edit the page never saw: the composed preview no longer matches what
      // the Server would execute.
      const overview = await readOverview(server)
      const policy = requirePolicy(
        overview.policies,
        (entry) => entry.family === bound.family,
        'policy for the family the preview bound',
      )
      const nextDays = changedDays(policy)
      const written = await changePolicyOutOfBand(page, policy.family, nextDays, policy.policyVersion)
      expect(written.status, JSON.stringify(written.body)).toBe(200)

      // The page holds the preview it composed and cannot see the change, so it
      // still offers the run - the Server is the one that refuses it.
      const run = slot(page, 'retention-run-submit')
      await confirmRun(page, preview.previewId)
      await run.click()
      const runPanel = slot(page, 'retention-run')
      const previewPanel = slot(page, 'retention-preview')
      // The run panel quotes the Server's refusal verbatim; the preview panel
      // carries the page's own explanation of what happens next.
      await expect(runPanel).toContainText('no longer matches the current policies')
      await expect(previewPanel).toContainText('The Server refused a run for this preview')
      await expect(previewPanel).toContainText('this page will not retry it')

      // A refused run queues nothing: the Server recorded no retention run, and
      // the page stops offering the refused preview.
      const refused = await readOverview(server)
      expect(refused.lastRun ?? null).toBeNull()
      expect(refused.preview ?? null).toBeNull()
      await expect(run).toBeDisabled()

      // Recovery is a new preview with new estimates, never a retry.
      const fresh = await composePreview(page, server, preview.previewId)
      expect(fresh.previewId).not.toBe(preview.previewId)
      await expect(slot(page, 'retention-preview')).toContainText(shortId(fresh.previewId))
      await expect(slot(page, 'retention-preview')).not.toContainText('refused a run for this preview')
      // The confirmation named the refused preview and is never inherited: the
      // replacement preview is confirmed again before it can be run.
      await expect(run).toBeDisabled()
      await confirmRun(page, fresh.previewId)
      await run.click()
      await expect(runPanel).toContainText('The Server queued the retention run as')

      const queued = (await readOverview(server)).lastRun
      if (!queued) throw new Error('the Server recorded no retention run after the recovery')
      await expect(slot(page, 'retention-last-run')).toContainText(shortId(queued.operationId))
    } finally {
      await context.close()
    }
  } finally {
    await server.dispose()
  }
})

test('the retention surface stays inside the viewport, themed, and reachable by keyboard', async ({ page }) => {
  await loginAs(page)
  await page.goto('/admin/retention')
  await expect(page.getByRole('heading', { level: 1, name: 'Retention' })).toBeVisible()

  // The nav entry the route belongs to is the active one here. Below the lg
  // breakpoint the Admin nav is a closed drawer, so the entry is reached
  // through its hidden state rather than required to be visible.
  const adminNav = page.getByRole('navigation', { name: 'Admin', includeHidden: true })
  await expect(
    adminNav.getByRole('link', { name: 'Retention', exact: true, includeHidden: true }),
  ).toHaveAttribute(
    'aria-current',
    'page',
  )

  const table = slot(page, 'retention-policies-table')
  await expect(table).toBeVisible()
  await expectLocalTableScroll(page, 'retention-policies-table')
  await expectNoHorizontalOverflow(page)

  for (const colorScheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme })
    await expectResolvedTheme(page, colorScheme)
    await expect(table).toBeVisible()
    await expectNoHorizontalOverflow(page)
  }

  const viewport = page.viewportSize()
  if (viewport && viewport.width <= 768) {
    await expectVisibleInteractiveTargets(page)
  }

  // The editor opens from the keyboard, and nothing is saved before the
  // Server's estimate arrives and the typed confirmation matches.
  const edit = page.locator('[data-slot="retention-policy-edit"]').first()
  await expect(edit).toBeVisible()
  await focusByKeyboard(page, edit)
  await expectFocusedElementHasVisibleFocus(page)
  await page.keyboard.press('Enter')
  const editor = slot(page, 'retention-edit')
  await expect(editor).toBeVisible()
  await expect(editor.getByLabel('New retention (days)')).toBeVisible()
  await expect(slot(page, 'retention-impact')).toContainText('Impact estimate')
  await expect(slot(page, 'retention-save')).toBeDisabled()
  await expectNoHorizontalOverflow(page)
  await editor.getByRole('button', { name: 'Close' }).click()
  await expect(editor).toHaveCount(0)
})
