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

test.describe('Admin Settings workflows (issues #111 and #113)', () => {
  test('History Window exposes bounds, consequences, and typed confirmation', async ({ page }) => {
    await openAdmin(page, 'Settings')
    await expect(page.getByRole('heading', { level: 1, name: 'Settings' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByRole('heading', { level: 2, name: 'History Window' })).toBeVisible()
    await expect(page.getByText(/Shortening removes expired history asynchronously/)).toBeVisible()
    await expect(page.getByText(/Lengthening cannot recover deleted or missed history/)).toBeVisible()
    const save = page.getByRole('button', { name: 'Save History Window' })
    await expect(save).toBeDisabled()
    await expect(page.getByLabel('Type the change to confirm')).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('History Window rejects values outside the Server safety bounds', async ({ page }) => {
    await openAdmin(page, 'Settings')
    await expect(page.getByRole('heading', { level: 1, name: 'Settings' })).toBeVisible({ timeout: 15_000 })
    const days = page.getByLabel('New window (days)')
    const save = page.getByRole('button', { name: 'Save History Window' })
    const minimum = Number(await days.getAttribute('min'))
    const maximum = Number(await days.getAttribute('max'))

    await days.fill(String(minimum - 1))
    await expect(page.getByRole('alert')).toContainText(`Must be between ${minimum} and ${maximum} days.`)
    await expect(save).toBeDisabled()

    await days.fill(String(maximum + 1))
    await expect(page.getByRole('alert')).toContainText(`Must be between ${minimum} and ${maximum} days.`)
    await expect(save).toBeDisabled()
    await expectNoHorizontalOverflow(page)
  })

  test('History Window completes impact preview, typed confirmation, save, and Audit feedback', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'History Window mutation runs once on the desktop fixture')
    await openAdmin(page, 'Settings')

    const card = page.locator('article.settings-card').filter({ hasText: 'History Window' })
    const days = card.getByLabel('New window (days)')
    const save = card.getByRole('button', { name: 'Save History Window' })
    const confirmation = card.getByLabel('Type the change to confirm')
    const currentText = await card.locator('dl > div').filter({ hasText: /^Current/ }).locator('dd').textContent()
    const current = Number(currentText?.match(/\d+/)?.[0])
    const minimum = Number(await days.getAttribute('min'))
    const maximum = Number(await days.getAttribute('max'))
    expect(Number.isInteger(current)).toBe(true)

    const candidate = current === minimum ? minimum + 1 : minimum
    expect(candidate).toBeLessThanOrEqual(maximum)
    await days.fill(String(candidate))
    await expect(card.getByRole('heading', { name: 'Impact preview' })).toBeVisible()
    await expect(card.getByText('Protected history state is preserved.')).toBeVisible({ timeout: 15_000 })
    await confirmation.fill('history-window ' + candidate)
    await expect(save).toBeEnabled()
    await save.click()
    await expect(card.getByRole('status')).toContainText(
      new RegExp('History Window updated to ' + candidate + ' days [(]Audit #[0-9]+[)]'),
    )

    await days.fill(String(current))
    await expect(card.getByText('Protected history state is preserved.')).toBeVisible({ timeout: 15_000 })
    await confirmation.fill('history-window ' + current)
    await expect(save).toBeEnabled()
    await save.click()
    await expect(card.getByRole('status')).toContainText(
      new RegExp('History Window updated to ' + current + ' days [(]Audit #[0-9]+[)]'),
    )
    await expectNoHorizontalOverflow(page)
  })

  test('Site Access shows the global mode and safe transition copy', async ({ page }) => {
    await openAdmin(page, 'Settings')
    await expect(page.getByRole('heading', { level: 1, name: 'Settings' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByRole('heading', { level: 2, name: 'Site Access Mode' })).toBeVisible()
    await expect(page.getByRole('button', { name: /Make Home (Public|Private)/ })).toBeVisible()
    await expect(page.getByText(/Public permits anonymous Home reads/)).toBeVisible()
    await expect(page.getByText(/Private requires Owner login/)).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })
})
