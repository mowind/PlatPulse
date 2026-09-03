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
  await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
}

/** The document must never overflow the viewport horizontally. */
export async function expectNoHorizontalOverflow(page: Page) {
  const { overflow, offenders } = await page.evaluate(() => {
    const vw = document.documentElement.clientWidth || window.innerWidth
    const offenders: string[] = []
    let overflow = Math.max(0, document.documentElement.scrollWidth - vw)
    const navToggle = document.querySelector<HTMLElement>('.nav-toggle')
    const mobileAdminDrawerClosed = Boolean(
      navToggle &&
      getComputedStyle(navToggle).display !== 'none' &&
      navToggle.getAttribute('aria-expanded') !== 'true',
    )
    for (const el of document.querySelectorAll<HTMLElement>('*')) {
      const style = getComputedStyle(el)
      const rect = el.getBoundingClientRect()
      if (style.display === 'none' || style.visibility === 'hidden' || rect.width === 0 || rect.height === 0) continue
      if (mobileAdminDrawerClosed && el.closest('.admin-nav')) continue
      let clippedByAncestor = false
      for (let parent = el.parentElement; parent; parent = parent.parentElement) {
        const overflowX = getComputedStyle(parent).overflowX
        if (overflowX === 'auto' || overflowX === 'scroll' || overflowX === 'hidden' || overflowX === 'clip') {
          clippedByAncestor = true
          break
        }
      }
      if (clippedByAncestor) continue
      const leftOverflow = Math.max(0, -rect.left)
      const rightOverflow = Math.max(0, rect.right - vw)
      if (leftOverflow > 0.5 || rightOverflow > 0.5) {
        overflow = Math.max(overflow, leftOverflow, rightOverflow)
        offenders.push(
          `${el.tagName.toLowerCase()}.${String(el.className).slice(0, 60)} ` +
            `[left=${Math.round(rect.left)} width=${Math.round(rect.width)}] ` +
            `"${(el.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 60)}"`,
        )
      }
    }
    return {
      overflow,
      offenders: offenders.slice(0, 10),
    }
  })
  expect(
    overflow,
    `page must not overflow horizontally: ${offenders.join(' | ')}`,
  ).toBeLessThanOrEqual(0)
}

/** Every visible control in a fixed-viewport scenario must remain a usable
 * 44px touch target. The selector is intentionally limited to native
 * interactive elements; it does not depend on component implementation
 * classes or private state. */
export async function expectVisibleInteractiveTargets(page: Page) {
  const undersized = await page.locator('a[href],button,input,select,textarea,summary').evaluateAll((elements) =>
    elements.flatMap((element) => {
      const html = element as HTMLElement
      const style = getComputedStyle(html)
      const rect = html.getBoundingClientRect()
      if (
        style.display === 'none' ||
        style.visibility === 'hidden' ||
        rect.width === 0 ||
        rect.height === 0 ||
        (html instanceof HTMLInputElement && html.type === 'hidden')
      ) return []
      return rect.width < 44 || rect.height < 44
        ? [`${html.tagName.toLowerCase()} ${html.textContent?.trim() || html.getAttribute('aria-label') || ''}`]
        : []
    }),
  )
  expect(undersized, 'visible interactive controls must be at least 44px').toEqual([])
}

/** Simulate browser zoom by applying its equivalent reduced CSS viewport. */
export async function setPageZoom(page: Page, factor: number) {
  const viewport = page.viewportSize()
  if (!viewport || factor <= 0) throw new Error('a positive viewport and zoom factor are required')
  const minimumReflowWidth = 320
  await page.setViewportSize({
    width: Math.max(minimumReflowWidth, Math.floor(viewport.width / factor)),
    height: Math.max(minimumReflowWidth, Math.floor(viewport.height / factor)),
  })
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
