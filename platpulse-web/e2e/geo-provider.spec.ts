import { expect, test } from '@playwright/test'
import {
  E2E_VIEWER_PASSWORD,
  E2E_VIEWER_USERNAME,
  expectNoHorizontalOverflow,
  loginAs,
} from './helpers'

/**
 * Owner Geo provider selection and the background country resolution it
 * schedules (issue #132). The Server, the operator-provided MMDB, the
 * background path, and the Public projection are all real; only the Peer
 * fixture is seeded. The spec normalizes the provider to Disabled first, so
 * it stays independent of an earlier run, and restores Disabled afterwards.
 */
test.describe('Geo provider selection and background country resolution', () => {
  test('Owner enables Local MMDB and the open Public country list follows the background lookup', async ({ page }) => {
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
    // No provider this Server does not implement is offered, and the privacy
    // consequence is explicit (issues #132 and #134). IPinfo is implemented,
    // so it is offered with its third-party consequence; this spec never
    // selects it, so no Peer address ever leaves the test Server.
    await expect(geoCard.getByRole('radio', { name: /GeoJS/ })).toHaveCount(0)
    const ipinfo = geoCard.getByRole('radio', { name: 'IPinfo' })
    await expect(ipinfo).toBeVisible()
    await expect(ipinfo).not.toBeChecked()
    await expect(geoCard.getByText(/Sends observed Peer public IPs to a third party/)).toBeVisible()
    await expect(geoCard.getByText(/ipinfo\.io\/\{ip\}\/json/)).toBeVisible()
    await expect(geoCard.getByText(/Peer addresses never leave the Server/)).toBeVisible()
    await expect(geoCard.getByText('Not scheduled')).toBeVisible()
    await expect(geoCard.getByText('Configured')).toBeVisible()

    const publicPage = await page.context().newPage()
    try {
      await publicPage.goto('/networks/platon-e2e')
      await expect(
        publicPage.getByRole('heading', { level: 1, name: 'PlatON E2E Network' }),
      ).toBeVisible()
      // Disabled renders the neutral notice and no country panel at all.
      await expect(
        publicPage.getByText('Peer countries · Disabled by server', { exact: true }),
      ).toBeVisible()
      await expect(publicPage.getByRole('region', { name: 'Peer countries' })).toHaveCount(0)

      await local.check()
      await save.click()
      await expect(geoCard.getByText(/Geo provider is now Local MMDB/)).toBeVisible()
      await expect(geoCard.getByText(/Geo provider is now Local MMDB/)).toHaveCount(1)
      // Exactly one provider is selected, and the background path has work.
      await expect(local).toBeChecked()
      await expect(publicPage.getByText('89.160.20.112')).toHaveCount(0)
      await expect(page.getByText('89.160.20.112')).toHaveCount(0)
      await expect(page.getByText(/GeoLite2-Country-Test/)).toHaveCount(0)

      // The Server resolves the retained Peer addresses in the background and
      // publishes an invalidation; the already-open Public page follows it
      // without a reload. Peer records are counted per Node, never
      // deduplicated by address: two records have no usable public remote IP
      // and stay Unknown, one is resolved to SE.
      const countries = publicPage.getByRole('region', { name: 'Peer countries' })
      await expect(countries).toBeVisible({ timeout: 30_000 })
      await expect(countries).toContainText('Known 1 · Unknown 2', { timeout: 30_000 })
      await expect(
        countries.getByRole('list', { name: 'Peer countries by count' }),
      ).toContainText('SE')
      await expect(countries).toContainText(
        '3 Peer records in scope; counted per Node, not deduplicated by IP.',
      )
      await expect(countries).toContainText('Partial scope')
      await expect(countries).toContainText(/GeoLite Data created by MaxMind/)
      await expect(countries).toContainText('2 without a usable public remote IP')
      // The bundled fixture's build epoch is older than the 30-day database
      // boundary, so the Server reports the database as Stale while still
      // serving the retained country result as last-good.
      await expect(countries).toContainText('Geo database is Stale')
      // No raw Peer address, database path, or internal error crosses the
      // Public or Admin boundary.
      await expect(publicPage.getByText('89.160.20.112')).toHaveCount(0)
      await expect(publicPage.getByText(/GeoLite2-Country-Test/)).toHaveCount(0)
      await expect(page.getByText('89.160.20.112')).toHaveCount(0)
      await expect(page.getByText(/GeoLite2-Country-Test/)).toHaveCount(0)
      await expectNoHorizontalOverflow(publicPage)
      await expectNoHorizontalOverflow(page)
    } finally {
      // Disabling stops scheduling and returns the neutral notice on the
      // already-open Public page.
      await disabled.check()
      await save.click()
      await expect(
        publicPage.getByText('Peer countries · Disabled by server', { exact: true }),
      ).toBeVisible({ timeout: 30_000 })
      await expect(publicPage.getByRole('region', { name: 'Peer countries' })).toHaveCount(0)
      await publicPage.close()
    }
  })

  test('a Viewer cannot reach the Geo configuration surface or change it through the API', async ({ page }) => {
    await loginAs(page, E2E_VIEWER_USERNAME, E2E_VIEWER_PASSWORD)
    await page.goto('/admin/settings')
    await expect(
      page.getByRole('heading', { level: 1, name: 'Owner access required' }),
    ).toBeVisible()

    // The Server, not the browser, enforces the boundary: the Geo status and
    // the provider mutation are Owner-only Admin routes.
    const status = await page.request.get('/api/admin/v1/geo')
    expect(status.status()).toBe(403)
    expect((await status.json()).error.code).toBe('owner_required')
    const mutation = await page.request.put('/api/admin/v1/geo/provider', {
      data: { provider: 'local_mmdb' },
    })
    expect(mutation.status()).toBe(403)
    await expectNoHorizontalOverflow(page)
  })
})
