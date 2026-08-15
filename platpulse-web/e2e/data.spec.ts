import { readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { expect, test, type Page } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

/**
 * Owner Data and Operations surfaces (issue #50, webui.md §4.4/§4.5/§5.5/
 * §8.4): retention with safety bounds and typed confirmation, sanitized
 * backup metadata with verification, read-only Doctor, and durable
 * Operations recoverable through REST. Mutations share one Server, so
 * every mutation runs once on desktop-1280 only (same convention as
 * access.spec.ts / agent-lifecycle.spec.ts); the other projects verify the
 * read-only surfaces and responsive behavior.
 */

async function openAdmin(page: Page, section: string) {
  await loginAs(page)
  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible({
    timeout: 15_000,
  })
  const menu = page.getByRole('button', { name: 'Menu' })
  const usesDrawer = await menu.isVisible()
  if (usesDrawer) await menu.click()
  await page.getByRole('link', { name: section }).click()
  if (usesDrawer) {
    await expect(page.getByRole('navigation', { name: 'Admin' })).not.toBeInViewport()
  }
}

test.describe('Read-only Data surfaces (all viewports)', () => {
  test('Operations history renders the fixed vocabulary without overflow', async ({ page }) => {
    await openAdmin(page, 'Operations')
    await expect(page.getByRole('heading', { level: 1, name: 'Operations' })).toBeVisible({
      timeout: 15_000,
    })
    await expect(page.getByRole('columnheader', { name: 'Status' })).toBeVisible()
    await expect(page.getByRole('columnheader', { name: 'Progress' })).toBeVisible()
    await expect(page.getByRole('columnheader', { name: 'Request ID' })).toBeVisible()
    await expect(page.getByText(/never cancels an Operation/)).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('Retention policies show fixed safety bounds and protected state', async ({ page }) => {
    await openAdmin(page, 'Retention')
    await expect(page.getByRole('heading', { level: 1, name: 'Retention' })).toBeVisible({
      timeout: 15_000,
    })
    const raw = page.getByRole('row', { name: /Raw Block Summaries/ })
    await expect(raw).toBeVisible({ timeout: 15_000 })
    await expect(raw).toContainText('7 days')
    await expect(raw).toContainText('1–30 days')
    // Unsupported aggregate families stay visible and honest.
    await expect(page.getByRole('row', { name: /1-Hour Aggregates/ })).toContainText(
      'Unsupported',
    )
    await expect(page.getByRole('heading', { level: 2, name: 'Protected state' })).toBeVisible()
    await expect(page.getByText('historical high-water marks', { exact: true })).toBeVisible()
    await expect(page.getByText('immutable Incident history', { exact: true })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('Backups render sanitized metadata headers', async ({ page }) => {
    await openAdmin(page, 'Backups')
    await expect(page.getByRole('heading', { level: 1, name: 'Backups' })).toBeVisible({
      timeout: 15_000,
    })
    await expect(page.getByRole('columnheader', { name: 'Checksum' })).toBeVisible()
    await expect(page.getByRole('columnheader', { name: 'Verification' })).toBeVisible()
    await expect(page.getByText(/never displayed and there is no download/)).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('Doctor renders as read-only with the Run action', async ({ page }) => {
    await openAdmin(page, 'Doctor')
    await expect(page.getByRole('heading', { level: 1, name: 'Doctor' })).toBeVisible({
      timeout: 15_000,
    })
    await expect(page.getByText(/never auto-fixes/)).toBeVisible()
    await expect(page.getByRole('button', { name: 'Run Doctor' })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('Restore renders prerequisites and the dedicated confirmation flow', async ({ page }) => {
    await openAdmin(page, 'Restore')
    await expect(page.getByRole('heading', { level: 1, name: 'Restore a backup' })).toBeVisible({
      timeout: 15_000,
    })
    await expect(page.getByText(/exclusive stopped-Server condition/i)).toBeVisible()
    await expect(page.getByText(/Secrets are never restored/)).toBeVisible()
    await expect(page.getByText(/Failure preserves the current database/)).toBeVisible()
    // The workflow cannot be triggered from a generic Operation row: the
    // typed confirmation lives only on this dedicated route.
    await expect(page.getByLabel(/Type the backup file name/)).not.toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('Restore confirmation flow works on every viewport (read-only)', async ({ page }) => {
    await openAdmin(page, 'Restore')
    await expect(page.getByRole('heading', { level: 1, name: 'Restore a backup' })).toBeVisible({
      timeout: 15_000,
    })
    const row = page.getByRole('row', { name: /platpulse-e2e-seed-good\.db/ })
    await expect(row).toBeVisible({ timeout: 15_000 })
    await page.getByLabel('platpulse-e2e-seed-good.db').click()
    const start = page.getByRole('button', { name: 'Start Restore' })
    await expect(start).toBeDisabled()
    // Server-computed checksum, integrity, and schema validation.
    await page.getByRole('button', { name: 'Validate this backup' }).click()
    await expect(page.getByText('Pass').first()).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText(/backup 23 \/ Server 23/)).toBeVisible()
    // The typed confirmation must match the backup file name.
    await page.getByLabel(/Type the backup file name/).fill('wrong-name.db')
    await expect(start).toBeDisabled()
    await page.getByLabel(/Type the backup file name/).fill('platpulse-e2e-seed-good.db')
    await expect(start).toBeEnabled()
    await expectNoHorizontalOverflow(page)
  })

  test('Restore failure path is honest on every viewport (read-only)', async ({ page }) => {
    await openAdmin(page, 'Restore')
    await expect(page.getByRole('heading', { level: 1, name: 'Restore a backup' })).toBeVisible({
      timeout: 15_000,
    })
    const row = page.getByRole('row', { name: /platpulse-e2e-seed-tampered\.db/ })
    await expect(row).toBeVisible({ timeout: 15_000 })
    await page.getByLabel('platpulse-e2e-seed-tampered.db').click()
    await page.getByRole('button', { name: 'Validate this backup' }).click()
    await expect(page.getByText(/restore_checksum_mismatch/)).toBeVisible({ timeout: 15_000 })
    // Checks that were never reached are Not checked, never Pass.
    await expect(page.getByText('Not checked').first()).toBeVisible()
    await expect(page.getByRole('button', { name: 'Start Restore' })).toBeDisabled()
    await expectNoHorizontalOverflow(page)
  })
})

test.describe.serial('Data mutations (one run on desktop-1280)', () => {
  test('Doctor run results stay recoverable through REST after a reload', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'mutations run once')
    await openAdmin(page, 'Doctor')
    await page.getByRole('button', { name: 'Run Doctor' }).click()
    // The mutation returns immediately with an Operation reference.
    await expect(page.getByRole('heading', { level: 1, name: 'Doctor' })).toBeVisible({
      timeout: 15_000,
    })
    await expect(page).toHaveURL(/\/admin\/operations\/[0-9a-f-]+$/, { timeout: 15_000 })
    await expect(page.getByRole('heading', { level: 1, name: 'Doctor' })).toBeVisible({
      timeout: 15_000,
    })
    await expect(page.getByText('Succeeded', { exact: false }).first()).toBeVisible({
      timeout: 90_000,
    })
    // The report carries distinct check statuses and never offers fixes.
    await expect(page.getByText(/Database integrity/)).toBeVisible()
    await expect(page.getByText('Pass').first()).toBeVisible()
    // The seeded, never-verified tampered artifact keeps the latest-backup
    // check an honest Warning (it is the newest artifact at seed time).
    await expect(page.getByText('Warning').first()).toBeVisible()
    await expect(page.getByRole('button', { name: /fix/i })).toHaveCount(0)
    // REST recovery: navigate away and back — the Operation is still there.
    await page.getByRole('link', { name: 'Operations', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Operations' })).toBeVisible({
      timeout: 15_000,
    })
    await page.reload()
    await expect(page.getByRole('heading', { level: 1, name: 'Operations' })).toBeVisible({
      timeout: 15_000,
    })
    await expect(page.getByRole('row', { name: /Doctor/ }).first()).toBeVisible({
      timeout: 15_000,
    })
  })

  test('Retention edit shows impact and requires typed confirmation', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'mutations run once')
    await openAdmin(page, 'Retention')
    await page
      .getByRole('row', { name: /Raw Block Summaries/ })
      .getByRole('link', { name: 'Raw Block Summaries' })
      .click()
    await expect(page.getByRole('heading', { level: 1, name: 'Edit Raw Block Summaries' })).toBeVisible({
      timeout: 15_000,
    })
    await expect(page.getByText('1–30 days')).toBeVisible()
    // The Server computes the read-only impact preview.
    await expect(page.getByText(/rows? would be removed/)).toBeVisible({ timeout: 15_000 })
    // Out-of-bounds values are rejected before any confirmation.
    await page.getByLabel('Retention (days)').fill('99')
    await expect(page.getByText(/between 1 and 30 days/)).toBeVisible()
    await page.getByLabel('Retention (days)').fill('14')
    const save = page.getByRole('button', { name: 'Save policy' })
    await expect(save).toBeDisabled()
    await page.getByLabel(/Type the family and value/).fill('raw_block_summary 14')
    await expect(save).toBeEnabled()
    await save.click()
    await expect(page.getByText(/Audit #\d+/)).toBeVisible({ timeout: 15_000 })
  })

  test('Retention runs are cancelable while Running', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'mutations run once')
    await openAdmin(page, 'Retention')
    await page.getByRole('button', { name: 'Run retention now' }).click()
    await expect(page.getByText(/Run retention for every enabled family now\?/)).toBeVisible()
    await page.getByRole('button', { name: 'Yes, run now' }).click()
    await expect(page).toHaveURL(/\/admin\/operations\/[0-9a-f-]+$/, { timeout: 15_000 })
    // The seeded 2000-row fixture keeps the run Running for ~15 seconds.
    await expect(page.getByText('Running').first()).toBeVisible({ timeout: 15_000 })
    await page.getByRole('button', { name: 'Cancel Operation' }).click()
    await expect(page.getByText(/Cancel this Operation\?/)).toBeVisible()
    await page.getByRole('button', { name: 'Yes, cancel' }).click()
    await expect(page.getByText('Cancelled').first()).toBeVisible({ timeout: 30_000 })
    await expect(page.getByText(/Cancellation requested/)).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('Backup creation and verification expose sanitized metadata', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'mutations run once')
    await openAdmin(page, 'Backups')
    await page.getByRole('link', { name: 'Create a backup' }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Create a backup' })).toBeVisible({
      timeout: 15_000,
    })
    const queue = page.getByRole('button', { name: 'Queue backup' })
    await expect(queue).toBeDisabled()
    await page.getByLabel(/Type the confirmation phrase/).fill('create backup')
    await expect(queue).toBeEnabled()
    await queue.click()
    await expect(page).toHaveURL(/\/admin\/operations\/[0-9a-f-]+$/, { timeout: 15_000 })
    await expect(page.getByText('Succeeded').first()).toBeVisible({ timeout: 90_000 })
    // Result summary carries the artifact id; the artifact list shows only
    // sanitized metadata.
    await page.getByRole('link', { name: 'Backups', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Backups' })).toBeVisible({
      timeout: 15_000,
    })
    const row = page.getByRole('row', { name: /platpulse-.*\.db/ }).first()
    await expect(row).toBeVisible({ timeout: 15_000 })
    await expect(row).toContainText(/\d+(\.\d+)? (B|KiB|MiB)/)
    await expect(row).toContainText(/[0-9a-f]{16}…/)
    await expect(row).toContainText('23')
    await row.getByRole('link', { name: /platpulse-.*\.db/ }).click()
    await expect(page.getByRole('heading', { level: 1, name: /platpulse-.*\.db/ })).toBeVisible({
      timeout: 15_000,
    })
    // Verification runs as an Operation and flips the artifact state.
    await page.getByRole('button', { name: 'Verify artifact' }).click()
    await expect(page).toHaveURL(/\/admin\/operations\/[0-9a-f-]+$/, { timeout: 15_000 })
    await expect(page.getByText('Succeeded').first()).toBeVisible({ timeout: 90_000 })
    await page.getByRole('link', { name: 'Backups', exact: true }).click()
    await expect(page.getByText('Verified').first()).toBeVisible({ timeout: 15_000 })

    // Failure preservation: tampering with the artifact on disk must fail
    // verification while the artifact row (and its metadata) stays listed.
    const backupDir = readFileSync('/tmp/platpulse-e2e-backup-dir', 'utf8').trim()
    const artifactRow = page.getByRole('row', { name: /platpulse-.*\.db/ }).first()
    const artifactId = (await artifactRow.getByRole('link').getAttribute('href'))!
      .split('/')
      .pop()!
    writeFileSync(path.join(backupDir, `platpulse-${artifactId}.db`), 'tampered')
    await artifactRow.getByRole('link', { name: /platpulse-.*\.db/ }).click()
    await expect(page.getByRole('heading', { level: 1, name: /platpulse-.*\.db/ })).toBeVisible({
      timeout: 15_000,
    })
    await page.getByRole('button', { name: 'Verify artifact' }).click()
    await expect(page).toHaveURL(/\/admin\/operations\/[0-9a-f-]+$/, { timeout: 15_000 })
    await expect(page.getByText('Failed').first()).toBeVisible({ timeout: 90_000 })
    await expect(page.getByText(/checksum mismatch/).first()).toBeVisible()
    await page.getByRole('link', { name: 'Backups', exact: true }).click()
    await expect(page.getByText('Verification failed').first()).toBeVisible({ timeout: 15_000 })
    await expectNoHorizontalOverflow(page)
  })

  test('Restore is refused while the Server runs and the database stays authoritative', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'mutations run once')
    // A fresh verified backup is required as the restore identity.
    await openAdmin(page, 'Backups')
    await page.getByRole('link', { name: 'Create a backup' }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Create a backup' })).toBeVisible({
      timeout: 15_000,
    })
    await page.getByLabel(/Type the confirmation phrase/).fill('create backup')
    await page.getByRole('button', { name: 'Queue backup' }).click()
    await expect(page.getByText('Succeeded').first()).toBeVisible({ timeout: 90_000 })
    // The dedicated high-risk route is reached through the Admin sidebar,
    // never from a generic Operation row.
    await page.getByRole('link', { name: 'Restore' }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Restore a backup' })).toBeVisible({
      timeout: 15_000,
    })
    const artifactRow = page.getByRole('row', { name: /platpulse-.*\.db/ }).first()
    const filename = (await artifactRow.getByText(/platpulse-.*\.db/).textContent())!
    await page.getByLabel(filename).click()
    const start = page.getByRole('button', { name: 'Start Restore' })
    await expect(start).toBeDisabled()

    // Checksum, integrity, and schema validation are Server-computed and
    // read-only.
    await page.getByRole('button', { name: 'Validate this backup' }).click()
    await expect(page.getByText('Pass').first()).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText(/backup 23 \/ Server 23/)).toBeVisible()
    // A wrong typed confirmation is not enough.
    await page.getByLabel(/Type the backup file name/).fill('wrong-name.db')
    await expect(start).toBeDisabled()
    await page.getByLabel(/Type the backup file name/).fill(filename)
    // Validation is a Server-computed read of the whole artifact; a
    // refetch triggered by concurrent Admin mutations may briefly reset
    // the gate, so allow the state to settle under parallel load.
    await expect(start).toBeEnabled({ timeout: 15_000 })
    await start.click()

    // The running Server refuses before any mutation with the typed
    // stopped-Server failure; the Operation stays recoverable through REST.
    await expect(
      page.getByRole('heading', { name: 'Restore Operation', level: 2, exact: true }),
    ).toBeVisible({
      timeout: 15_000,
    })
    await expect(page.getByText('Failed').first()).toBeVisible({ timeout: 90_000 })
    await expect(page.getByText(/exclusive stopped-Server condition is required/)).toBeVisible()
    await expect(page.getByText(/platpulse-server restore --artifact-id/).first()).toBeVisible()
    // The current database remains authoritative: backups still list the
    // artifact and the Overview still loads.
    await page.getByRole('link', { name: 'Backups', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Backups' })).toBeVisible({
      timeout: 15_000,
    })
    await expect(page.getByRole('row', { name: /platpulse-.*\.db/ }).first()).toBeVisible({
      timeout: 15_000,
    })
    await page.getByRole('link', { name: 'Overview', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible({
      timeout: 15_000,
    })
    await expectNoHorizontalOverflow(page)
  })

  test('A tampered backup fails validation and cannot be restored', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'mutations run once')
    await openAdmin(page, 'Backups')
    await expect(page.getByRole('row', { name: /platpulse-.*\.db/ }).first()).toBeVisible({
      timeout: 15_000,
    })
    const backupDir = readFileSync('/tmp/platpulse-e2e-backup-dir', 'utf8').trim()
    const artifactRow = page.getByRole('row', { name: /platpulse-.*\.db/ }).first()
    const artifactId = (await artifactRow.getByRole('link').getAttribute('href'))!
      .split('/')
      .pop()!
    writeFileSync(path.join(backupDir, `platpulse-${artifactId}.db`), 'tampered')

    await page.getByRole('link', { name: 'Restore' }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Restore a backup' })).toBeVisible({
      timeout: 15_000,
    })
    const artifactRowRestore = page.getByRole('row', { name: /platpulse-.*\.db/ }).first()
    const filename = (await artifactRowRestore.getByText(/platpulse-.*\.db/).textContent())!
    await page.getByLabel(filename).click()
    await page.getByRole('button', { name: 'Validate this backup' }).click()
    await expect(page.getByText(/restore_checksum_mismatch/)).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText(/does not match its recorded manifest/)).toBeVisible()
    await expect(page.getByRole('button', { name: 'Start Restore' })).toBeDisabled()
    await expectNoHorizontalOverflow(page)
  })
})
