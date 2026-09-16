import { expect, type Locator, type Page } from '@playwright/test'

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
    const navToggle = document.querySelector<HTMLElement>('[data-slot="admin-nav-toggle"]')
    const mobileAdminDrawerClosed = Boolean(
      navToggle &&
      getComputedStyle(navToggle).display !== 'none' &&
      navToggle.getAttribute('aria-expanded') !== 'true',
    )
    for (const el of document.querySelectorAll<HTMLElement>('*')) {
      const style = getComputedStyle(el)
      const rect = el.getBoundingClientRect()
      if (style.display === 'none' || style.visibility === 'hidden' || rect.width === 0 || rect.height === 0) continue
      if (mobileAdminDrawerClosed && el.closest('[data-slot="admin-nav"]')) continue
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

/** Every metric row inside a surface is one `data item / value` line at the
 *  current viewport: the label and value share a line, the value's right edge
 *  meets the row's right edge, and a progress track spans the full row
 *  (issue #140). Returns the offending rows as the failure payload. */
export async function expectMetricRowsAligned(scope: Locator) {
  const offenders = await scope.locator('[data-slot="metric-row"]').evaluateAll((rows) =>
    rows.flatMap((row) => {
      const label = row.querySelector('[data-slot="metric-row-label"]')
      const value = row.querySelector('[data-slot="metric-row-value"]')
      if (!label || !value) return ['a metric row is missing its label or value']
      const labelBox = label.getBoundingClientRect()
      const valueBox = value.getBoundingClientRect()
      const rowBox = row.getBoundingClientRect()
      const sameLine = Math.abs(labelBox.top - valueBox.top) <= 8
      const valueRightAligned = Math.abs(valueBox.right - rowBox.right) <= 1.5
      const valueRightOfLabel = valueBox.left >= labelBox.right - 1
      const progress = row.querySelector('[data-slot="progress-thin"]')
      const progressFullWidth = progress === null || Math.abs(progress.getBoundingClientRect().width - rowBox.width) <= 1.5
      return sameLine && valueRightAligned && valueRightOfLabel && progressFullWidth
        ? []
        : [`"${(row.textContent ?? '').replace(/\s+/g, ' ').trim()}" sameLine=${sameLine} rightAligned=${valueRightAligned} rightOfLabel=${valueRightOfLabel} fullTrack=${progressFullWidth}`]
    }),
  )
  expect(offenders, 'every metric row is one data-item / value line with the value flush right').toEqual([])
}

/** Open the Node Detail Peer diagnostics disclosure by pointer or keyboard
 *  and assert it opened. */
export async function openPeerDisclosure(page: Page, via: 'click' | 'keyboard' = 'click') {
  const disclosure = page.locator('details[data-slot="node-disclosure"]', { hasText: 'Peer diagnostics' })
  const summary = disclosure.locator('summary')
  if (via === 'keyboard') {
    await summary.focus()
    await page.keyboard.press('Enter')
  } else {
    await summary.click()
  }
  await expect(disclosure).toHaveAttribute('open', '')
  return disclosure
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
      let hitHeight = rect.height
      if (html.matches('.compact-tabs [data-slot="tabs-trigger"]')) {
        const pseudo = getComputedStyle(html, '::before')
        const top = rect.top + parseFloat(pseudo.top)
        const bottom = rect.bottom - parseFloat(pseudo.bottom)
        // Horizontal tab scrolling intentionally clips offscreen controls;
        // measure the visible part and require it to hit the actual trigger.
        const scroller = html.closest('.overflow-x-auto')!.getBoundingClientRect()
        const left = Math.max(rect.left, scroller.left)
        const right = Math.min(rect.right, scroller.right)
        // Partially scrolled-out tabs are exercised after scrollIntoView in
        // emerald-refinement.spec.ts, not counted as fully exposed targets.
        if (right - left < rect.width - 1) return []
        const x = (left + right) / 2
        if (html.contains(document.elementFromPoint(x, top + 1)) && html.contains(document.elementFromPoint(x, bottom - 1))) hitHeight = bottom - top
      }
      return rect.width < 44 || hitHeight < 44
        ? [`${html.tagName.toLowerCase()} ${html.textContent?.trim() || html.getAttribute('aria-label') || ''}`]
        : []
    }),
  )
  expect(undersized, 'visible interactive controls must have hit-tested targets of at least 44px').toEqual([])
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

/**
 * Compare a computed colour by value rather than by notation.
 *
 * Chrome preserves the specified colour space, so an expectation written as
 * rgba(255, 255, 255, 0.6) and a value derived from Emerald's oklch token
 * (bg-background/60) come back as oklab(1 0 0 / 0.6): the same colour, spelled
 * differently, which toHaveCSS reports as a mismatch. Both sides are resolved to
 * sRGB components through a 1x1 canvas here, so the assertion still pins the
 * exact colour and alpha. A notation the canvas rejects samples as
 * [-1, -1, -1, -1] and fails loudly rather than passing by accident.
 */
export async function expectComputedColor(locator: Locator, property: string, expected: string) {
  // Poll rather than read once: these surfaces transition (150ms on the card
  // surface), and a single read catches the interpolated value mid-transition
  // exactly the way toHaveCSS would not.
  await expect
    .poll(
      async () => {
        const actual = await locator.evaluate(
          (element, prop) => getComputedStyle(element).getPropertyValue(prop).trim(),
          property,
        )
        return locator.evaluate(
    (_element, pair) => {
      const sample = (input: string): number[] => {
        const canvas = document.createElement('canvas')
        canvas.width = 1
        canvas.height = 1
        const context = canvas.getContext('2d')
        if (!context) return [-1, -1, -1, -1]
        context.fillStyle = '#010203'
        context.fillStyle = input
        if (context.fillStyle === '#010203' && input.trim().toLowerCase() !== '#010203') {
          return [-1, -1, -1, -1]
        }
        context.clearRect(0, 0, 1, 1)
        context.fillRect(0, 0, 1, 1)
        return Array.from(context.getImageData(0, 0, 1, 1).data)
      }
      // Canonicalise every colour token in the value, not just whole-value
      // colours, so a box-shadow written with rgba() and the same shadow
      // derived from an oklch token compare equal while the geometry (offsets,
      // blur, spread) still has to match exactly. A token the canvas rejects is
      // left as written, so an unknown notation fails instead of passing.
      const canonical = (input: string) =>
        input.replace(
          /(?:rgba?|oklab|oklch|hsla?|hwb|lab|lch|color)\([^()]*\)|#[0-9a-fA-F]{3,8}\b/g,
          (token) => {
            const px = sample(token)
            return px.some((component) => component < 0) ? token : 'rgba(' + px.join(',') + ')'
          },
        )
      const left = canonical(pair[0])
      const right = canonical(pair[1])
      if (left === right) return true
      // Fall back to component comparison for a bare colour with rounding.
      const leftPx = sample(pair[0])
      const rightPx = sample(pair[1])
      return (
        leftPx.length === rightPx.length &&
        leftPx.every((component, index) => Math.abs(component - rightPx[index]) <= 1)
      )
    },
    [actual, expected] as [string, string],
  )
      },
      { message: () => property + ' should equal ' + expected, timeout: 5000 },
    )
    .toBe(true)
}

/**
 * Assert a hover lift by its effect rather than by its mechanism. Tailwind v4
 * expresses translate-* through the individual 'translate' property, while v3
 * emitted a transform matrix; both are a 2px lift. The vertical offset is read
 * from whichever mechanism is in play, so the assertion still pins the exact
 * distance.
 */
export async function expectLiftedUp(locator: Locator, pixels: number) {
  await expect
    .poll(
      async () =>
        locator.evaluate((element, expected) => {
          const style = getComputedStyle(element)
          const matrix = style.transform.match(/^matrix\(([^)]+)\)$/)
          const matrixY = matrix ? Number(matrix[1].split(',')[5]) : null
          if (matrixY != null && Number.isFinite(matrixY)) return Math.abs(matrixY + expected) < 0.5
          const translate = style.translate.match(/^(-?[\d.]+)px(?:\s+(-?[\d.]+)px)?$/)
          if (translate) {
            const y = translate[2] == null ? 0 : Number(translate[2])
            return Math.abs(y + expected) < 0.5
          }
          return false
        }, pixels),
      { message: () => 'the element is lifted by ' + pixels + 'px on hover', timeout: 5000 },
    )
    .toBe(true)
}
