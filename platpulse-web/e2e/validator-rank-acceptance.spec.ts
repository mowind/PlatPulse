import { expect, test } from '@playwright/test'
import type { AdminValidatorInsight, Validator, ValidatorDetail } from '../src/api/generated'
import { formatObservedAt } from '../src/components/StatusBadge'
import { VIEWPORTS, expectResolvedTheme, gotoAuthenticated, loginToDisposableServer } from './admin-flow'
import { expectNoHorizontalOverflow } from './helpers'
import { PLATSCAN_CAPTURE_NODE_ID, startPlatscanReplay } from './platscan-replay'
import { startDisposableServer, type DisposableServer } from './server-harness'

const VALIDATOR_ID = 'rank-refresh-validator'
const DETAIL_PATH = '/api/admin/v1/validators/' + VALIDATOR_ID

async function insight(server: DisposableServer): Promise<AdminValidatorInsight | null | undefined> {
  const detail = await server.expectAdminGet(DETAIL_PATH, 200) as ValidatorDetail
  return detail.insight
}

/** Real ranking failure with successful detail, followed by detail-only failure
 * with successful ranking. Chronological metric freshness never hides the latest
 * failed detail outcome or changes the independently refreshed ranking. */
test('rank and detail refresh failures remain independently explicit in every Admin viewport and theme', async ({ browser }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-1280', 'the matrix flow runs once per suite')
  test.setTimeout(300_000)
  const replay = await startPlatscanReplay()
  const now = new Date().toISOString().replace(/\.\d{3}Z$/, 'Z')
  const server = await startDisposableServer({
    seedSql: "INSERT INTO validators (validator_id, network_key, validator_node_id, created_at, updated_at) VALUES ('" + VALIDATOR_ID + "', 'platon-mainnet', '" + PLATSCAN_CAPTURE_NODE_ID + "', '" + now + "', '" + now + "');",
    validatorProvider: {
      timezone: 'UTC',
      deployments: { 'platon-mainnet': replay.baseUrl },
      refreshSeconds: 5,
    },
  })
  try {
    await expect.poll(async () => (await insight(server))?.rankOutcome, { timeout: 30_000 }).toBe('success')
    const good = await insight(server)
    expect(good?.rank).toBe(1)
    expect(good?.rankFreshness).toBe('fresh')
    expect(good?.rankLastGoodReceivedAt).toBeTruthy()
    expect(replay.rankingPages.slice(0, 5)).toEqual([1, 2, 3, 4, 5])

    replay.setRankingFailure(true)
    await expect.poll(async () => (await insight(server))?.rankOutcome, { timeout: 30_000 }).toBe('error')
    const failed = await insight(server)
    expect(failed?.outcome).toBe('success')
    expect(failed?.freshness).toBe('fresh')
    expect(failed?.rank).toBe(1)
    expect(failed?.rankState).toBe('error')
    expect(failed?.rankFreshness).toBe('stale')
    expect(failed?.rankLastGoodReceivedAt).toBe(good?.rankLastGoodReceivedAt)
    expect(failed?.lastGoodReceivedAt).not.toBe(good?.rankLastGoodReceivedAt) // Detail advanced; ranking did not.
    expect(failed?.rankLastGoodAgeSeconds).toBeGreaterThanOrEqual(0)
    expect(failed?.rankDiagnostic).toBeTruthy()
    const rows = await server.expectAdminGet('/api/admin/v1/validators', 200) as Validator[]
    expect(rows.find((row) => row.validatorId === VALIDATOR_ID)?.insight?.rankFreshness).toBe('stale')

    const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
    try {
      const page = await context.newPage()
      await loginToDisposableServer(page, server.baseUrl)
      for (const viewport of VIEWPORTS) {
        for (const colorScheme of ['light', 'dark'] as const) {
          await page.setViewportSize(viewport)
          await page.emulateMedia({ colorScheme })
          await gotoAuthenticated(page, server.baseUrl, '/admin/validators')
          await expectResolvedTheme(page, colorScheme)
          const rank = page.locator('[data-slot="validator-table"] tbody tr').first().locator('td[data-label="Rank"]')
          await expect(rank).toContainText('#1')
          await expect(rank).toContainText('Retained rank (stale)')
          await expect(rank).toContainText('Rank refresh error')
          await expect(rank).toContainText('Rank confirmed at ' + formatObservedAt(good?.rankLastGoodReceivedAt ?? ''))
          await expect(rank).not.toContainText('Rank never observed')
          await expect(rank).not.toContainText('Fresh rank')
          await expect(rank).not.toContainText('Unranked')
          const metrics = page.locator('[data-slot="validator-table"] tbody tr').first().locator('td[data-label="Metrics evidence"]')
          await expect(metrics).toContainText('Fresh metrics')
          await expect(metrics).toContainText('Last-good metric age')
          await expect(metrics).toContainText('Last-good metric received at')
          await expect(metrics).not.toContainText(formatObservedAt(good?.rankLastGoodReceivedAt ?? ''))
          await expectNoHorizontalOverflow(page)

          await gotoAuthenticated(page, server.baseUrl, '/admin/validators/' + VALIDATOR_ID)
          await expectResolvedTheme(page, colorScheme)
          const rankPanel = page.locator('[data-slot="card-x"]').filter({ has: page.getByRole('heading', { name: 'Rank evidence', exact: true }) })
          await expect(rankPanel).toContainText('#1')
          await expect(rankPanel).toContainText('Retained rank (stale)')
          await expect(rankPanel).toContainText('Rank refresh error')
          await expect(rankPanel).toContainText('Rank attempted at')
          await expect(rankPanel).toContainText('Rank last-good age')
          await expect(rankPanel).toContainText(formatObservedAt(good?.rankLastGoodReceivedAt ?? ''))
          const providerPanel = page.locator('[data-slot="card-x"]').filter({ has: page.getByRole('heading', { name: 'Provider evidence', exact: true }) })
          await expect(providerPanel).toContainText('Fresh')
          await expect(providerPanel).toContainText('success')
          await expectNoHorizontalOverflow(page)
        }
      }

      // Recover ranking first so the next phase isolates a detail-only failure.
      // The fresh success receipt is captured immediately after that real loop.
      replay.setRankingFailure(false)
      await expect.poll(async () => {
        const current = await insight(server)
        return [current?.outcome, current?.rankOutcome]
      }, { timeout: 30_000 }).toEqual(['success', 'success'])
      const detailGood = await insight(server)
      expect(detailGood?.lastGoodReceivedAt).toBeTruthy()
      replay.setDetailFailure(true)
      await expect.poll(async () => {
        const current = await insight(server)
        return [current?.outcome, current?.rankOutcome]
      }, { timeout: 30_000 }).toEqual(['error', 'success'])
      const detailFailed = await insight(server)
      expect(detailFailed?.freshness).toBe('fresh')
      expect(detailFailed?.lastGoodReceivedAt).toBe(detailGood?.lastGoodReceivedAt)
      expect(detailFailed?.stakeAmount).toBe(detailGood?.stakeAmount)
      expect(detailFailed?.diagnostic).toBeTruthy()
      expect(detailFailed?.rankFreshness).toBe('fresh')
      expect(detailFailed?.rankLastGoodReceivedAt).not.toBe(detailFailed?.lastGoodReceivedAt)
      const detailFailureRows = await server.expectAdminGet('/api/admin/v1/validators', 200) as Validator[]
      expect(detailFailureRows.find((row) => row.validatorId === VALIDATOR_ID)?.insight?.outcome).toBe('error')

      for (const viewport of VIEWPORTS) {
        for (const colorScheme of ['light', 'dark'] as const) {
          await page.setViewportSize(viewport)
          await page.emulateMedia({ colorScheme })
          await gotoAuthenticated(page, server.baseUrl, '/admin/validators')
          await expectResolvedTheme(page, colorScheme)
          const row = page.locator('[data-slot="validator-table"] tbody tr').first()
          const metrics = row.locator('td[data-label="Metrics evidence"]')
          await expect(metrics).toContainText('Fresh metrics')
          await expect(metrics).toContainText('Latest detail refresh error')
          await expect(metrics).toContainText('Retained last-good metrics')
          await expect(metrics).toContainText(detailFailed?.diagnostic ?? '')
          await expect(metrics).toContainText(formatObservedAt(detailGood?.lastGoodReceivedAt ?? ''))
          await expect(metrics).not.toContainText('Retained metrics (stale)')
          const rank = row.locator('td[data-label="Rank"]')
          await expect(rank).toContainText('Fresh rank')
          await expect(rank).toContainText('Rank refresh success')
          await expect(rank).not.toContainText('Rank refresh error')
          await expectNoHorizontalOverflow(page)
        }
      }
    } finally {
      await context.close()
    }
  } finally {
    await server.dispose()
    await replay.stop()
  }
})
