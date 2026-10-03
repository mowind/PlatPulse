import { expect, test, type Page } from '@playwright/test'
import {
  VIEWPORTS,
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
} from './helpers'
import { startDisposableServer } from './server-harness'

/**
 * Backup artifact inspection and verification acceptance (issue #209, #202
 * Stage 2 ticket 7). Every test owns one throwaway Server, port, and temporary
 * SQLite database and drives the production WebUI build against real Server
 * HTTP: no mock, no test-only seam, and no jsdom.
 *
 * The point of the flow is that an artifact the Server wrote during an offline
 * backup window is readable from its own page, that requesting verification is
 * visibly only an acceptance, and that the outcome the page shows is the one
 * the Server recorded - including a corrupted file and a file that vanished.
 * Nothing here creates, restores, or deletes an artifact online: the artifact
 * is created by the offline `platpulse-server backup` command and the failures
 * are produced by damaging the file on the Server host, which is what an
 * operator would actually find.
 */

type AdminServer = {
  adminGet(path: string): Promise<{ status: number; body: unknown }>
}

type ArtifactSummary = Record<string, unknown>

/** Read the recorded artifact ledger over real Server HTTP. */
async function readArtifacts(server: AdminServer): Promise<ArtifactSummary[]> {
  const response = await server.adminGet('/api/admin/v1/backups')
  expect(response.status).toBe(200)
  return response.body as ArtifactSummary[]
}

/** Read one recorded artifact, including the Server's own verification reason. */
async function readArtifact(
  server: AdminServer,
  artifactId: string,
): Promise<{ artifact: ArtifactSummary; verificationError: string | null }> {
  const response = await server.adminGet('/api/admin/v1/backups/' + artifactId)
  expect(response.status).toBe(200)
  return response.body as { artifact: ArtifactSummary; verificationError: string | null }
}

/** Wait until the worker records a terminal verification outcome on the Server. */
async function waitForVerification(
  server: AdminServer,
  artifactId: string,
  expected: string,
): Promise<{ artifact: ArtifactSummary; verificationError: string | null }> {
  await expect
    .poll(
      async () => (await readArtifact(server, artifactId)).artifact.verification ?? null,
      { timeout: 120_000 },
    )
    .toBe(expected)
  return readArtifact(server, artifactId)
}

/** The recorded artifact ledger, without any control that would change it. */
async function expectBackupsLayout(page: Page, width: number) {
  await expect(page.getByRole('heading', { level: 1, name: 'Backups', exact: true })).toBeVisible()
  await expect(page.locator('table[data-slot="backups-table"]')).toBeVisible()
  await expectLocalTableScroll(page, 'backups-table')
  if (width <= 768) await expectVisibleInteractiveTargets(page)
}

/** The artifact detail surface, whose title is the recorded file name. */
async function expectArtifactLayout(page: Page, filename: string, width: number) {
  await expect(page.getByRole('heading', { level: 1, name: filename })).toBeVisible()
  await expect(page.locator('[data-slot="backup-verification"]')).toBeVisible()
  await expectNoHorizontalOverflow(page)
  if (width <= 768) await expectVisibleInteractiveTargets(page)
}

test.describe('Backup artifact inspection and verification (issue #209)', () => {
  test('an offline artifact is readable and its acceptance is not yet a result', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the process-heavy flow runs once')
    test.setTimeout(900_000)
    const server = await startDisposableServer()
    try {
      const context = await browser.newContext()
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)

        // A Server that has never run an offline backup says so, and offers
        // no way to create one: creation is a command, not a web action.
        await gotoAuthenticated(page, server.baseUrl, '/admin/backups')
        await expect(
          page.getByText('No backup artifact is recorded yet', { exact: false }),
        ).toBeVisible()
        expect(await readArtifacts(server)).toHaveLength(0)

        // The artifact only exists because the offline command made it.
        const artifact = await server.createOfflineBackup()
        await gotoAuthenticated(page, server.baseUrl, '/admin/backups')
        const table = page.locator('table[data-slot="backups-table"]')
        await expect(table).toBeVisible()
        await expect(table.getByRole('row')).toHaveCount(2)
        const recorded = (await readArtifacts(server))[0]
        expect(recorded?.artifactId).toBe(artifact.artifactId)

        // The list reads the recorded manifest and the verification state the
        // Server actually holds, which is still "nothing recorded yet".
        const artifactLink = table.getByRole('link', { name: artifact.filename })
        await expect(artifactLink).toHaveAttribute('href', '/admin/backups/' + artifact.artifactId)
        await expect(table.getByText('Not verified')).toBeVisible()
        await expect(table.getByText('No verification outcome recorded')).toBeVisible()
        expect(
          await page.getByRole('button', { name: /Create|Restore|Delete|Schedule/i }).count(),
        ).toBe(0)
        await expect(
          page.getByText(/it never creates, restores, or deletes a backup/, { exact: false }),
        ).toBeVisible()

        // The detail page names the file, its recorded hash, and that no
        // verification outcome exists, with no verdict invented from absence.
        await artifactLink.click()
        await expect(page.getByRole('heading', { level: 1, name: artifact.filename })).toBeVisible()
        await expect(
          page.getByText(String(recorded?.sha256), { exact: true }),
        ).toBeVisible()
        await expect(
          page.getByText('No verification has recorded an outcome for this artifact yet', {
            exact: false,
          }),
        ).toBeVisible()
        await expect(
          page.getByText(/nothing here says the file is readable/, { exact: false }),
        ).toBeVisible()
        await expect(page.getByText(/It is not a restore rehearsal/, { exact: false })).toBeVisible()

        // An unknown id is named as unknown instead of rendering an empty page.
        await gotoAuthenticated(
          page,
          server.baseUrl,
          '/admin/backups/0195f2a1-9999-4999-8999-000000009999',
        )
        await expect(
          page.getByRole('heading', { level: 1, name: 'Backup artifact not found', exact: true }),
        ).toBeVisible()
        await expect(page.getByRole('link', { name: 'Back to Backups' })).toBeVisible()

        // Requesting verification is only an acceptance: the recorded state on
        // the page stays the one the Server last wrote.
        await gotoAuthenticated(page, server.baseUrl, '/admin/backups/' + artifact.artifactId)
        await page.getByRole('button', { name: 'Request verification', exact: true }).click()
        await expect(page.getByText(/Acceptance is not a result/, { exact: false })).toBeVisible()
        await expect(page.getByText('Not verified').first()).toBeVisible()
        const queued = (await readArtifacts(server))[0]
        expect(queued?.verification).toBe('pending')

        // The queued task is the Server's: the outcome the page later shows is
        // the one that task recorded, and the linkage is recorded too.
        const verified = await waitForVerification(server, artifact.artifactId, 'ok')
        expect(String(verified.artifact.verifiedAt ?? '').length).toBeGreaterThan(0)
        const verifyOperationId = String(verified.artifact.verifyOperationId ?? '')
        expect(verifyOperationId.length).toBeGreaterThan(0)

        // The page left open has to reconcile by itself: it reads the task's
        // own recorded status, re-reads the artifact, and only then claims the
        // outcome on screen is the one that task wrote. No reload in between,
        // so a page that never updated would fail here.
        await expect(page.getByText('Verified').first()).toBeVisible({ timeout: 60_000 })
        await expect(
          page.getByText(/every check it runs passed/, { exact: false }),
        ).toBeVisible()
        await expect(
          page.getByText(/recorded the outcome shown above/, { exact: false }),
        ).toBeVisible()
        await expect(
          page.locator('a[href="/admin/operations/' + verifyOperationId + '"]'),
        ).toHaveCount(1)
        expect((await readArtifact(server, artifact.artifactId)).artifact.verification).toBe('ok')

        // The recorded state survives a reload because it is Server state, and
        // the list and the detail page agree about it.
        await page.reload()
        await expect(page.getByText('Verified').first()).toBeVisible()
        await expect(
          page.getByText(/every check it runs passed/, { exact: false }),
        ).toBeVisible()
        await gotoAuthenticated(page, server.baseUrl, '/admin/backups')
        await expect(table.getByText('Verified')).toBeVisible()

        // A verification task is an ordinary retained task: its own page shows
        // the recorded outcome, and a finished task is not cancellable.
        await gotoAuthenticated(
          page,
          server.baseUrl,
          '/admin/operations/' + verifyOperationId,
        )
        await expect(
          page.getByRole('heading', { level: 1, name: 'Operation detail', exact: true }),
        ).toBeVisible()
        await expect(page.getByText('Succeeded').first()).toBeVisible()
        await expect(page.getByRole('button', { name: 'Cancel task' })).toBeDisabled()

        // Owner-only: an anonymous visitor reaches the Admin boundary, never a
        // sanitized artifact list (the recorded file name stays unrendered).
        await page.context().clearCookies()
        await page.goto(server.baseUrl + '/admin/backups')
        await expect(page.locator('h1').first()).toHaveText(/Owner access required/i)
        await expect(page.locator('table[data-slot="backups-table"]')).toHaveCount(0)
        await expect(page.getByText(artifact.filename)).toHaveCount(0)
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('a corrupted file and a missing file are distinct recorded failures', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the failure flow runs once')
    test.setTimeout(900_000)
    const server = await startDisposableServer()
    try {
      const context = await browser.newContext()
      const page = await context.newPage()
      try {
        const corrupted = await server.createOfflineBackup()
        const removed = await server.createOfflineBackup()
        expect(removed.artifactId).not.toBe(corrupted.artifactId)
        expect(await readArtifacts(server)).toHaveLength(2)

        // Two real-world failures an operator finds on the Server host: the
        // bytes changed after creation, and the file is gone.
        const { writeFileSync, unlinkSync, statSync } = await import('node:fs')
        writeFileSync(corrupted.artifactPath, 'tampered', { encoding: 'utf8' })
        expect(statSync(corrupted.artifactPath).size).toBe(8)
        unlinkSync(removed.artifactPath)

        await loginToDisposableServer(page, server.baseUrl)
        await gotoAuthenticated(page, server.baseUrl, '/admin/backups/' + corrupted.artifactId)
        await page.getByRole('button', { name: 'Request verification', exact: true }).click()
        await expect(page.getByText(/Acceptance is not a result/, { exact: false })).toBeVisible()
        const changed = await waitForVerification(server, corrupted.artifactId, 'failed')
        expect(String(changed.verificationError ?? '')).toContain('checksum')

        // Read the failure the Server recorded on the page that is already
        // open: the task ends as `failed` after writing this artifact outcome,
        // so the card says the artifact record holds the failure it recorded
        // rather than claiming it failed before writing anything.
        await expect(page.getByText('Verification failed').first()).toBeVisible({ timeout: 60_000 })
        await expect(
          page.getByText(/no longer matches the checksum recorded when the artifact was created/, {
            exact: false,
          }),
        ).toBeVisible()
        await expect(page.locator('[data-slot="backup-verification-reason"]')).toHaveText(
          String(changed.verificationError),
        )
        await expect(
          page.getByText(/holds the failure it recorded/, { exact: false }),
        ).toBeVisible()
        await expect(page.getByText('Verified')).toHaveCount(0)

        await page.reload()
        await expect(page.getByText('Verification failed').first()).toBeVisible()

        await gotoAuthenticated(page, server.baseUrl, '/admin/backups/' + removed.artifactId)
        await page.getByRole('button', { name: 'Request verification', exact: true }).click()
        await expect(page.getByText(/Acceptance is not a result/, { exact: false })).toBeVisible()
        const missing = await waitForVerification(server, removed.artifactId, 'failed')
        expect(String(missing.verificationError ?? '')).toContain('cannot open')

        await expect(page.getByText('Verification failed').first()).toBeVisible({ timeout: 60_000 })
        await expect(
          page.getByText(/The Server could not open the file, so none of its contents were checked/, {
            exact: false,
          }),
        ).toBeVisible()

        await page.reload()
        await expect(page.getByText('Verification failed').first()).toBeVisible()

        // Neither failure deleted the recorded artifact: a failed verification
        // never rewrites the ledger, and the surface offers no delete control.
        const stillRecorded = await readArtifacts(server)
        expect(stillRecorded).toHaveLength(2)
        expect(
          stillRecorded.map((entry) => entry.artifactId).sort(),
        ).toEqual([corrupted.artifactId, removed.artifactId].sort())
        expect(await page.getByRole('button', { name: /Delete|Remove|Restore/i }).count()).toBe(0)
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('the backup surface holds across the viewport, theme, keyboard, and touch matrix', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the matrix runs once')
    test.setTimeout(900_000)
    const server = await startDisposableServer()
    try {
      const artifact = await server.createOfflineBackup()
      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        const detailPath = '/admin/backups/' + artifact.artifactId

        for (const viewport of VIEWPORTS) {
          await page.setViewportSize(viewport)

          await page.emulateMedia({ colorScheme: 'light' })
          await gotoAuthenticated(page, server.baseUrl, '/admin/backups')
          await expectResolvedTheme(page, 'light')
          await expectBackupsLayout(page, viewport.width)
          await page.emulateMedia({ colorScheme: 'dark' })
          await expectResolvedTheme(page, 'dark')
          await expectBackupsLayout(page, viewport.width)

          await page.emulateMedia({ colorScheme: 'light' })
          await gotoAuthenticated(page, server.baseUrl, detailPath)
          await expectResolvedTheme(page, 'light')
          await expectArtifactLayout(page, artifact.filename, viewport.width)
          await page.emulateMedia({ colorScheme: 'dark' })
          await expectResolvedTheme(page, 'dark')
          await expectArtifactLayout(page, artifact.filename, viewport.width)
        }

        // The one control on the surface is keyboard reachable and visibly
        // focused in both themes.
        await page.setViewportSize(VIEWPORTS[3])
        for (const colorScheme of ['light', 'dark'] as const) {
          await page.emulateMedia({ colorScheme })
          await gotoAuthenticated(page, server.baseUrl, detailPath)
          await expectResolvedTheme(page, colorScheme)
          await focusByKeyboard(
            page,
            page.getByRole('button', { name: 'Request verification', exact: true }),
          )
          await expectFocusedElementHasVisibleFocus(page)
        }

        // A touch tap reaches the same command at phone width, and the recorded
        // state still only moves when the Server records an outcome.
        await page.setViewportSize(VIEWPORTS[0])
        await gotoAuthenticated(page, server.baseUrl, detailPath)
        await page.getByRole('button', { name: 'Request verification', exact: true }).tap()
        await expect(page.getByText(/Acceptance is not a result/, { exact: false })).toBeVisible()
        await waitForVerification(server, artifact.artifactId, 'ok')
        // The touch-driven request reconciles on the open page too, and then
        // the same recorded state is what a fresh load shows.
        await expect(page.getByText('Verified').first()).toBeVisible({ timeout: 60_000 })
        await page.reload()
        await expect(page.getByText('Verified').first()).toBeVisible()
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })
})
