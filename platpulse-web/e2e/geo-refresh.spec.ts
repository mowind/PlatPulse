import { expect, test } from '@playwright/test'
import {
  E2E_VIEWER_PASSWORD,
  E2E_VIEWER_USERNAME,
  expectNoHorizontalOverflow,
  loginAs,
} from './helpers'

/**
 * Owner-triggered global Peer geolocation refresh (issue #136). The Server,
 * the operator-provided MMDB, the background path, the retained cache, and
 * the Public projection are all real; only the Peer fixture is seeded. The
 * spec normalizes the provider to Disabled first so it stays independent of
 * an earlier run, and restores Disabled afterwards.
 */
test.describe('Owner global Geo refresh', () => {
  test('forces a real re-query, reports per-address progress, and updates Public countries', async ({ page }) => {
    await loginAs(page)
    await page.goto('/admin/settings')
    const geoCard = page
      .getByRole('article')
      .filter({ has: page.getByRole('heading', { name: 'Geo provider' }) })
    const save = geoCard.getByRole('button', { name: 'Save Geo provider' })
    const disabled = geoCard.getByRole('radio', { name: 'Disabled' })
    const local = geoCard.getByRole('radio', { name: 'Local MMDB' })

    // Normalize to Disabled: the harness seeds it, but a failed earlier run
    // in the same reused Server must not change this spec's outcome.
    if (!(await disabled.isChecked())) {
      await disabled.check()
      await save.click()
      await expect(geoCard.getByText(/Geo provider is now Disabled/)).toBeVisible()
    }

    // Disabled Geo is explicit and never fakes a refresh: the Server owns the
    // reason and the control cannot be submitted (issue #136). A previous run
    // from an earlier spec in the same reused Server may still be shown as a
    // terminal result, so this asserts what Disabled really means - no run is
    // in progress, and none can be started.
    const refresh = geoCard.getByRole('button', { name: 'Refresh Peer geolocation' })
    await expect(refresh).toBeDisabled()
    await expect(
      geoCard.getByText(/Geo is Disabled, so no Peer address is resolved/),
    ).toBeVisible()
    await expect(geoCard.getByText(/Refreshing:/)).toHaveCount(0)

    const publicPage = await page.context().newPage()
    try {
      await publicPage.goto('/networks/platon-e2e')
      await expect(
        publicPage.getByRole('heading', { level: 1, name: 'PlatON E2E Network' }),
      ).toBeVisible()

      await local.check()
      await save.click()
      await expect(geoCard.getByText(/Geo provider is now Local MMDB/)).toBeVisible()

      // The Owner forces the global re-query. The Server really re-resolves
      // every eligible public Peer address the current Networks reference,
      // even though the automatic path already retained a valid result, and
      // it never needs a new Agent report to do it.
      await expect(refresh).toBeEnabled()
      await expect(
        geoCard.getByText(/Geo is Disabled, so no Peer address is resolved/),
      ).toHaveCount(0)
      await refresh.click()
      await expect(
        geoCard.getByText(/Refresh complete: 1 resolved, 0 without a country, 0 failed/),
      ).toBeVisible({ timeout: 30_000 })
      // The counts are IP lookups; the Peer records those addresses serve are
      // stated separately, because the country map counts Peer records.
      await expect(geoCard.getByText('1 of 1 complete')).toBeVisible()
      await expect(geoCard.getByText('Peer records referencing them').locator('..')).toContainText('1')
      await expect(geoCard.getByText(/One lookup per distinct IP address/)).toBeVisible()

      // The forced run wrote a real result: the retained success time belongs
      // to the run window, and the Public country list follows the refresh
      // without a reload.
      const status = await (await page.request.get('/api/admin/v1/geo')).json()
      expect(status.refresh.state).toBe('completed')
      expect(status.refresh.resolved_lookups).toBe(1)
      expect(status.refresh.peer_records_in_scope).toBe(1)
      expect(status.last_success_at >= status.refresh.started_at).toBe(true)

      const countries = publicPage.getByRole('region', { name: 'Peer countries' })
      await expect(countries).toContainText('SE', { timeout: 30_000 })
      await expect(countries).toContainText('Known 1 · Unknown 2')

      // Neither surface leaks the raw Peer address or the database path.
      await expect(page.getByText('89.160.20.112')).toHaveCount(0)
      await expect(publicPage.getByText('89.160.20.112')).toHaveCount(0)
      await expect(page.getByText(/GeoLite2-Country-Test/)).toHaveCount(0)
      await expectNoHorizontalOverflow(page)
      await expectNoHorizontalOverflow(publicPage)
    } finally {
      // Disabling stops scheduling and returns the neutral notice on the
      // already-open Public page.
      await disabled.check()
      await save.click()
      await expect(
        publicPage.getByText('Peer countries · Disabled by server', { exact: true }),
      ).toBeVisible({ timeout: 30_000 })
      await publicPage.close()
    }
  })

  test('a Viewer can neither see nor trigger the global refresh', async ({ page }) => {
    await loginAs(page, E2E_VIEWER_USERNAME, E2E_VIEWER_PASSWORD)
    await page.goto('/admin/settings')
    await expect(
      page.getByRole('heading', { level: 1, name: 'Owner access required' }),
    ).toBeVisible()

    // The Server, not the browser, enforces the boundary: the refresh route is
    // Owner-only, so a Viewer request never starts a run.
    const response = await page.request.post('/api/admin/v1/geo/refresh')
    expect(response.status()).toBe(403)
    expect((await response.json()).error.code).toBe('owner_required')
    await expectNoHorizontalOverflow(page)
  })
})
