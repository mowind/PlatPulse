import { expect, test, type Locator, type Page } from '@playwright/test'
import { E2E_PASSWORD, expectFocusedElementHasVisibleFocus, expectNoHorizontalOverflow, loginAs, expectComputedColor, expectLiftedUp } from './helpers'

/**
 * SCN-THEME-LIFECYCLE (webui.md §11.1 "Theme behavior"): the production
 * Auto → Light → Dark lifecycle, persistence, pre-mount first paint, and
 * readability of Login, Home, Admin, and the public Node Detail in both
 * themes, plus the public card feedback (issue #147). The seam
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

/** Read the shared public top atmosphere (issue #151). The fields default to
 *  empty so a missing layer fails an explicit assertion instead of throwing. */
async function readAtmosphere(page: Page) {
  return page.evaluate(() => {
    const empty = {
      ready: false,
      width: 0,
      height: 0,
      gradientOpacity: '',
      gradientImage: '',
      gradientMask: '',
      atmosphereMask: '',
      gridFill: '',
      ariaHidden: null as string | null,
      pointerEvents: '',
    }
    const decoration = document.querySelector('[data-slot="background-decoration"]')
    const atmosphere = document.querySelector('[data-slot="background-decoration-atmosphere"]')
    const gradient = document.querySelector('[data-slot="background-decoration-gradient"]')
    const grid = document.querySelector('[data-slot="background-decoration-grid"]')
    if (!decoration || !atmosphere || !gradient || !grid) return empty
    const atmosphereStyle = getComputedStyle(atmosphere)
    const gradientStyle = getComputedStyle(gradient)
    const gridStyle = getComputedStyle(grid)
    const box = atmosphere.getBoundingClientRect()
    return {
      ready: true,
      width: box.width,
      height: box.height,
      gradientOpacity: gradientStyle.opacity,
      gradientImage: gradientStyle.backgroundImage,
      gradientMask: gradientStyle.maskImage || gradientStyle.webkitMaskImage,
      atmosphereMask: atmosphereStyle.maskImage || atmosphereStyle.webkitMaskImage,
      gridFill: gridStyle.fill,
      ariaHidden: decoration.getAttribute('aria-hidden'),
      pointerEvents: getComputedStyle(decoration).pointerEvents,
    }
  })
}

/** Ordinary body text must keep at least 4.5:1 against its painted background.
 *  The target is a role/text locator where one exists, so the check does not
 *  depend on production CSS class names. */
async function expectReadable(page: Page, target: string | Locator) {
  const locator = typeof target === 'string' ? page.locator(target) : target
  const label = typeof target === 'string' ? target : 'readable public text'
  await expect.poll(async () => {
    const result = await locator
    .first()
    .evaluate((element) => {
      // Canvas converts modern CSS colours (including oklch) to sRGB before
      // contrast calculation; parsing their numeric components as RGB is wrong.
      const canvas = document.createElement('canvas')
      canvas.width = canvas.height = 1
      const context = canvas.getContext('2d')
      if (!context) throw new Error('Canvas colour conversion is unavailable')
      const parse = (value: string) => {
        context.clearRect(0, 0, 1, 1)
        context.fillStyle = value
        context.fillRect(0, 0, 1, 1)
        const [r, g, b, alpha] = context.getImageData(0, 0, 1, 1).data
        return [r, g, b, alpha / 255]
      }
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
    return result.ratio
  }, { message: label + ' contrast after theme transition' }).toBeGreaterThanOrEqual(4.5)
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
    // Emerald's --background is oklch(1 0 0) and oklch(0.141 0.005 285.823),
    // which index.html paints before the application module runs.
    { mode: 'light', dark: false, colorScheme: 'light', canvas: 'rgb(255, 255, 255)' },
    { mode: 'dark', dark: true, colorScheme: 'dark', canvas: 'rgb(9, 9, 11)' },
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

      const canvas = await page.evaluate(() => {
        const root = getComputedStyle(document.documentElement).backgroundColor
        return root === 'rgba(0, 0, 0, 0)' ? getComputedStyle(document.body).backgroundColor : root
      })
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
  await expectReadable(page, '[data-slot="login-hint"]')

  await themeButton(page).click()
  await themeButton(page).click()
  await expect(themeButton(page)).toHaveAttribute('aria-label', 'Theme: Dark. Switch to Auto')
  await expectReadable(page, '#login-heading')
  await expectReadable(page, '[data-slot="login-hint"]')

  // A failed dark-theme sign-in stays readable and operable before the
  // successful redirect.
  await page.getByLabel('Username').fill('admin')
  await page.getByLabel('Password').fill('not-the-password')
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page.getByRole('alert')).toContainText('Invalid username or password')
  await expectReadable(page, '[data-slot="form-error"]')

  await page.getByLabel('Password').fill(E2E_PASSWORD)
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
  await expectReadable(page, '[data-slot="app-brand"]')

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
  await expectReadable(page, '[data-slot="app-brand"]')
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

test('keeps Home and the public Node Detail readable in both themes', async ({ page }) => {
  await loginAs(page)

  // Public Node Detail in Light: the hero metrics and the six chart cards
  // stay readable against their composed surfaces.
  await page.getByRole('link', { name: /Node A/ }).click()
  await expect(page.getByRole('heading', { level: 1, name: /Node A/ })).toBeVisible({ timeout: 15_000 })
  await expectReadable(page, page.getByText('Process uptime').first())
  await expectReadable(page, page.getByRole('heading', { level: 3, name: 'Host network' }))
  await expect(page.getByRole('heading', { level: 2, name: 'Latest 60 seconds' })).toBeVisible()
  await expect(page.getByText('Peer diagnostics')).toBeVisible()
  await expectNoHorizontalOverflow(page)

  // The same surface in Dark.
  await themeButton(page).click()
  await themeButton(page).click()
  expect((await resolvedTheme(page)).dark).toBe(true)
  await expectReadable(page, page.getByText('Process uptime').first())
  await expectReadable(page, page.getByRole('heading', { level: 3, name: 'Host network' }))
  await expect(page.getByRole('heading', { level: 2, name: 'Latest 60 seconds' })).toBeVisible()
  await expectNoHorizontalOverflow(page)

  // The Node Detail breadcrumb is now "All Networks" and returns Home; the
  // deleted Network overview route cannot render, and Home stays readable in
  // Dark.
  const allNetworks = page.getByRole('link', { name: 'All Networks', exact: true })
  await expect(allNetworks).toHaveAttribute('href', '/')
  await allNetworks.click()
  await expect(page.getByRole('region', { name: 'Home' })).toBeVisible({ timeout: 15_000 })
  await expectReadable(page, page.getByRole('region', { name: 'Home' }))
  await expectNoHorizontalOverflow(page)
})

/** Normalize literals through the browser, not production tokens: changing a token
 * must not silently change the expectation alongside the implementation. */
async function normalizedStyle(page: Page, property: string, value: string) {
  return page.evaluate(({ property, value }) => {
    const probe = document.createElement('div')
    probe.style.setProperty(property, value)
    document.body.append(probe)
    const result = getComputedStyle(probe).getPropertyValue(property)
    probe.remove()
    return result
  }, { property, value })
}

const PUBLIC_FONT = 'system-ui, -apple-system, "Segoe UI", Roboto, sans-serif'

async function expectBorderless(card: Locator) {
  for (const side of ['top', 'right', 'bottom', 'left']) {
    await expect(card).toHaveCSS('border-' + side + '-style', 'none')
    await expect(card).toHaveCSS('border-' + side + '-width', '0px')
  }
}

async function expectQuietShadow(card: Locator) {
  // Emerald permits either no shadow or its transparent 1px outline at rest.
  await expect(card).toHaveCSS('box-shadow', /^(none|rgba\(0, 0, 0, 0\) 0px 0px 0px 1px)$/)
}

for (const theme of ['light', 'dark'] as const) {
  test('aligns public Emerald cards and isolates Admin in ' + theme, async ({ page }) => {
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'no-preference' })
    await loginAs(page)
    const hoverCapable = await page.evaluate(
      () => matchMedia('(hover: hover) and (pointer: fine)').matches,
    )
    const background = await normalizedStyle(page, 'background-color', theme === 'light'
      ? 'rgba(255, 255, 255, 0.6)' : 'oklch(0.141 0.005 285.823 / 0.6)')
    const opaque = await normalizedStyle(page, 'background-color', theme === 'light'
      ? 'rgb(255, 255, 255)' : 'oklch(0.141 0.005 285.823)')
    // Node Detail's three reading tiers: 60% summary, 50% info/chart, 40% disclosure.
    const background50 = await normalizedStyle(page, 'background-color', theme === 'light'
      ? 'rgba(255, 255, 255, 0.5)' : 'oklch(0.141 0.005 285.823 / 0.5)')
    const background40 = await normalizedStyle(page, 'background-color', theme === 'light'
      ? 'rgba(255, 255, 255, 0.4)' : 'oklch(0.141 0.005 285.823 / 0.4)')
    const font = await normalizedStyle(page, 'font-family', PUBLIC_FONT)
    const glow = await normalizedStyle(page, 'box-shadow',
      '0 0 20px oklch(0.596 0.145 163.225 / 10%), 0 0 0 1px oklch(0.596 0.145 163.225 / 10%)')

    async function checkCard(card: Locator, options: { lifts?: boolean; resting?: string; interactive?: boolean } = {}) {
      const { lifts = false, resting = background, interactive = false } = options
      await expect(card).toBeVisible({ timeout: 15_000 })
      await page.mouse.move(2, 2)
      await expectComputedColor(card, 'background-color', resting)
      await expect(card).toHaveCSS('font-family', font)
      await expectBorderless(card)
      await expect(card).toHaveCSS('backdrop-filter', 'none')
      await expect(card).toHaveCSS('background-image', 'none')
      await expectQuietShadow(card)
      await expect(card).toHaveCSS('transform', 'none')
      await card.hover()
      // Only a card that actually navigates or acts may go opaque on hover; a
      // static Node Detail surface keeps its resting tier opacity.
      await expectComputedColor(card, 'background-color', interactive && hoverCapable ? opaque : resting)
      await expectBorderless(card)
      if (hoverCapable && lifts) {
        await expectComputedColor(card, 'box-shadow', glow)
        await expectLiftedUp(card, 2)
      } else {
        await expectQuietShadow(card)
        await expect(card).toHaveCSS('transform', 'none')
      }
    }

    await expect(page.locator('[data-slot="home-shell"]')).toHaveCSS('font-family', font)
    await expect(themeButton(page)).toHaveCSS('font-family', font)
    await expect(page.getByRole('combobox', { name: 'Sort' })).toHaveCSS('font-family', font)
    await expect(page.locator('[data-slot="node-card"] h2').first()).toHaveCSS('font-weight', '600')
    await expect(page.locator('[data-slot="node-card"] h2').first()).toHaveCSS('font-size', '16px')
    await checkCard(page.getByRole('article').filter({ hasText: 'Active Nodes' }).first(), { interactive: true })
    const nodeLink = page.getByRole('link', { name: /Node A/ })
    const nodeCard = page.locator('[data-slot="node-card"]').filter({ has: nodeLink })
    await checkCard(nodeCard, { lifts: true, interactive: true })

    // A real keyboard traversal retains the whole-card link's visible focus ring.
    await page.mouse.move(2, 2)
    await themeButton(page).focus()
    for (let step = 0; step < 40; step += 1) {
      if (await nodeLink.evaluate((element) => element === document.activeElement)) break
      await page.keyboard.press('Tab')
    }
    await expect(nodeLink).toBeFocused()
    await expectFocusedElementHasVisibleFocus(page)
    await nodeLink.press('Enter')
    await expect(page.getByRole('heading', { level: 1, name: /Node A/ })).toBeVisible({ timeout: 15_000 })
    await expect(page.locator('[data-slot="home-shell"]')).toHaveCSS('font-family', font)
    // Cover every rendered information group, summary tile, and chart card,
    // rather than letting one passing representative hide a stale override.
    // The Validator diagnostics borrow the shared disclosure presentation but
    // must not paint a second card inside the Linked Validator card.
    const nestedDisclosure = page.locator('[data-slot="home-shell"] details[data-surface="none"]')
    await expect(nestedDisclosure).toHaveCount(1)
    await expect(nestedDisclosure).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)')

    for (const [selector, resting] of [
      ['[data-slot="node-summary-tile"]', background],
      ['[data-slot="node-info-group"]', background50],
      ['[data-slot="node-metric-card"]', background50],
      ['[data-slot="linked-validator"]', background50],
      ['details[data-slot="disclosure"][data-surface="card"]', background40],
    ] as Array<[string, string]>) {
      const cards = page.locator('[data-slot="home-shell"] ' + selector)
      expect(await cards.count(), selector + ' fixture coverage').toBeGreaterThan(0)
      for (const card of await cards.all()) await checkCard(card, { resting })
    }

    await page.goto('/')
    await expect(nodeLink).toBeVisible({ timeout: 15_000 })
    // Filters and sorting stay operable on both public surfaces.
    await page.getByRole('tablist', { name: 'Network filter' }).getByRole('tab').nth(1).click()
    await page.getByRole('combobox', { name: 'Sort' }).selectOption('head')
    await page.getByRole('tab', { name: 'All Networks', exact: true }).click()
    await page.emulateMedia({ reducedMotion: 'reduce' })
    await nodeCard.hover()
    await expect(nodeCard).toHaveCSS('transform', 'none')
    await expect(nodeCard).toHaveCSS('transition-duration', '0s')
    await expectComputedColor(nodeCard, 'background-color', hoverCapable ? opaque : background)
    if (hoverCapable) await expectComputedColor(nodeCard, 'box-shadow', glow)
    await expectNoHorizontalOverflow(page)

    // SPA navigation keeps Emerald typography and surfaces, not the public data shell.
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
    // The migration resolved the old Inter/system-ui split into Emerald's
    // single system stack, so public and Admin now share one family
    // (deviation 6).
    await expect(page.locator('[data-slot="admin-shell"]')).toHaveCSS('font-family', font)
    await expect(page.locator('[data-slot="admin-shell"] [data-slot="background-decoration"]')).toHaveCount(1)
    await expect(page.locator('[data-slot="admin-shell"] [data-slot="background-decoration"]')).toHaveAttribute('aria-hidden', 'true')
    await expect(page.locator('[data-slot="admin-shell"] [data-slot="geo-chart"]')).toHaveCount(0)
    await page.goto('/admin/networks')
    await page.getByRole('button', { name: 'Register a Network' }).click()
    const adminCard = page.locator('#network-create-form')
    await expect(adminCard).toBeVisible({ timeout: 15_000 })
    await expect(adminCard).toHaveCSS('font-family', font)
    // Admin panels now use Emerald's single card surface too, so they
    // resolve to the same 60% background the public cards do (deviation 2).
    await expectComputedColor(adminCard, 'background-color', background)
    // Emerald cards are borderless - its own components pass border-none - while
    // the retired Admin panel carried a 1px border. Asserting all four sides is
    // stronger than the single-side colour check this replaces.
    await expectBorderless(adminCard)
  })
}

test('keeps the retained Admin workbench readable in both themes', async ({ page }, testInfo) => {
  await loginAs(page)

  // ADR 0003 retains Emerald BackgroundDecoration in Admin; only the workspace
  // geometry changes. The old zero-decoration assertion predates this contract.
  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
  await expect(page.locator('[data-slot="admin-shell"] [data-slot="background-decoration"]')).toHaveCount(1)
  await expect(page.locator('[data-slot="admin-shell"] [data-slot="background-decoration"]')).toHaveAttribute('aria-hidden', 'true')
  await expect(page.locator('[data-slot="admin-shell"] [data-slot="geo-chart"]')).toHaveCount(0)

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
  await expectReadable(page, page.locator('[data-slot="settings-block"]').first())

  // The mobile/tablet drawer keeps focus entry, scroll lock, Escape close, and
  // focus restoration while Dark is active.
  if (!['desktop-1280', 'desktop-1440'].includes(testInfo.project.name)) {
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

/**
 * Issue #151: the public top atmosphere is the faithful Emerald Background.vue
 * port rather than the earlier hand-written approximation. The one deliberate
 * deviation is width, so the check widens past the upstream fixed 1300px to
 * prove the layer is full-bleed.
 */
test('ports the shared Emerald top atmosphere in both themes', async ({ page }) => {
  await page.setViewportSize({ width: 1512, height: 900 })
  await page.goto('/login')

  const light = await readAtmosphere(page)
  expect(light.ready, 'the shared atmosphere layer renders on Login').toBe(true)
  // An earlier PlatPulse port stretched the wash to the viewport; that
  // adaptation is gone, so this asserts upstream's own fixed w-325 (1300px).
  expect(Math.round(light.width), "the atmosphere keeps upstream's fixed 1300px width").toBe(1300)
  expect(light.height).toBe(400)
  expect(light.gradientImage).toContain('linear-gradient')
  expect(light.gradientOpacity).toBe('0.4')
  expect(light.gradientMask).toContain('radial-gradient')
  expect(light.gridFill).toMatch(/0\.4|40%/)
  expect(light.ariaHidden).toBe('true')
  expect(light.pointerEvents).toBe('none')

  await themeButton(page).click()
  await themeButton(page).click()
  await expect(themeButton(page)).toHaveAttribute('aria-label', 'Theme: Dark. Switch to Auto')
  const dark = await readAtmosphere(page)
  expect(dark.atmosphereMask, 'Dark keeps the upstream vertical atmosphere mask').toContain('linear-gradient')
  expect(dark.gradientOpacity).toBe('1')
  expect(dark.gridFill, 'the dark grid is white at 2.5%').toMatch(/rgba\(255, 255, 255, 0\.02|oklab\([^)]*\/ 0\.025\)/)
})