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

test('Home retains the compact band with the approved statistics to map split', async ({ page }) => {
  await loginAs(page)
  const summary = page.locator('[aria-label="Home summary"]')
  await expect(summary).toBeVisible()
  const geometry = await summary.evaluate((element) => {
    const band = element.parentElement!
    const map = band.firstElementChild!
    return {
      summary: element.getBoundingClientRect().width,
      map: map.getBoundingClientRect().width,
      bandHeight: band.getBoundingClientRect().height,
    }
  })
  if (page.viewportSize()!.width >= 768) {
    expect(geometry.summary / geometry.map).toBeCloseTo(37 / 61, 2)
    expect(geometry.bandHeight).toBe(232)
  } else {
    expect(geometry.summary).toBeCloseTo(geometry.map, 0)
  }
  await expectNoHorizontalOverflow(page)
})
