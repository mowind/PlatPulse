import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, expectVisibleInteractiveTargets, loginAs } from './helpers'

test('Login has an accessible brand header in both themes', async ({ page }) => {
  await page.goto('/login')
  const header = page.getByRole('banner')
  await expect(header.getByRole('link', { name: 'PlatPulse' })).toHaveAttribute('href', '/')
  await expect(header.getByRole('link', { name: 'Admin', exact: true })).toHaveCount(0)
  for (const dark of [false, true]) {
    await page.evaluate((value) => document.documentElement.classList.toggle('dark', value), dark)
    await expect(page.getByRole('heading', { name: 'Sign in to PlatPulse' })).toBeVisible()
    await expectVisibleInteractiveTargets(page)
    await expectNoHorizontalOverflow(page)
    const headerBox = await header.boundingBox()
    const formBox = await page.locator('form').boundingBox()
    expect(headerBox).not.toBeNull()
    expect(formBox).not.toBeNull()
    expect(formBox!.y).toBeGreaterThanOrEqual(headerBox!.y + headerBox!.height)
  }
})

test('selection controls keep compact indicators and native touch and keyboard behavior', async ({ page }) => {
  await loginAs(page)
  await page.goto('/admin/settings')
  const card = page.getByRole('article').filter({ has: page.getByRole('heading', { name: 'Geo provider' }) })
  const disabledProvider = card.getByRole('radio', { name: 'Disabled', exact: true })
  const local = card.getByRole('radio', { name: 'Local MMDB', exact: true })
  await expect(card.getByRole('radio', { checked: true })).toHaveCount(1)
  for (const dark of [false, true]) {
    await page.evaluate((value) => document.documentElement.classList.toggle('dark', value), dark)
    const inputBox = await disabledProvider.boundingBox()
    const indicator = disabledProvider.locator('..').locator('[data-slot="selection-indicator"]')
    const indicatorBox = await indicator.boundingBox()
    expect(inputBox).not.toBeNull()
    expect(indicatorBox).not.toBeNull()
    expect(inputBox!.width).toBeGreaterThanOrEqual(44)
    expect(inputBox!.height).toBeGreaterThanOrEqual(44)
    expect(indicatorBox!.width).toBe(16)
    expect(indicatorBox!.height).toBe(16)
    // Click outside the 16px indicator but inside the real 44px input.
    await disabledProvider.click({ position: { x: 2, y: 2 } })
    await expect(disabledProvider).toBeChecked()
    await disabledProvider.press('ArrowDown')
    await expect(local).toBeChecked()
    await expect(local).toBeFocused()
    expect(await local.evaluate((element) => element.matches(':focus-visible'))).toBe(true)
    const localIndicator = local.locator('..').locator('[data-slot="selection-indicator"]')
    await expect(localIndicator).toHaveCSS('opacity', '1')
    await expect(localIndicator).toHaveCSS('box-shadow', /0px 0px 0px 3px/)
    await expect(localIndicator.locator('span')).toBeVisible()
    await local.press('ArrowUp')
    await expect(disabledProvider).toBeChecked()
    await expectVisibleInteractiveTargets(page)
    await expectNoHorizontalOverflow(page)
  }
  await page.emulateMedia({ forcedColors: 'active' })
  await expect(disabledProvider).toHaveCSS('opacity', '1')
  await expect(disabledProvider.locator('..').locator('[data-slot="selection-indicator"]')).toBeHidden()
  await disabledProvider.press('ArrowDown')
  await expect(local).toBeChecked()
  // Local selection only: deliberately never save a provider change.
})

test('Home retains six overview cards beside a proportional map', async ({ page }) => {
  await loginAs(page)
  const summary = page.locator('[aria-label="Home summary"]')
  await expect(summary).toBeVisible()
  await expect(summary.locator('[data-slot="summary-card"]')).toHaveCount(6)
  const geometry = await summary.evaluate((element) => {
    const band = element.parentElement!
    const map = band.querySelector('[data-slot="home-map"]')!
    return {
      summary: element.getBoundingClientRect().width,
      map: map.getBoundingClientRect().width,
      summaryTop: element.getBoundingClientRect().top,
      summaryBottom: element.getBoundingClientRect().bottom,
      mapTop: map.getBoundingClientRect().top,
    }
  })
  const width = page.viewportSize()!.width
  if (width >= 1024) {
    // Desktop keeps the map beside the statistics. The 4:3 split holds each of
    // the six tiles at the width the original four-card grid gave them.
    expect(geometry.summary / geometry.map).toBeCloseTo(4 / 3, 1)
    expect(geometry.mapTop).toBeLessThan(geometry.summaryBottom)
  } else {
    // Narrow layouts stack the complete map below the statistics.
    expect(geometry.mapTop).toBeGreaterThanOrEqual(geometry.summaryBottom - 1)
  }
  await expectNoHorizontalOverflow(page)
})
