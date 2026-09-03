import { expect, test } from '@playwright/test'
import {
  E2E_VIEWER_PASSWORD,
  E2E_VIEWER_USERNAME,
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow,
  loginAs,
} from './helpers'

test.describe('Viewer role boundary', () => {
  test('a Viewer signs in, uses Home, and is not offered the Admin link', async ({ page }) => {
    await loginAs(page, E2E_VIEWER_USERNAME, E2E_VIEWER_PASSWORD)

    await expect(
      page.getByRole('region', { name: 'Home' }),
    ).toBeVisible()
    await expect(page.getByRole('link', { name: 'Admin', exact: true })).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })

  test('a Viewer gets the stable Owner-required panel on /admin', async ({ page }) => {
    await loginAs(page, E2E_VIEWER_USERNAME, E2E_VIEWER_PASSWORD)
    await page.goto('/admin')
    await expect(
      page.getByRole('heading', { level: 1, name: 'Owner access required' }),
    ).toBeVisible()
    await expectNoHorizontalOverflow(page)

    // The Home shell stays reachable from the blocked panel.
    await page.goto('/')
    await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
  })

  test('the Admin API refuses a Viewer session with a stable 403', async ({ page }) => {
    await loginAs(page, E2E_VIEWER_USERNAME, E2E_VIEWER_PASSWORD)

    // The Server, not the browser, enforces the role boundary: every
    // /api/admin/v1/* request answers `owner_required` for a Viewer.
    const response = await page.request.get('/api/admin/v1/sessions')
    expect(response.status()).toBe(403)
    const body = await response.json()
    expect(body.error.code).toBe('owner_required')
    await expectNoHorizontalOverflow(page)
  })

  test('Viewer navigation is keyboard-operable with a visible focus ring', async ({ page }) => {
    await loginAs(page, E2E_VIEWER_USERNAME, E2E_VIEWER_PASSWORD)

    // Focus order for a Viewer starts with the brand and the Home scan controls.
    await page.keyboard.press('Tab')
    await expect(page.getByRole('link', { name: 'PlatPulse' })).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(page.getByRole('button', { name: 'All Networks' })).toBeFocused()
    await expectFocusedElementHasVisibleFocus(page)
    await page.keyboard.press('Enter')
    await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
  })
})
