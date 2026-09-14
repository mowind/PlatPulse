import { expect, test, type Locator, type Page } from '@playwright/test'
import {
  E2E_PASSWORD,
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow,
  loginAs,
} from './helpers'

/**
 * SCN-THEME-LIFECYCLE (webui.md §11.1 "Theme behavior"): the production
 * Auto → Light → Dark lifecycle, persistence, pre-mount first paint, and
 * readability of Login, Home, Admin, Network Overview, and the public Node
 * Detail in both themes, plus the public card feedback (issue #147). The seam
 * is the real routed application served by e2e/start-server.sh.
 */

const THEME_KEY = 'platpulse.themeMode'
const PROTOTYPE_THEME_KEY = 'platpulse.emerald-prototype.themeMode'

function themeButton(page: Page) {
  return page.getByRole('button', { name: /^Theme: / })
}

async function storedMode(page: Page) {
  return page.evaluate((key) => window.localStorage.getItem(key), THEME_KEY)
}

async function resolvedTheme(page: Page) {
  return page.evaluate(() => ({
    dark: document.documentElement.classList.contains('dark'),
    colorScheme: getComputedStyle(document.documentElement).colorScheme,
    attr: document.documentElement.getAttribute('data-theme-mode'),
  }))
}

/** Ordinary body text must keep at least 4.5:1 against its painted background.
 *  The target is a role/text locator where one exists, so the check does not
 *  depend on production CSS class names. */
async function expectReadable(page: Page, target: string | Locator) {
  const locator = typeof target === 'string' ? page.locator(target) : target
  const label = typeof target === 'string' ? target : 'readable public text'
  const result = await locator
    .first()
    .evaluate((element) => {
      const parse = (value: string) => (value.match(/[\d.]+/g) ?? []).map(Number)
      const luminance = (rgb: number[]) => {
        const [r, g, b] = rgb.slice(0, 3).map((value) => {
          const channel = value / 255
          return channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4
        })
        return 0.2126 * r + 0.7152 * g + 0.0722 * b
      }
      // Collect painted backgrounds from the element outward, then composite
      // them bottom-up so translucent panels are measured as actually painted.
      const layers: number[][] = []
      for (let node: Element | null = element; node; node = node.parentElement) {
        const parts = parse(getComputedStyle(node).backgroundColor)
        if (parts.length < 3) continue
        const alpha = parts.length === 4 ? parts[3] : 1
        if (alpha === 0) continue
        layers.push([parts[0], parts[1], parts[2], alpha])
        if (alpha === 1) break
      }
      let background: number[] = layers.length > 0 ? layers[layers.length - 1].slice(0, 3) : [255, 255, 255]
      for (let index = layers.length - 2; index >= 0; index -= 1) {
        const [r, g, b, alpha] = layers[index]
        background = [
          r * alpha + background[0] * (1 - alpha),
          g * alpha + background[1] * (1 - alpha),
          b * alpha + background[2] * (1 - alpha),
        ]
      }
      const foreground = parse(getComputedStyle(element).color)
      const light = luminance(foreground)
      const dark = luminance(background)
      return {
        ratio: (Math.max(light, dark) + 0.05) / (Math.min(light, dark) + 0.05),
        foreground,
        background,
      }
    })
  expect(result.ratio, label + ' contrast ' + JSON.stringify(result)).toBeGreaterThanOrEqual(4.5)
}

test('cycles the theme on Login with an accessible name and a touch-sized control', async ({ page }) => {
  await page.goto('/login')
  const button = themeButton(page)
  await expect(button).toHaveAttribute('aria-label', 'Theme: Auto. Switch to Light')

  const box = await button.boundingBox()
  expect(box?.width ?? 0, 'theme control width').toBeGreaterThanOrEqual(44)
  expect(box?.height ?? 0, 'theme control height').toBeGreaterThanOrEqual(44)

  // Reachable and visibly focused from the keyboard.
  for (let step = 0; step < 6; step += 1) {
    if (await button.evaluate((element) => element === document.activeElement)) break
    await page.keyboard.press('Tab')
  }
  await expect(button).toBeFocused()
  await expectFocusedElementHasVisibleFocus(page)

  await button.click()
  await expect(button).toHaveAttribute('aria-label', 'Theme: Light. Switch to Dark')
  await button.click()
  await expect(button).toHaveAttribute('aria-label', 'Theme: Dark. Switch to Auto')
  expect(await resolvedTheme(page)).toMatchObject({ dark: true, colorScheme: 'dark', attr: 'dark' })

  await button.click()
  await expect(button).toHaveAttribute('aria-label', 'Theme: Auto. Switch to Light')
  expect(await storedMode(page)).toBe('auto')
  await expectNoHorizontalOverflow(page)
})

test('persists the theme across reload and Home/Admin navigation', async ({ page }) => {
  await loginAs(page)

  await themeButton(page).click()
  await themeButton(page).click()
  await expect(themeButton(page)).toHaveAttribute('aria-label', 'Theme: Dark. Switch to Auto')
  expect(await storedMode(page)).toBe('dark')

  await page.reload()
  await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
  expect(await resolvedTheme(page)).toMatchObject({ dark: true, attr: 'dark' })

  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
  expect((await resolvedTheme(page)).dark).toBe(true)

  await page.getByRole('link', { name: 'PlatPulse', exact: true }).click()
  await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
  expect((await resolvedTheme(page)).dark).toBe(true)
  await expectNoHorizontalOverflow(page)
})

test('Auto follows the system and an explicit choice is never overridden', async ({ page }) => {
  await page.emulateMedia({ colorScheme: 'light' })
  await page.goto('/login')
  expect((await resolvedTheme(page)).dark).toBe(false)

  await page.emulateMedia({ colorScheme: 'dark' })
  await expect.poll(async () => (await resolvedTheme(page)).dark).toBe(true)
  expect(await storedMode(page)).toBe('auto')

  await themeButton(page).click()
  expect((await resolvedTheme(page)).dark).toBe(false)

  await page.emulateMedia({ colorScheme: 'light' })
  await page.emulateMedia({ colorScheme: 'dark' })
  await expect(themeButton(page)).toHaveAttribute('aria-label', 'Theme: Light. Switch to Dark')
  expect((await resolvedTheme(page)).dark).toBe(false)
})

test('defaults an invalid preference and the prototype key to Auto', async ({ page }) => {
  await page.addInitScript(
    (keys) => {
      window.localStorage.setItem(keys.production, 'sepia')
      window.localStorage.setItem(keys.prototype, 'dark')
    },
    { production: THEME_KEY, prototype: PROTOTYPE_THEME_KEY },
  )
  await page.goto('/login')
  await expect(themeButton(page)).toHaveAttribute('aria-label', 'Theme: Auto. Switch to Light')
  expect((await resolvedTheme(page)).dark).toBe(false)
})

test('paints the correct theme before the application module runs on direct entry', async ({ page }) => {
  await page.route('**/assets/*.js', (route) => route.abort())

  const cases = [
    { mode: 'light', dark: false, colorScheme: 'light', canvas: 'rgb(248, 250, 252)' },
    { mode: 'dark', dark: true, colorScheme: 'dark', canvas: 'rgb(20, 25, 35)' },
  ] as const

  for (const testCase of cases) {
    await page.addInitScript((mode) => {
      window.localStorage.setItem('platpulse.themeMode', mode)
    }, testCase.mode)
    for (const route of ['/', '/login', '/admin']) {
      const label = testCase.mode + ' ' + route
      await page.goto(route, { waitUntil: 'domcontentloaded' })
      await expect.poll(async () => (await resolvedTheme(page)).dark, { message: label }).toBe(testCase.dark)
      expect((await resolvedTheme(page)).colorScheme, label + ' color-scheme').toBe(testCase.colorScheme)

      const mounted = await page.evaluate(
        () => (document.getElementById('root')?.childElementCount ?? 0) > 0,
      )
      expect(mounted, label + ': the application module must be blocked').toBe(false)

      const canvas = await page.evaluate(
        () => getComputedStyle(document.documentElement).backgroundColor,
      )
      expect(canvas, label + ': pre-mount canvas').toBe(testCase.canvas)
    }
  }

  // Auto resolves against the system before mount without a stored preference.
  await page.emulateMedia({ colorScheme: 'dark' })
  await page.addInitScript(() => {
    window.localStorage.removeItem('platpulse.themeMode')
  })
  await page.goto('/login', { waitUntil: 'domcontentloaded' })
  await expect.poll(async () => (await resolvedTheme(page)).dark).toBe(true)
  expect((await resolvedTheme(page)).attr).toBe('auto')
})

test('keeps Login, Home, and Admin readable in both themes', async ({ page }) => {
  await page.goto('/login')

  await expectReadable(page, '#login-heading')
  await expectReadable(page, '.login-hint')

  await themeButton(page).click()
  await themeButton(page).click()
  await expect(themeButton(page)).toHaveAttribute('aria-label', 'Theme: Dark. Switch to Auto')
  await expectReadable(page, '#login-heading')
  await expectReadable(page, '.login-hint')

  // A failed dark-theme sign-in stays readable and operable before the
  // successful redirect.
  await page.getByLabel('Username').fill('admin')
  await page.getByLabel('Password').fill('not-the-password')
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page.getByRole('alert')).toContainText('Invalid username or password')
  await expectReadable(page, '.form-error')

  await page.getByLabel('Password').fill(E2E_PASSWORD)
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
  await expectReadable(page, '.app-brand')

  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
  await expectReadable(page, 'main h1')

  // The same surfaces stay readable after switching back to Light.
  await themeButton(page).click()
  await themeButton(page).click()
  expect((await resolvedTheme(page)).dark).toBe(false)
  await expectReadable(page, 'main h1')
  await page.getByRole('link', { name: 'PlatPulse', exact: true }).click()
  await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
  await expectReadable(page, '.app-brand')
  await expectNoHorizontalOverflow(page)
})

test('respects reduced motion while switching themes', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' })
  await page.goto('/login')
  const transitionSeconds = await themeButton(page).evaluate((element) =>
    Math.max(
      ...getComputedStyle(element)
        .transitionDuration.split(',')
        .map((value) => Number.parseFloat(value)),
    ),
  )
  expect(transitionSeconds, 'reduced motion removes the theme transition').toBeLessThan(0.01)
  await themeButton(page).click()
  await themeButton(page).click()
  expect((await resolvedTheme(page)).dark).toBe(true)
  await expectNoHorizontalOverflow(page)
})

/** A computed colour's alpha channel, defaulting to opaque. */
function colorAlpha(color: string): number {
  const parts = (color.match(/[\d.]+/g) ?? []).map(Number)
  return parts.length === 4 ? parts[3] : 1
}

test('keeps the public Network and Node Detail readable in both themes', async ({ page }) => {
  await loginAs(page)

  // Public Node Detail in Light: the hero metrics and the four chart cards
  // stay readable against their composed surfaces.
  await page.getByRole('link', { name: /Node A/ }).click()
  await expect(page.getByRole('heading', { level: 1, name: /Node A/ })).toBeVisible({ timeout: 15_000 })
  await expectReadable(page, page.getByText('Process uptime').first())
  await expectReadable(page, page.getByRole('heading', { level: 3, name: 'Network' }))
  await expect(page.getByRole('heading', { level: 2, name: 'Latest 60 seconds' })).toBeVisible()
  await expect(page.getByText('Peer diagnostics')).toBeVisible()
  await expectNoHorizontalOverflow(page)

  // The same surface in Dark.
  await themeButton(page).click()
  await themeButton(page).click()
  expect((await resolvedTheme(page)).dark).toBe(true)
  await expectReadable(page, page.getByText('Process uptime').first())
  await expectReadable(page, page.getByRole('heading', { level: 3, name: 'Network' }))
  await expect(page.getByRole('heading', { level: 2, name: 'Latest 60 seconds' })).toBeVisible()
  await expectNoHorizontalOverflow(page)

  // Network Overview keeps its Peer/Geo modules and readable copy in Dark.
  await page.getByRole('link', { name: /platon-e2e/ }).click()
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible({ timeout: 15_000 })
  await expect(page.getByRole('region', { name: 'Peer insight' })).toBeVisible()
  await expectReadable(page, page.getByRole('heading', { level: 1 }).first())
  await expectNoHorizontalOverflow(page)
})

test('gives public cards calibrated hover, reduced-motion, and filter feedback', async ({ page }) => {
  await loginAs(page)
  await themeButton(page).click()
  await themeButton(page).click()
  expect((await resolvedTheme(page)).dark).toBe(true)

  const hoverCapable = await page.evaluate(
    () => matchMedia('(hover: hover) and (pointer: fine)').matches,
  )
  const summaryCard = page.getByRole('article').filter({ hasText: 'Active Nodes' }).first()
  await expect(summaryCard).toBeVisible({ timeout: 15_000 })

  // At rest the card is a ~60% surface; the pointer is parked first so a
  // leftover hover from a previous action cannot be measured.
  await page.mouse.move(2, 2)
  const resting = await summaryCard.evaluate((card) => {
    const style = getComputedStyle(card)
    return { background: style.backgroundColor, transform: style.transform, shadow: style.boxShadow }
  })
  expect(colorAlpha(resting.background), 'summary card rests on a translucent surface').toBeLessThan(1)

  await summaryCard.hover()
  await page.waitForTimeout(320)
  const hovered = await summaryCard.evaluate((card) => {
    const style = getComputedStyle(card)
    return { background: style.backgroundColor, transform: style.transform, shadow: style.boxShadow }
  })
  if (hoverCapable) {
    expect(colorAlpha(hovered.background), 'hovered card becomes opaque').toBe(1)
    expect(hovered.transform, 'hover-capable devices get the 2px lift').not.toBe('none')
    expect(hovered.shadow, 'hover adds the Emerald glow').not.toBe(resting.shadow)
  } else {
    expect(hovered.transform, 'touch devices never receive the lift').toBe('none')
  }

  // Filters and sorting stay operable on the dark Home surface.
  await page.getByRole('group', { name: 'Network filter' }).getByRole('button').nth(1).click()
  await page.getByRole('combobox', { name: 'Sort' }).selectOption('head')
  await expectNoHorizontalOverflow(page)
  await page.getByRole('button', { name: 'All Networks', exact: true }).click()

  // The chart plot never moves on hover, even where hover is available.
  await page.getByRole('link', { name: /Node A/ }).click()
  await expect(page.getByRole('heading', { level: 1, name: /Node A/ })).toBeVisible({ timeout: 15_000 })
  const chartCard = page
    .getByRole('article')
    .filter({ has: page.getByRole('heading', { level: 3, name: 'Network' }) })
  await expect(chartCard).toBeVisible()
  await page.mouse.move(2, 2)
  const chartResting = await chartCard.evaluate((card) => getComputedStyle(card).boxShadow)
  await chartCard.hover()
  await page.waitForTimeout(320)
  const chartHovered = await chartCard.evaluate((card) => ({
    transform: getComputedStyle(card).transform,
    shadow: getComputedStyle(card).boxShadow,
  }))
  expect(
    chartHovered.transform === 'none' || chartHovered.transform === 'matrix(1, 0, 0, 1, 0, 0)',
    'the chart card keeps the plot in place',
  ).toBe(true)
  if (hoverCapable) {
    expect(chartHovered.shadow, 'the chart card still gains the outline/glow').not.toBe(chartResting)
  }

  // reduced motion removes the summary-card lift in both themes.
  await page.emulateMedia({ reducedMotion: 'reduce' })
  await page.goto('/')
  await expect(page.getByRole('region', { name: 'Home' })).toBeVisible({ timeout: 15_000 })
  const reducedCard = page.getByRole('article').filter({ hasText: 'Active Nodes' }).first()
  await page.mouse.move(2, 2)
  await reducedCard.hover()
  await page.waitForTimeout(120)
  expect(
    await reducedCard.evaluate((card) => getComputedStyle(card).transform),
    'reduced motion removes the lift in Dark',
  ).toBe('none')

  // One click from Dark reaches Auto; the Playwright default system theme is
  // Light, so the same reduced-motion contract is verified in both themes.
  await themeButton(page).click()
  expect((await resolvedTheme(page)).dark).toBe(false)
  await page.mouse.move(2, 2)
  await reducedCard.hover()
  await page.waitForTimeout(120)
  expect(
    await reducedCard.evaluate((card) => getComputedStyle(card).transform),
    'reduced motion removes the lift in Light',
  ).toBe('none')
  await expectNoHorizontalOverflow(page)
})

test('keeps the retained Admin workbench readable in both themes', async ({ page }, testInfo) => {
  await loginAs(page)

  // The public top treatment never crosses the Admin workbench.
  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
  await expect(page.locator('.admin-shell .background-decoration')).toHaveCount(0)

  const routes = [
    { path: '/admin', heading: 'Overview' },
    { path: '/admin/agents', heading: 'Agents' },
    { path: '/admin/nodes', heading: 'Nodes' },
    { path: '/admin/networks', heading: 'Networks' },
    { path: '/admin/settings', heading: 'Settings' },
    { path: '/admin/access/sessions', heading: 'Sessions' },
    { path: '/admin/access/audit', heading: 'Audit log' },
  ] as const

  // Direct entry on every retained route, so the pre-mount paint and the page
  // heading, table header, and layout are checked in the requested theme.
  async function sweep(theme: 'light' | 'dark') {
    for (const route of routes) {
      await page.goto(route.path)
      const heading = page.getByRole('heading', { level: 1, name: route.heading })
      await expect(heading).toBeVisible({ timeout: 15_000 })
      expect(
        (await resolvedTheme(page)).dark,
        theme + ' ' + route.path + ' direct entry',
      ).toBe(theme === 'dark')
      await expectReadable(page, heading)
      const tableHeader = page.locator('thead th').first()
      if ((await tableHeader.count()) > 0) await expectReadable(page, tableHeader)
      await expectNoHorizontalOverflow(page)
    }
  }

  // Explicit Light: one click from Auto selects Light and persists it.
  await themeButton(page).click()
  await expect(themeButton(page)).toHaveAttribute('aria-label', 'Theme: Light. Switch to Dark')
  expect((await resolvedTheme(page)).dark).toBe(false)
  await sweep('light')

  // Explicit Dark.
  await themeButton(page).click()
  await expect(themeButton(page)).toHaveAttribute('aria-label', 'Theme: Dark. Switch to Auto')
  expect((await resolvedTheme(page)).dark).toBe(true)
  await sweep('dark')

  // A representative management form (the Network registration disclosure)
  // stays readable on the dark workbench, including its labels and heading.
  await page.goto('/admin/networks')
  await expect(page.getByRole('heading', { level: 1, name: 'Networks' })).toBeVisible({ timeout: 15_000 })
  await page.getByRole('button', { name: 'Register a Network' }).click()
  await expectReadable(page, page.getByRole('heading', { level: 2, name: 'Register a Network' }))
  await expectReadable(page, page.getByLabel('Network key'))
  await expectReadable(page, page.getByLabel('Display name'))
  await page.getByRole('button', { name: 'Close form' }).click()

  // The Settings card and its secondary link stay readable in Dark.
  await page.goto('/admin/settings')
  await expect(page.getByRole('heading', { level: 1, name: 'Settings' })).toBeVisible({ timeout: 15_000 })
  await expectReadable(page, page.getByLabel('New window (days)'))
  await expectReadable(page, page.getByRole('link', { name: /Admin overview/ }))
  await expectReadable(page, page.locator('.settings-surface').first())

  // The mobile/tablet drawer keeps focus entry, scroll lock, Escape close, and
  // focus restoration while Dark is active.
  if (testInfo.project.name !== 'desktop-1280') {
    const menu = page.getByRole('button', { name: 'Menu' })
    await expect(menu).toBeVisible()
    await menu.click()
    const adminNav = page.getByRole('navigation', { name: 'Admin' })
    await expect(adminNav).toBeVisible()
    // Opening the drawer moves focus to the first retained page-group link.
    await expect(adminNav.getByRole('link', { name: 'Overview', exact: true })).toBeFocused()
    await expect.poll(() => page.evaluate(() => document.body.style.overflow)).toBe('hidden')
    await page.keyboard.press('Escape')
    await expect(menu).toBeFocused()
    await expect.poll(() => page.evaluate(() => document.body.style.overflow)).toBe('')
  }
})
