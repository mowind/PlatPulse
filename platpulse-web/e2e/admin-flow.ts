import { expect, type Locator, type Page } from '@playwright/test'

import { expectNoHorizontalOverflow } from './helpers'
import { HARNESS_OWNER_PASSWORD, HARNESS_OWNER_USERNAME } from './server-harness'

/**
 * The driving vocabulary shared by the Admin acceptance specs (issues #208,
 * #209). Signing in, reaching an authenticated route, keyboard focus, local
 * table scroll, and the resolved theme are properties of the Admin shell, not
 * of one page, so both specs read the same definitions instead of a copy that
 * can drift.
 */

/** The five viewports every Admin acceptance matrix must cover. */
export const VIEWPORTS = [
  { width: 360, height: 800 },
  { width: 390, height: 844 },
  { width: 768, height: 1024 },
  { width: 1280, height: 800 },
  { width: 1440, height: 900 },
]

export async function loginToDisposableServer(page: Page, baseUrl: string) {
  await page.context().clearCookies()
  await page.goto(baseUrl + '/login')
  await page.getByLabel('Username').fill(HARNESS_OWNER_USERNAME)
  await page.getByLabel('Password').fill(HARNESS_OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
}

/** Reach one Admin route as the harness Owner, re-authenticating if the
 * session was dropped rather than asserting on a sign-in screen. */
export async function gotoAuthenticated(
  page: Page,
  baseUrl: string,
  path: string,
): Promise<void> {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await page.goto(baseUrl + path)
    const heading = page.locator('h1').first()
    await heading.waitFor({ state: 'visible', timeout: 15_000 })
    if (!/^Sign in\b/i.test((await heading.innerText()).trim())) return
    await loginToDisposableServer(page, baseUrl)
  }
  await page.goto(baseUrl + path)
}

/** Tab until the target control holds focus, so focus styling is exercised. */
export async function focusByKeyboard(page: Page, target: Locator): Promise<Locator> {
  for (let press = 0; press < 160; press += 1) {
    await page.keyboard.press('Tab')
    const focused = await target
      .evaluate((element) => element === document.activeElement)
      .catch(() => false)
    if (focused) return target
  }
  throw new Error('keyboard focus never reached the control ' + target)
}

/** A wide table scrolls inside its own region, never the document. */
export async function expectLocalTableScroll(page: Page, slot: string) {
  const overflowX = await page
    .locator('[data-slot="' + slot + '"]')
    .evaluate((table) => getComputedStyle(table.parentElement ?? table).overflowX)
  expect(overflowX, slot + ' must scroll inside its own region').toBe('auto')
  await expectNoHorizontalOverflow(page)
}

/**
 * The resolved theme must follow the emulated system preference, otherwise a
 * dark pass would only re-assert the light rendering it already passed.
 */
export async function expectResolvedTheme(page: Page, expected: 'light' | 'dark') {
  await expect
    .poll(
      async () => page.evaluate(() => document.documentElement.classList.contains('dark')),
      { message: () => 'the resolved theme must follow the emulated system preference' },
    )
    .toBe(expected === 'dark')
}
