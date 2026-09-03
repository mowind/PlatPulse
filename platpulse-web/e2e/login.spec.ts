import { expect, test } from '@playwright/test'
import {
  E2E_PASSWORD,
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow,
  loginAs,
  setPageZoom,
} from './helpers'

test.describe('Private Home login', () => {
  test('unauthenticated visits are guided to the login page', async ({ page }) => {
    await page.goto('/')
    await expect(page).toHaveURL(/\/login$/)
    await expect(
      page.getByRole('heading', { level: 1, name: 'Sign in to PlatPulse' }),
    ).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('a wrong password shows a readable error', async ({ page }) => {
    await page.goto('/login')
    await page.getByLabel('Username').fill('admin')
    await page.getByLabel('Password').fill('not-the-password')
    await page.getByRole('button', { name: 'Sign in' }).click()
    await expect(page.getByRole('alert')).toContainText('Invalid username or password')
    await expect(
      page.getByRole('heading', { level: 1, name: 'Sign in to PlatPulse' }),
    ).toBeVisible()
  })

  test('an Owner can sign in, enter Home and Admin, and sign out', async ({ page }) => {
    await loginAs(page)

    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page).toHaveURL(/\/admin$/)
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
    await expectNoHorizontalOverflow(page)

    await page.getByRole('link', { name: 'Home', exact: true }).click()
    await expect(page).toHaveURL(/\/$/)
    await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()

    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await page.getByRole('button', { name: 'Sign out' }).click()
    await expect(page).toHaveURL(/\/login$/)
    await expect(
      page.getByRole('heading', { level: 1, name: 'Sign in to PlatPulse' }),
    ).toBeVisible()
  })

  test('the form is fully keyboard-operable with a visible focus ring', async ({ page }) => {
    await page.goto('/login')
    await expect(page.getByLabel('Username')).toBeFocused()

    // Tab moves username → password with a visible focus indicator.
    await page.keyboard.press('Tab')
    await expect(page.getByLabel('Password')).toBeFocused()
    await expectFocusedElementHasVisibleFocus(page)

    // Enter submits the form.
    await page.getByLabel('Username').fill('admin')
    await page.getByLabel('Password').fill(E2E_PASSWORD)
    await page.keyboard.press('Enter')
    await expect(page).toHaveURL(/\/$/)
    await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
  })

  test('login, Home, and Admin stay operable at 200% zoom', async ({ page }) => {
    await setPageZoom(page, 2)

    await page.goto('/login')
    await expect(
      page.getByRole('heading', { level: 1, name: 'Sign in to PlatPulse' }),
    ).toBeVisible()
    await expectNoHorizontalOverflow(page)

    await page.getByLabel('Username').fill('admin')
    await page.getByLabel('Password').fill(E2E_PASSWORD)
    await page.getByRole('button', { name: 'Sign in' }).click()
    await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
    await expectNoHorizontalOverflow(page)

    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })
})
