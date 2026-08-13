import { expect, test } from '@playwright/test'
import { E2E_PASSWORD, expectNoHorizontalOverflow, loginAs } from './helpers'

test.describe('Phase 1 release-candidate vertical slice', () => {
  test('public projection isolates a private Node and Node detail works at fixed viewports', async ({ page }) => {
    await loginAs(page)
    await expect(page.getByRole('link', { name: 'Node A' })).toBeVisible()
    await expect(page.getByText('Node B (private)', { exact: true })).toHaveCount(0)

    await page.getByRole('link', { name: 'Node A' }).click()
    await expect(page).toHaveURL(/\/nodes\/0195f2a1-0014-4014-8014-000000000014$/)
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible()
    await expect(page.getByText('Current head')).toBeVisible()
    await expectNoHorizontalOverflow(page)

    // A guessed private Node URL must be indistinguishable from a missing
    // public representation; no private label or diagnostics may leak.
    await page.goto('/nodes/0195f2a1-0015-4015-8015-000000000015')
    await expect(page.getByRole('alert')).toContainText('resource not found')
    await expect(page.getByText('Node B (private)', { exact: true })).toHaveCount(0)
  })

  test('Owner diagnostics and SSE reconnect do not disturb an active form field', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Admin diagnostics' })).toBeVisible()
    await expect(page.getByText('Node A', { exact: true })).toBeVisible()
    await expect(page.getByText('Node B (private)', { exact: true })).toBeVisible()

    const nodeId = page.getByLabel('Node ID')
    await nodeId.fill('0195f2a1-0014-4014-8014-000000000014')
    await page.waitForTimeout(1_100)
    await expect(nodeId).toHaveValue('0195f2a1-0014-4014-8014-000000000014')
    await expectNoHorizontalOverflow(page)
  })

  test('reduced-motion preference and keyboard login remain honored', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' })
    await page.goto('/login')
    await expect(page.getByLabel('Username')).toBeFocused()
    await page.getByLabel('Username').fill('admin')
    await page.getByLabel('Password').fill(E2E_PASSWORD)
    await page.keyboard.press('Enter')
    await expect(page.getByRole('heading', { level: 1, name: 'Home' })).toBeVisible()
    const reduced = await page.evaluate(() => {
      const style = getComputedStyle(document.documentElement)
      return matchMedia('(prefers-reduced-motion: reduce)').matches && style.scrollBehavior !== 'smooth'
    })
    expect(reduced).toBe(true)
  })
})
