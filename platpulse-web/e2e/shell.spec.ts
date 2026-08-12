import { expect, test, type Page } from '@playwright/test'

async function expectNoHorizontalOverflow(page: Page) {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - window.innerWidth,
  )
  expect(overflow, 'page must not overflow horizontally').toBeLessThanOrEqual(0)
}

test.describe('Phase 0 shell', () => {
  test('Home shell fits the viewport without horizontal overflow', async ({ page }) => {
    await page.goto('/')
    await expect(page.getByRole('heading', { level: 1, name: 'Home' })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('Admin shell fits the viewport without horizontal overflow', async ({ page }) => {
    await page.goto('/admin')
    await expect(page.getByRole('heading', { level: 1, name: 'Admin' })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('Home and Admin navigation is reachable in both directions', async ({ page }) => {
    await page.goto('/')
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page).toHaveURL(/\/admin$/)
    await expect(page.getByRole('heading', { level: 1, name: 'Admin' })).toBeVisible()
    await expectNoHorizontalOverflow(page)

    await page.getByRole('link', { name: 'Home', exact: true }).click()
    await expect(page).toHaveURL(/\/$/)
    await expect(page.getByRole('heading', { level: 1, name: 'Home' })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })
})
