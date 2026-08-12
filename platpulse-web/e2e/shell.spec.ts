import { expect, test, type Page } from '@playwright/test'
import {
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow,
  loginAs,
} from './helpers'

async function expectShellFitsViewport(page: Page, heading: string) {
  await expect(page.getByRole('heading', { level: 1, name: heading })).toBeVisible()
  await expectNoHorizontalOverflow(page)
}

test.describe('Authenticated shell', () => {
  test('Home shell fits the viewport without horizontal overflow', async ({ page }) => {
    await loginAs(page)
    await expectShellFitsViewport(page, 'Home')
  })

  test('Admin shell fits the viewport without horizontal overflow', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expectShellFitsViewport(page, 'Admin')
  })

  test('Home and Admin navigation is reachable in both directions', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page).toHaveURL(/\/admin$/)
    await expectShellFitsViewport(page, 'Admin')

    await page.getByRole('link', { name: 'Home', exact: true }).click()
    await expect(page).toHaveURL(/\/$/)
    await expectShellFitsViewport(page, 'Home')
  })

  test('shell navigation is keyboard-operable with a visible focus ring', async ({ page }) => {
    await loginAs(page)

    // Tab from the brand to the Home link (focus order: brand, Home,
    // Admin, Sign out) and verify the focus ring is visible.
    await page.keyboard.press('Tab')
    await expect(page.getByRole('link', { name: 'PlatPulse' })).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(page.getByRole('link', { name: 'Home', exact: true })).toBeFocused()
    await expectFocusedElementHasVisibleFocus(page)

    // Enter activates the focused link without a pointer.
    await page.keyboard.press('Enter')
    await expect(page.getByRole('heading', { level: 1, name: 'Home' })).toBeVisible()

    await page.keyboard.press('Tab')
    await expect(page.getByRole('link', { name: 'Admin', exact: true })).toBeFocused()
    await expectFocusedElementHasVisibleFocus(page)
    await page.keyboard.press('Enter')
    await expect(page).toHaveURL(/\/admin$/)
    await expect(page.getByRole('heading', { level: 1, name: 'Admin' })).toBeVisible()

    // Navigation remounts the layout, so find Sign out by tabbing around
    // the (small, wrapping) header focus order.
    const signOut = page.getByRole('button', { name: 'Sign out' })
    for (let tab = 0; tab < 6; tab += 1) {
      await page.keyboard.press('Tab')
      if (await signOut.evaluate((element) => element === document.activeElement)) {
        break
      }
    }
    await expect(signOut).toBeFocused()
    await expectFocusedElementHasVisibleFocus(page)
    await page.keyboard.press('Enter')
    await expect(page).toHaveURL(/\/login$/)
  })
})
