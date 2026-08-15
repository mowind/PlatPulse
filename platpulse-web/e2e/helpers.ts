import { expect, type Page } from '@playwright/test'

/** Password provisioned by e2e/start-server.sh via stdin (never argv). */
export const E2E_PASSWORD = 'platpulse-e2e-admin-2026'

/** Viewer credentials provisioned by e2e/start-server.sh via stdin. */
export const E2E_VIEWER_USERNAME = 'viewer'
export const E2E_VIEWER_PASSWORD = 'platpulse-e2e-viewer-2026'

/** Sign in through the real login flow and land on the Home shell. */
export async function loginAs(
  page: Page,
  username = 'admin',
  password = E2E_PASSWORD,
) {
  await page.goto('/')
  // The Guest-access e2e (access.spec.ts) enables anonymous Home for a
  // short window; a fresh anonymous context then renders Home instead of
  // the login page. Retry until the protected login flow is reachable
  // again instead of misreading the Guest surface as a session.
  await expect(async () => {
    if (!page.url().endsWith('/login')) {
      await page.goto('/')
    }
    await expect(page).toHaveURL(/\/login$/, { timeout: 1_000 })
  }).toPass({ timeout: 30_000 })
  await page.getByLabel('Username').fill(username)
  await page.getByLabel('Password').fill(password)
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Home' })).toBeVisible()
}

/** The document must never overflow the viewport horizontally. */
export async function expectNoHorizontalOverflow(page: Page) {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - window.innerWidth,
  )
  expect(overflow, 'page must not overflow horizontally').toBeLessThanOrEqual(0)
}

/** Simulate browser zoom (200%) via CDP page scale. */
export async function setPageZoom(page: Page, factor: number) {
  const session = await page.context().newCDPSession(page)
  await session.send('Emulation.setPageScaleFactor', { pageScaleFactor: factor })
}

/** Assert the currently focused element has a visible focus indicator. */
export async function expectFocusedElementHasVisibleFocus(page: Page) {
  const focus = await page.evaluate(() => {
    const element = document.activeElement
    if (!(element instanceof HTMLElement)) return null
    const style = getComputedStyle(element)
    return {
      focusVisible: element.matches(':focus-visible'),
      outlineWidth: parseFloat(style.outlineWidth),
    }
  })
  expect(focus, 'an element must be focused').not.toBeNull()
  expect(focus!.focusVisible, 'focused element must match :focus-visible').toBe(true)
  expect(focus!.outlineWidth, 'focus must be visibly outlined').toBeGreaterThan(0)
}
