import { expect, test, type Locator, type Page } from '@playwright/test'
import type { OperationDetail, OperationSummary, DoctorOverview } from '../src/api/generated'
import {
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
} from './helpers'
import {
  HARNESS_OWNER_PASSWORD,
  HARNESS_OWNER_USERNAME,
  startDisposableServer,
} from './server-harness'

/**
 * Operations ledger and Doctor diagnostics acceptance (issue #208, #202
 * Stage 2). Every test owns one throwaway Server, port, and temporary SQLite
 * database and drives the production WebUI build against real Server HTTP:
 * no mock, no test-only seam, and no jsdom.
 *
 * The point of the flow is that a queued, running, and terminal task are all
 * observable from the surfaces that own them: Doctor queues a run, the ledger
 * shows the same task, its detail carries the recorded outcome, and a reload
 * shows the identical recorded state.
 *
 * The matrix is split across two tests and each theme is switched with an
 * emulated system preference rather than a second navigation: a full page
 * load costs more than every assertion on the page put together, and the
 * resolved theme is asserted so a dark pass cannot silently re-render light.
 */

const VIEWPORTS = [
  { width: 360, height: 800 },
  { width: 390, height: 844 },
  { width: 768, height: 1024 },
  { width: 1280, height: 800 },
  { width: 1440, height: 900 },
]
async function loginToDisposableServer(page: Page, baseUrl: string) {
  await page.context().clearCookies()
  await page.goto(baseUrl + '/login')
  await page.getByLabel('Username').fill(HARNESS_OWNER_USERNAME)
  await page.getByLabel('Password').fill(HARNESS_OWNER_PASSWORD)
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


/** Read the recorded Operation ledger over real Server HTTP. */
async function readOperations(
  server: { adminGet(path: string): Promise<{ status: number; body: unknown }> },
  query = '?kind=doctor_run&limit=10',
): Promise<OperationSummary[]> {
  const response = await server.adminGet('/api/admin/v1/operations' + query)
  expect(response.status).toBe(200)
  return response.body as OperationSummary[]
}

async function readDoctor(
  server: { adminGet(path: string): Promise<{ status: number; body: unknown }> },
): Promise<DoctorOverview> {
  const response = await server.adminGet('/api/admin/v1/doctor')
  expect(response.status).toBe(200)
  return response.body as DoctorOverview
}

/** Wait until a queued Doctor run records a terminal status on the Server. */
async function waitForRecordedRun(server: {
  adminGet(path: string): Promise<{ status: number; body: unknown }>
}): Promise<OperationSummary> {
  await expect
    .poll(
      async () => {
        const runs = await readOperations(server)
        return runs[0]?.status ?? null
      },
      { timeout: 120_000 },
    )
    .toMatch(/^(succeeded|succeeded_with_warnings|failed|cancelled)$/)
  return (await readOperations(server))[0]
}

/** Tab until the target control holds focus, so focus styling is exercised. */
async function focusByKeyboard(page: Page, target: Locator): Promise<Locator> {
  for (let press = 0; press < 120; press += 1) {
    await page.keyboard.press('Tab')
    const focused = await target
      .evaluate((element) => element === document.activeElement)
      .catch(() => false)
    if (focused) return target
  }
  throw new Error('keyboard focus never reached the control ' + target)
}

/** A wide table scrolls inside its own region, never the document. */
async function expectLocalTableScroll(page: Page, slot: string) {
  const overflowX = await page
    .locator('[data-slot="' + slot + '"]')
    .evaluate((table) => getComputedStyle(table.parentElement ?? table).overflowX)
  expect(overflowX, slot + ' must scroll inside its own region').toBe('auto')
  await expectNoHorizontalOverflow(page)
}

/**
 * The resolved theme must follow the emulated system preference, otherwise a
 * dark pass would only re-assert the light rendering it already passed.
 */
async function expectResolvedTheme(page: Page, expected: 'light' | 'dark') {
  await expect
    .poll(
      async () =>
        page.evaluate(() => document.documentElement.classList.contains('dark')),
      { message: () => 'the resolved theme must follow the emulated system preference' },
    )
    .toBe(expected === 'dark')
}

/** The ledger lays out the one recorded task without overflowing the document. */
async function expectLedgerLayout(page: Page, width: number) {
  await expect(
    page.getByRole('heading', { level: 1, name: 'Operations', exact: true }),
  ).toBeVisible()
  const ledger = page.getByRole('table', { name: 'Recorded Operations' })
  await expect(ledger.getByRole('row')).toHaveCount(2)
  await expectLocalTableScroll(page, 'operations-table')
  if (width <= 768) await expectVisibleInteractiveTargets(page)
}

/** The Doctor surface lays out the recorded report and its checks. */
async function expectDoctorLayout(page: Page, width: number) {
  await expect(
    page.getByRole('heading', { level: 1, name: 'Doctor', exact: true }),
  ).toBeVisible()
  await expect(
    page.getByRole('table', { name: /Checks from the last recorded Doctor report/ }),
  ).toBeVisible()
  await expectLocalTableScroll(page, 'doctor-checks-table')
  if (width <= 768) await expectVisibleInteractiveTargets(page)
}

test.describe('Operations ledger and Doctor diagnostics (issue #208)', () => {
  test('a Doctor run is observable from its own task flow through a real Server', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the process-heavy flow runs once')
    test.setTimeout(600_000)
    const server = await startDisposableServer()
    try {
      const context = await browser.newContext()
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)

        // A fresh Server has no report, and the page says so instead of
        // rendering an empty chart or a zero age.
        await gotoAuthenticated(page, server.baseUrl, '/admin/doctor')
        await expect(page.getByRole('heading', { level: 1, name: 'Doctor', exact: true })).toBeVisible()
        await expect(
          page.getByText('Doctor has not produced a report on this Server yet', { exact: false }),
        ).toBeVisible()

        // One click queues the run: a read-only diagnostic confirms nothing and
        // deletes nothing.
        const run = page.getByRole('button', { name: 'Run Doctor', exact: true })
        await run.click()
        await expect(page.getByText('The Server queued a Doctor run as', { exact: false })).toBeVisible()
        expect(await page.getByRole('dialog').count()).toBe(0)

        // The same run is a task in the ledger, queued or already picked up.
        await gotoAuthenticated(page, server.baseUrl, '/admin/operations')
        await expect(
          page.getByRole('heading', { level: 1, name: 'Operations', exact: true }),
        ).toBeVisible()
        const ledger = page.getByRole('table', { name: 'Recorded Operations' })
        await expect(ledger).toBeVisible()
        await expect(ledger.getByRole('row')).toHaveCount(2)

        // The Server is the authority for what the page shows.
        const [queuedRun] = await readOperations(server)
        expect(queuedRun?.kind).toBe('doctor_run')
        const taskLink = ledger.getByRole('link', { name: queuedRun?.operationId.slice(0, 8) })
        await expect(taskLink).toHaveCount(1)

        // Polling is what makes the terminal outcome observable without a
        // manual reload; the recorded status is never guessed locally.
        await expect
          .poll(
            async () => {
              const current = await readOperations(server)
              const row = current.find((operation) => operation.operationId === queuedRun?.operationId)
              return row?.status ?? null
            },
            { timeout: 120_000 },
          )
          .toMatch(/^(succeeded|succeeded_with_warnings|failed|cancelled)$/)
        const terminal = (await readOperations(server)).find(
          (operation) => operation.operationId === queuedRun?.operationId,
        )
        await expect(ledger.getByRole('row').filter({ hasText: 'Succeeded' }).first()).toBeVisible()

        // The detail page carries the recorded outcome, and nothing deletes.
        await taskLink.click()
        await expect(
          page.getByRole('heading', { level: 1, name: 'Operation detail', exact: true }),
        ).toBeVisible()
        await expect(page.getByText('Recorded outcome')).toBeVisible()
        await expect(page.getByRole('heading', { name: 'Result', exact: true })).toBeVisible()
        const responseRegion = page.getByRole('region', { name: 'Operation result payload' })
        await expect(responseRegion).toBeVisible()
        // A finished task is not cancellable and offers no destructive control.
        await expect(page.getByRole('button', { name: 'Cancel task' })).toBeDisabled()
        expect(await page.getByRole('button', { name: /Delete|Remove|Purge/ }).count()).toBe(0)
        const detail = (await server.adminGet(
          '/api/admin/v1/operations/' + String(terminal?.operationId),
        )) as { status: number; body: OperationDetail }
        expect(detail.status).toBe(200)
        expect(detail.body.cancellable).toBe(false)

        // A reload shows the identical recorded state, and the Doctor page
        // replaces the empty state with the recorded report and its checks.
        await page.reload()
        await expect(page.getByText('Recorded task', { exact: true })).toBeVisible()
        await expect(
          page.getByRole('heading', { level: 1, name: 'Operation detail', exact: true }),
        ).toBeVisible()
        await gotoAuthenticated(page, server.baseUrl, '/admin/doctor')
        await expect(
          page.getByText('Doctor has not produced a report on this Server yet', { exact: false }),
        ).toHaveCount(0)
        const checks = page.getByRole('table', { name: /Checks from the last recorded Doctor report/ })
        await expect(checks.getByRole('row')).not.toHaveCount(1)
        const doctor = await readDoctor(server)
        expect(doctor.currentRun ?? null).toBeNull()
        expect(doctor.lastRun?.operationId).toBe(terminal?.operationId)
        expect(doctor.checks.length).toBeGreaterThan(0)

        // Running Doctor again queues a second task; the report stays the
        // previous one until that run records its outcome.
        await page.getByRole('button', { name: 'Run Doctor', exact: true }).click()
        await expect(page.getByText('The Server queued a Doctor run as', { exact: false })).toBeVisible()
        const runs = await readOperations(server)
        expect(runs.length).toBeGreaterThanOrEqual(2)
      } finally {
        await context.close()
      }

      // A Guest never reaches either surface, and neither leaks a task.
      const anonymous = await browser.newContext()
      const anonymousPage = await anonymous.newPage()
      try {
        await anonymousPage.goto(server.baseUrl + '/admin/operations')
        await expect(
          anonymousPage.getByRole('heading', { level: 1, name: 'Owner access required' }),
        ).toBeVisible()
        await anonymousPage.goto(server.baseUrl + '/admin/doctor')
        await expect(
          anonymousPage.getByRole('heading', { level: 1, name: 'Owner access required' }),
        ).toBeVisible()
        expect(await anonymousPage.getByRole('table', { name: 'Recorded Operations' }).count()).toBe(0)
      } finally {
        await anonymous.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('the ledger and Doctor hold across the fixed viewport and theme matrix', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the matrix runs once against one disposable Server')
    test.setTimeout(600_000)
    const server = await startDisposableServer()
    try {
      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        // One recorded run gives both surfaces real content to lay out.
        await gotoAuthenticated(page, server.baseUrl, '/admin/doctor')
        await page.getByRole('button', { name: 'Run Doctor', exact: true }).click()
        await expect(page.getByText('The Server queued a Doctor run as', { exact: false })).toBeVisible()
        await waitForRecordedRun(server)

        for (const viewport of VIEWPORTS) {
          await page.setViewportSize(viewport)

          await page.emulateMedia({ colorScheme: 'light' })
          await gotoAuthenticated(page, server.baseUrl, '/admin/operations')
          await expectResolvedTheme(page, 'light')
          await expectLedgerLayout(page, viewport.width)
          await page.emulateMedia({ colorScheme: 'dark' })
          await expectResolvedTheme(page, 'dark')
          await expectLedgerLayout(page, viewport.width)

          await page.emulateMedia({ colorScheme: 'light' })
          await gotoAuthenticated(page, server.baseUrl, '/admin/doctor')
          await expectResolvedTheme(page, 'light')
          await expectDoctorLayout(page, viewport.width)
          await page.emulateMedia({ colorScheme: 'dark' })
          await expectResolvedTheme(page, 'dark')
          await expectDoctorLayout(page, viewport.width)
        }
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('a real failure and a real cancel request read exactly as the Server recorded them', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the lifecycle matrix runs once')
    test.setTimeout(600_000)
    // Seeded only while the Server's database is still closed. An external
    // SQLite connection that writes to a live WAL Server destroys the
    // `-wal` sidecar (see DisposableServerOptions), and the Server re-arms
    // rows left running by a crash, so an in-flight state cannot be staged
    // at all: every active state below is produced by the real worker.
    const seedSql = `
      INSERT INTO operations (
        operation_id, kind, status, progress_percent, progress_label, request_id,
        params_json, warnings_json, errors_json, result_json, created_by_user_id,
        created_at, started_at, finished_at, audit_event_id, cancel_requested
      ) VALUES
      ('e2e-retired-backup-create', 'backup_create', 'queued', 0, 'Queued', NULL, '{}', '[]', '[]', NULL, NULL,
        strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-300 seconds'), NULL, NULL, NULL, 0),
      ('e2e-failed-doctor', 'doctor_run', 'failed', 61, 'Checking storage', NULL, '{}', '[]',
        '["backup storage check failed: the configured path is not readable"]', NULL, NULL,
        strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-240 seconds'),
        strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-230 seconds'),
        strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-220 seconds'), NULL, 0),
      ('e2e-cancelled-restore', 'restore', 'cancelled', 15, 'Restoring tables', NULL, '{}', '[]', '[]', NULL, NULL,
        strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-180 seconds'),
        strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-170 seconds'),
        strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-160 seconds'), NULL, 1);
    `
    const server = await startDisposableServer({ seedSql })
    try {
      const context = await browser.newContext()
      const page = await context.newPage()
      try {
        const STATUS_LABELS = [
          'Cancelled',
          'Failed',
          'Succeeded with warnings',
          'Succeeded',
          'Running',
          'Queued',
          'Unknown',
        ]
        const TERMINAL_LABELS = new Set([
          'Cancelled',
          'Failed',
          'Succeeded with warnings',
          'Succeeded',
        ])
        const recorded = async (id: string) =>
          (await readOperations(server, '?limit=200')).find(
            (operation) => operation.operationId === id,
          )
        const pageStatus = async () => {
          for (const label of STATUS_LABELS) {
            if (await page.getByText(label, { exact: true }).first().isVisible().catch(() => false)) {
              return label
            }
          }
          return 'unread'
        }

        await loginToDisposableServer(page, server.baseUrl)
        await gotoAuthenticated(page, server.baseUrl, '/admin/operations')
        const ledger = page.getByRole('table', { name: 'Recorded Operations' })
        await expect(ledger).toBeVisible()
        const rowFor = (id: string) =>
          ledger
            .getByRole('row')
            .filter({ has: page.locator('a[href="/admin/operations/' + id + '"]') })
        const ledgerStatus = async (id: string) => {
          for (const label of STATUS_LABELS) {
            if (await rowFor(id).getByText(label, { exact: true }).first().isVisible().catch(() => false)) {
              return label
            }
          }
          return 'unread'
        }

        // A failure the Server produces by itself: the worker claims the
        // staged `backup_create` row and finalises it, so this is a recorded
        // failure rather than a staged one.
        const failedOperationId = 'e2e-retired-backup-create'
        await expect.poll(async () => (await recorded(failedOperationId))?.status, {
          timeout: 60_000,
        }).toBe('failed')
        await expect.poll(() => ledgerStatus(failedOperationId), { timeout: 30_000 }).toBe('Failed')
        await expect(rowFor(failedOperationId)).toContainText('Not applicable')

        // The detail page reports the recorded outcome, keeps a task with no
        // Audit link honest about it, and offers no cancel for a finished one.
        await gotoAuthenticated(page, server.baseUrl, '/admin/operations/' + failedOperationId)
        await expect(
          page.getByRole('heading', { level: 1, name: 'Operation detail', exact: true }),
        ).toBeVisible()
        await expect.poll(() => pageStatus(), { timeout: 30_000 }).toBe('Failed')
        await expect(page.getByText('Not recorded').first()).toBeVisible()
        expect(page.getByRole('link', { name: /^Event / })).toHaveCount(0)
        expect(await page.getByRole('button', { name: 'Cancel task', exact: true }).isDisabled()).toBe(true)

        // A failed run carries no report, so Doctor keeps naming the absence
        // instead of inventing one: the ledger says Failed and Doctor still
        // says it has never produced a report.
        await gotoAuthenticated(page, server.baseUrl, '/admin/doctor')
        await expect(page.getByText('Last recorded report')).toBeVisible()
        await expect(
          page.getByText('Doctor has not produced a report on this Server yet'),
        ).toBeVisible()

        // A task that already carries a cancel request offers no second
        // request to make.
        await gotoAuthenticated(page, server.baseUrl, '/admin/operations/e2e-cancelled-restore')
        await expect.poll(() => pageStatus(), { timeout: 30_000 }).toBe('Cancelled')
        await expect(page.getByText('Cancel requested').first()).toBeVisible()
        expect(await page.getByRole('button', { name: 'Cancel task', exact: true }).isDisabled()).toBe(true)

        // Real work: the worker takes one queued task per tick, so a queue of
        // Doctor runs holds Queued rows for long enough to be read.
        for (let queued = 0; queued < 12; queued += 1) {
          const queuedRun = await server.adminPost('/api/admin/v1/doctor', {})
          expect(
            queuedRun.status,
            'queueing a real Doctor run: ' + JSON.stringify(queuedRun.body),
          ).toBe(200)
        }
        const queuedRuns = async () =>
          (await readOperations(server, '?kind=doctor_run&limit=200')).filter(
            (operation) => !operation.operationId.startsWith('e2e-') && operation.status === 'queued',
          )
        await expect.poll(() => queuedRuns().then((rows) => rows.length), { timeout: 60_000 })
          .toBeGreaterThan(2)
        await gotoAuthenticated(page, server.baseUrl, '/admin/operations')
        await expect(ledger).toBeVisible()
        await expect(ledger.getByText('Queued', { exact: true }).first()).toBeVisible()
        const targetId = String((await queuedRuns())[0].operationId)
        await expect(rowFor(targetId)).toBeVisible()

        // A cancel REQUEST is recorded as a request. Sampling the page and
        // the Server together proves the page never claims a terminal
        // outcome the Server has not recorded yet.
        await gotoAuthenticated(page, server.baseUrl, '/admin/operations/' + targetId)
        await expect(
          page.getByRole('heading', { level: 1, name: 'Operation detail', exact: true }),
        ).toBeVisible()
        const samples: { page: string; server: string }[] = []
        const sample = async () => {
          samples.push({ page: await pageStatus(), server: (await recorded(targetId))?.status ?? 'missing' })
        }
        await sample()
        await page.getByRole('button', { name: 'Cancel task', exact: true }).click()
        await page.getByRole('button', { name: 'Confirm cancel request', exact: true }).click()
        await expect(page.getByText('The Server recorded the cancel request')).toBeVisible()
        await expect(page.getByText('Cancel requested').first()).toBeVisible()
        await sample()

        // The Server owns the outcome: a queued task becomes Cancelled at
        // once, a running task keeps its recorded status until the worker
        // writes the terminal one.
        await expect.poll(async () => (await recorded(targetId))?.status, { timeout: 60_000 })
          .toMatch(/^(succeeded|succeeded_with_warnings|failed|cancelled)$/)
        const finalStatus = await recorded(targetId)
        expect(finalStatus?.cancelRequested).toBe(true)
        expect(finalStatus?.finishedAt != null).toBe(true)
        expect(samples.length).toBe(2)
        for (const sample of samples) {
          if (TERMINAL_LABELS.has(sample.page)) {
            expect(['cancelled', 'failed', 'succeeded', 'succeeded_with_warnings']).toContain(
              sample.server,
            )
          }
        }

        // The page settles on the outcome the Server recorded, not on a local
        // guess made when the request was sent, and a reload agrees.
        const recordedLabel = {
          cancelled: 'Cancelled',
          failed: 'Failed',
          succeeded: 'Succeeded',
          succeeded_with_warnings: 'Succeeded with warnings',
        }[finalStatus?.status ?? '']
        await expect.poll(() => pageStatus(), { timeout: 30_000 }).toBe(recordedLabel)
        await page.reload()
        await expect.poll(() => pageStatus(), { timeout: 30_000 }).toBe(recordedLabel)
        await expect(page.getByText('Cancel requested').first()).toBeVisible()

        // The real runs replace the empty report: Doctor then names the
        // report the Server recorded, with its own recorded checks.
        await gotoAuthenticated(page, server.baseUrl, '/admin/doctor')
        await expect(
          page.getByRole('table', { name: 'Checks from the last recorded Doctor report' }),
        ).toBeVisible({ timeout: 90_000 })
        await expect
          .poll(
            async () => {
              for (const label of ['Succeeded', 'Succeeded with warnings']) {
                if (await page.getByText(label, { exact: true }).first().isVisible().catch(() => false)) {
                  return label
                }
              }
              return 'no recorded report yet'
            },
            { timeout: 90_000 },
          )
          .toMatch(/^Succeeded/)
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('Doctor follows a queued run to its report with the event stream cut off', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the polling proof runs once')
    test.setTimeout(600_000)
    const server = await startDisposableServer()
    try {
      const context = await browser.newContext()
      // The Admin event stream is severed, so nothing but the page's own
      // polling can carry the run to its recorded outcome.
      await context.route('**/api/admin/v1/events*', (route) => route.abort())
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        await gotoAuthenticated(page, server.baseUrl, '/admin/doctor')
        await page.getByRole('button', { name: 'Run Doctor', exact: true }).click()
        await expect(page.getByText('The Server queued a Doctor run as', { exact: false })).toBeVisible()

        // The page is never reloaded and never navigated away from: the run
        // is in flight first and the finished report arrives by polling.
        await expect(page.getByText('Run in flight')).toBeVisible()
        // The intermediate state is rendered, not skipped: a real run is
        // Queued or Running before it records an outcome.
        await expect(page.getByText(/^This run is (Queued|Running)/)).toBeVisible()
        await expect(
          page.getByRole('table', { name: 'Checks from the last recorded Doctor report' }),
        ).toBeVisible({ timeout: 90_000 })
        await expect(page.getByText('Run in flight')).toHaveCount(0)

        // The Server agrees, so the UI never outran the record.
        const overview = await readDoctor(server)
        expect(overview.currentRun).toBeNull()
        expect(overview.lastRun?.status).toMatch(/^(succeeded|succeeded_with_warnings)$/)
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('a recorded task, the run command, and touch reach hold at every width', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the interaction matrix runs once')
    test.setTimeout(600_000)
    const server = await startDisposableServer()
    try {
      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        await gotoAuthenticated(page, server.baseUrl, '/admin/doctor')
        await page.getByRole('button', { name: 'Run Doctor', exact: true }).click()
        await expect(page.getByText('The Server queued a Doctor run as', { exact: false })).toBeVisible()
        const recorded = await waitForRecordedRun(server)

        // A terminal task is legible in its detail at every width and in
        // both themes, and the recorded outcome scrolls inside its own
        // region instead of widening the page.
        const detailPath = '/admin/operations/' + String(recorded?.operationId)
        for (const colorScheme of ['light', 'dark'] as const) {
          await page.emulateMedia({ colorScheme })
          for (const viewport of VIEWPORTS) {
            await page.setViewportSize(viewport)
            await gotoAuthenticated(page, server.baseUrl, detailPath)
            await expectResolvedTheme(page, colorScheme)
            await expect(
              page.getByRole('heading', { level: 1, name: 'Operation detail', exact: true }),
            ).toBeVisible()
            await expect(page.getByRole('region', { name: 'Operation result payload' })).toBeVisible()
            await expectNoHorizontalOverflow(page)
            if (viewport.width <= 768) await expectVisibleInteractiveTargets(page)
          }
        }
        await page.emulateMedia({ colorScheme: 'light' })

        // The read-only run command is keyboard reachable and visibly focused
        // in both themes.
        await page.setViewportSize(VIEWPORTS[3])
        for (const colorScheme of ['light', 'dark'] as const) {
          await page.emulateMedia({ colorScheme })
          await gotoAuthenticated(page, server.baseUrl, '/admin/doctor')
          await expectResolvedTheme(page, colorScheme)
          const runButton = page.getByRole('button', { name: 'Run Doctor', exact: true })
          await focusByKeyboard(page, runButton)
          await expectFocusedElementHasVisibleFocus(page)
        }

        // A touch tap reaches the command and the recorded state never moves
        // backwards: rendering the pages queued nothing.
        await page.setViewportSize(VIEWPORTS[0])
        await gotoAuthenticated(page, server.baseUrl, '/admin/doctor')
        await page.getByRole('button', { name: 'Run Doctor', exact: true }).tap()
        await expect(page.getByText('The Server queued a Doctor run as', { exact: false })).toBeVisible()
        expect((await readOperations(server)).length).toBeGreaterThanOrEqual(2)
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })
})
