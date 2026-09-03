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

  test('Home dashboard exposes the public scan controls at every fixed viewport', async ({ page }) => {
    await loginAs(page)
    await expect(page.getByText('Active Nodes', { exact: true })).toBeVisible()
    await expect(page.getByText('Healthy Nodes', { exact: true })).toBeVisible()
    await expect(page.getByText('Attention', { exact: true })).toBeVisible()
    await expect(page.getByText('Networks', { exact: true })).toBeVisible()
    await expect(page.getByRole('group', { name: 'Network filter' })).toBeVisible()
    await expect(page.getByRole('button', { name: 'All Networks' })).toHaveAttribute('aria-pressed', 'true')
    await expect(page.getByRole('combobox', { name: 'Sort' })).toBeVisible()
    await expect(page.getByRole('link', { name: 'Admin', exact: true })).toHaveAttribute('href', '/admin')
    await expectNoHorizontalOverflow(page)
  })

  test('Home controls remain semantic and touch-sized', async ({ page }) => {
    await loginAs(page)
    const home = page.getByRole('region', { name: 'Home' })
    await expect(home.getByRole('button', { name: 'All Networks' })).toHaveAttribute('aria-pressed', 'true')
    await home.getByRole('combobox', { name: 'Sort' }).selectOption('head')

    const undersized = await home.locator('button, a, select').evaluateAll((elements) => elements.flatMap((element) => {
      const rect = element.getBoundingClientRect()
      return rect.width < 44 || rect.height < 44 ? [element.textContent?.trim() || element.getAttribute('aria-label') || element.tagName] : []
    }))
    expect(undersized, 'Home interactive targets must be at least 44px').toEqual([])
    await expectNoHorizontalOverflow(page)
  })

  test('Admin shell fits the viewport without horizontal overflow', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expectShellFitsViewport(page, 'Overview')
  })

  test('Home and Admin navigation is reachable in both directions', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page).toHaveURL(/\/admin$/)
    await expectShellFitsViewport(page, 'Overview')

    await page.getByRole('link', { name: 'PlatPulse', exact: true }).click()
    await expect(page).toHaveURL(/\/$/)
    await expectShellFitsViewport(page, 'Home')
  })

  test('shell navigation is keyboard-operable with a visible focus ring', async ({ page }) => {
    await loginAs(page)

    // Tab from the brand to the Admin icon and verify the focus ring is visible.
    await page.keyboard.press('Tab')
    await expect(page.getByRole('link', { name: 'PlatPulse' })).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(page.getByRole('link', { name: 'Admin', exact: true })).toBeFocused()
    await expectFocusedElementHasVisibleFocus(page)

    // Enter activates the focused link without a pointer.
    await page.keyboard.press('Enter')
    await expect(page).toHaveURL(/\/admin$/)
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()

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
