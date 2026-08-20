import { expect, test, type Page } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

async function openAdmin(page: Page, section: string) {
  await loginAs(page)
  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible({ timeout: 15_000 })
  const menu = page.getByRole('button', { name: 'Menu' })
  if (await menu.isVisible()) await menu.click()
  await page.getByRole('link', { name: section, exact: true }).click()
}

test.describe('Admin configuration workflows (issue #86)', () => {
  test('History Window exposes bounds, consequences, and typed confirmation', async ({ page }) => {
    await openAdmin(page, 'History Window')
    await expect(page.getByRole('heading', { level: 1, name: 'History Window' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText(/Shortening it asynchronously removes expired history/)).toBeVisible()
    await expect(page.getByText(/Safety bounds/)).toBeVisible()
    const save = page.getByRole('button', { name: 'Save History Window' })
    await expect(save).toBeDisabled()
    await expect(page.getByLabel('Type the change to confirm')).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('Site Access shows the global mode and safe transition copy', async ({ page }) => {
    await openAdmin(page, 'Site Access')
    await expect(page.getByRole('heading', { level: 1, name: 'Site Access' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByRole('heading', { level: 2, name: 'Current mode' })).toBeVisible()
    await expect(page.getByRole('button', { name: /Make Home (Public|Private)/ })).toBeVisible()
    await expect(page.getByText(/Admin always requires an Owner Session/)).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })
})
