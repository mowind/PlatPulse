import { expect, test } from '@playwright/test'
import { loginAs, expectNoHorizontalOverflow } from './helpers'

const VALIDATOR_ID = '0195f2a1-0030-4030-8030-000000000030'

test('Home and Admin show validator analytics with chart and table alternatives', async ({ page }) => {
  await loginAs(page)
  await page.goto('/networks/platon-e2e')
  await expect(page.getByRole('heading', { level: 1, name: 'PlatON E2E Network' })).toBeVisible()
  await expect(page.getByRole('img', { name: /Daily validator rank trend/ }).first()).toBeVisible()
  await expect(page.locator('.validator-daily-table td[data-label="Local date"]', { hasText: '2026-02-01' })).toBeVisible()
  await expect(page.locator('.validator-monthly-table td[data-label="Month"]', { hasText: '2026-03' })).toBeVisible()
  await expectNoHorizontalOverflow(page)

  await page.goto(`/admin/validators/${VALIDATOR_ID}`)
  await expect(page.getByRole('heading', { level: 1, name: 'E2E Validator' })).toBeVisible()
  await expect(page.getByText('Received').first()).toBeVisible()
  await expect(page.getByText('Updated').first()).toBeVisible()
  await expectNoHorizontalOverflow(page)
})
