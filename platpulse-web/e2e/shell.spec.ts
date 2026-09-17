import { expect, test, type Page } from '@playwright/test'
import {
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow, expectVisibleInteractiveTargets,
  loginAs,
} from './helpers'

async function expectShellFitsViewport(page: Page, heading: string) {
  if (heading === 'Home') {
    await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
  } else {
    await expect(page.getByRole('heading', { level: 1, name: heading })).toBeVisible()
  }
  await expectNoHorizontalOverflow(page)
}

async function measureAdminWorkbench(page: Page) {
  return page.evaluate(() => {
    const header = document.querySelector('header')
    const nav = document.querySelector('nav[aria-label="Admin"]')
    const main = document.querySelector('main')
    const pageContent = main?.querySelector('[data-slot="admin-page"]')
    const heading = main?.querySelector('h1')
    if (!header || !nav || !main || !pageContent || !heading) {
      throw new Error('Admin workbench geometry surfaces are missing')
    }
    const box = (element: Element) => {
      const rect = element.getBoundingClientRect()
      return { x: rect.x, right: rect.right, width: rect.width, height: rect.height, top: rect.top, bottom: rect.bottom }
    }
    return {
      viewport: document.documentElement.clientWidth,
      header: box(header),
      nav: box(nav),
      main: box(main),
      page: box(pageContent),
      heading: box(heading),
      headingFontSize: Number.parseFloat(getComputedStyle(heading).fontSize),
      decorationCount: document.querySelectorAll('[data-slot="background-decoration"]').length,
    }
  })
}

test.describe('Authenticated shell', () => {
  test('Home shell fits the viewport without horizontal overflow', async ({ page }) => {
    await loginAs(page)
    await expectShellFitsViewport(page, 'Home')
  })

  test('Home dashboard exposes the public scan controls at every fixed viewport', async ({ page }) => {
    await loginAs(page)
    await expect(page.getByText('Active Nodes', { exact: true })).toBeVisible()
    await expect(page.getByText('Healthy Nodes', { exact: true })).toBeVisible()
    await expect(page.getByText('Attention', { exact: true })).toBeVisible()
    await expect(page.getByText('Networks', { exact: true })).toBeVisible()
    await expect(page.getByRole('tablist', { name: 'Network filter' })).toBeVisible()
    await expect(page.getByRole('tab', { name: 'All Networks' })).toHaveAttribute('aria-selected', 'true')
    await expect(page.getByRole('combobox', { name: 'Sort' })).toBeVisible()
    await expect(page.getByRole('link', { name: 'Admin', exact: true })).toHaveAttribute('href', '/admin')
    await expectNoHorizontalOverflow(page)
  })

  test('Home controls remain semantic and touch-sized', async ({ page }) => {
    await loginAs(page)
    const home = page.getByRole('region', { name: 'Home' })
    await expect(home.getByRole('tab', { name: 'All Networks' })).toHaveAttribute('aria-selected', 'true')
    await home.getByRole('combobox', { name: 'Sort' }).selectOption('head')

    await expectVisibleInteractiveTargets(page)
    await expectNoHorizontalOverflow(page)
  })

  test('Admin shell fits the viewport without horizontal overflow', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expectShellFitsViewport(page, 'Overview')
  })

  test('Admin shell keeps the unified brand and operational proof semantic', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expectShellFitsViewport(page, 'Overview')

    const brand = page.getByRole('link', { name: 'PlatPulse', exact: true })
    await expect(brand).toHaveAttribute('href', '/')
    await expect(brand.locator('img')).toHaveAttribute('src', /platpulse-mark/)
    await expect(page.getByRole('button', { name: 'Sign out' })).toBeVisible()
    const adminNav = page.getByRole('navigation', { name: 'Admin', includeHidden: true })
    await expect(adminNav.getByRole('link', { name: 'Overview', includeHidden: true })).toHaveAttribute('aria-current', 'page')
    const attentionHeading = page.getByRole('heading', { level: 2, name: 'Attention queue' })
    await expect(attentionHeading).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Node Health Summary' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Agent inventory' })).toBeVisible()

    // Prove the user-visible Emerald light, translucent treatment through
    // semantic surfaces and WCAG contrast rather than exact CSS values or class names.
    const visual = await page.getByRole('banner').evaluate((banner, panelHeadingText) => {
      const panelHeading = Array.from(document.querySelectorAll('h2')).find(
        (heading) => heading.textContent?.trim() === panelHeadingText,
      )
      const panel = panelHeading?.closest('article')
      const adminNavigation = document.querySelector('nav[aria-label="Admin"]')
      const navigationLabel = Array.from(adminNavigation?.querySelectorAll('p') ?? []).find(
        (label) => label.textContent?.trim() === 'Operations',
      )
      const shell = banner.parentElement
      if (!panelHeading || !panel || !adminNavigation || !navigationLabel || !shell) {
        throw new Error('Admin visual proof surfaces are missing')
      }

      // Resolve through the browser rather than regex-parsing the string:
      // Tailwind's oklch tokens serialise as oklab(1 0 0 / 0.6), whose digits a
      // regex reads as near-black RGB.
      const parseColor = (value: string) => {
        const canvas = document.createElement('canvas')
        canvas.width = 1
        canvas.height = 1
        const context = canvas.getContext('2d')
        if (!context) throw new Error('a 2d context is required to read a colour')
        context.fillStyle = '#010203'
        context.fillStyle = value
        if (context.fillStyle === '#010203' && value.trim().toLowerCase() !== '#010203') {
          throw new Error(`Unsupported computed color: ${value}`)
        }
        context.clearRect(0, 0, 1, 1)
        context.fillRect(0, 0, 1, 1)
        const [red, green, blue, alpha] = context.getImageData(0, 0, 1, 1).data
        return { red, green, blue, alpha: alpha / 255 }
      }
      const luminance = ({ red, green, blue }: ReturnType<typeof parseColor>) => {
        const linear = [red, green, blue].map((channel) => {
          const normalized = channel / 255
          return normalized <= 0.04045
            ? normalized / 12.92
            : ((normalized + 0.055) / 1.055) ** 2.4
        })
        return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
      }
      const composite = (front: ReturnType<typeof parseColor>, back: ReturnType<typeof parseColor>) => ({
        red: front.red * front.alpha + back.red * (1 - front.alpha),
        green: front.green * front.alpha + back.green * (1 - front.alpha),
        blue: front.blue * front.alpha + back.blue * (1 - front.alpha),
        alpha: 1,
      })
      const contrast = (foreground: ReturnType<typeof parseColor>, background: ReturnType<typeof parseColor>) => {
        const foregroundLuminance = luminance(foreground)
        const backgroundLuminance = luminance(background)
        return (Math.max(foregroundLuminance, backgroundLuminance) + 0.05)
          / (Math.min(foregroundLuminance, backgroundLuminance) + 0.05)
      }
      const shellColor = parseColor(getComputedStyle(shell).backgroundColor)
      const bannerColor = parseColor(getComputedStyle(banner).backgroundColor)
      const panelColor = parseColor(getComputedStyle(panel).backgroundColor)
      const panelPaint = composite(panelColor, shellColor)
      const headingColor = composite(parseColor(getComputedStyle(panelHeading).color), panelPaint)
      const navigationColor = parseColor(getComputedStyle(adminNavigation).backgroundColor)
      const navigationLabelColor = composite(
        parseColor(getComputedStyle(navigationLabel).color),
        navigationColor,
      )
      return {
        bannerLuminance: luminance(composite(bannerColor, shellColor)),
        bannerAlpha: bannerColor.alpha,
        panelLuminance: luminance(panelPaint),
        panelAlpha: panelColor.alpha,
        panelHeadingContrast: contrast(headingColor, panelPaint),
        navigationLabelContrast: contrast(navigationLabelColor, navigationColor),
      }
    }, 'Attention queue')
    expect(visual.bannerLuminance).toBeGreaterThan(0.75)
    expect(visual.panelLuminance).toBeGreaterThan(0.75)
    expect(visual.bannerAlpha).toBeLessThan(1)
    expect(visual.panelAlpha).toBeLessThan(1)
    expect(visual.panelHeadingContrast).toBeGreaterThanOrEqual(4.5)
    expect(visual.navigationLabelContrast).toBeGreaterThanOrEqual(4.5)

    await page.goto('/admin/networks')
    await page.getByRole('button', { name: 'Register a Network' }).click()
    const validatorAddress = page.getByPlaceholder('0x…')
    await expect(validatorAddress).toBeVisible()
    const placeholderContrast = await validatorAddress.evaluate((input) => {
      // Resolve through the browser rather than regex-parsing the string:
      // Tailwind's oklch tokens serialise as oklab(1 0 0 / 0.6), whose digits a
      // regex reads as near-black RGB.
      const parseColor = (value: string) => {
        const canvas = document.createElement('canvas')
        canvas.width = 1
        canvas.height = 1
        const context = canvas.getContext('2d')
        if (!context) throw new Error('a 2d context is required to read a colour')
        context.fillStyle = '#010203'
        context.fillStyle = value
        if (context.fillStyle === '#010203' && value.trim().toLowerCase() !== '#010203') {
          throw new Error(`Unsupported computed color: ${value}`)
        }
        context.clearRect(0, 0, 1, 1)
        context.fillRect(0, 0, 1, 1)
        const [red, green, blue, alpha] = context.getImageData(0, 0, 1, 1).data
        return { red, green, blue, alpha: alpha / 255 }
      }
      const composite = (front: ReturnType<typeof parseColor>, back: ReturnType<typeof parseColor>) => ({
        red: front.red * front.alpha + back.red * (1 - front.alpha),
        green: front.green * front.alpha + back.green * (1 - front.alpha),
        blue: front.blue * front.alpha + back.blue * (1 - front.alpha),
        alpha: 1,
      })
      const luminance = ({ red, green, blue }: ReturnType<typeof parseColor>) => {
        const linear = [red, green, blue].map((channel) => {
          const normalized = channel / 255
          return normalized <= 0.04045
            ? normalized / 12.92
            : ((normalized + 0.055) / 1.055) ** 2.4
        })
        return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
      }
      const shell = document.querySelector('header')?.parentElement
      const panel = input.closest('article, [role="article"]')
      if (!shell || !panel) throw new Error('Admin form visual surfaces are missing')
      const shellColor = parseColor(getComputedStyle(shell).backgroundColor)
      const panelColor = composite(parseColor(getComputedStyle(panel).backgroundColor), shellColor)
      const inputColor = composite(parseColor(getComputedStyle(input).backgroundColor), panelColor)
      const placeholderColor = composite(
        parseColor(getComputedStyle(input, '::placeholder').color),
        inputColor,
      )
      const foreground = luminance(placeholderColor)
      const background = luminance(inputColor)
      return (Math.max(foreground, background) + 0.05)
        / (Math.min(foreground, background) + 0.05)
    })
    expect(placeholderContrast).toBeGreaterThanOrEqual(4.5)
    await expectNoHorizontalOverflow(page)
  })

  test('Admin connection status stays in shared header context', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()

    const status = page.getByRole('group', { name: 'Admin connection status' })
    await expect(status).toBeVisible()
    await expect(status).toContainText('Realtime')
    await expect(status).toContainText(/Current|Starting|Live updates paused/)

    const placement = await page.evaluate(() => {
      const header = document.querySelector('header')
      const main = document.querySelector('main')
      const status = document.querySelector('[role="group"][aria-label="Admin connection status"]')
      if (!header || !main || !status) throw new Error('Admin status placement surfaces are missing')
      const headerBox = header.getBoundingClientRect()
      const mainBox = main.getBoundingClientRect()
      const statusBox = status.getBoundingClientRect()
      return { headerTop: headerBox.top, headerBottom: headerBox.bottom, mainTop: mainBox.top, statusTop: statusBox.top, statusBottom: statusBox.bottom }
    })
    expect(placement.statusTop).toBeGreaterThanOrEqual(placement.headerTop - 1)
    expect(placement.statusBottom).toBeLessThanOrEqual(placement.headerBottom + 1)
    expect(placement.statusBottom).toBeLessThanOrEqual(placement.mainTop + 1)
    await expectNoHorizontalOverflow(page)
  })

  test('Admin shell keeps aligned content at fixed viewports', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()

    const metrics = await measureAdminWorkbench(page)
    expect(metrics.decorationCount).toBe(1)
    await expect(page.locator('[data-slot="background-decoration"]')).toHaveAttribute('aria-hidden', 'true')
    await expect(page.locator('[data-slot="background-decoration"]')).toHaveCSS('pointer-events', 'none')
    const expectedPadding = metrics.viewport >= 1024 ? 24 : 16
    expect(metrics.headingFontSize).toBeGreaterThanOrEqual(24)
    expect(metrics.headingFontSize).toBeLessThanOrEqual(28)
    expect(metrics.heading.x - metrics.main.x).toBeGreaterThanOrEqual(expectedPadding - 1)
    expect(metrics.heading.x - metrics.main.x).toBeLessThanOrEqual(expectedPadding + 1)

    if (metrics.viewport >= 768) {
      await expect(page.getByRole('columnheader', { name: 'Node', exact: true })).toBeVisible()
    }

    if (metrics.viewport >= 1024) {
      expect(metrics.nav.width).toBeGreaterThanOrEqual(208)
      expect(metrics.nav.width).toBeLessThanOrEqual(224)
      expect(metrics.header.height).toBeGreaterThanOrEqual(48)
      expect(metrics.header.height).toBeLessThanOrEqual(60)
      expect(Math.abs(metrics.main.x - metrics.nav.right)).toBeLessThanOrEqual(1)
    }
    await expectNoHorizontalOverflow(page)
  })

  test('Admin shell fills the shared sidebar workspace on wide screens', async ({ page }) => {
    test.skip((page.viewportSize()?.width ?? 0) < 1024, 'Ultrawide measurement belongs to the desktop project')
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()

    // ADR 0003 intentionally replaces the old centered 1280px Admin cap.
    // Assert available-space use and shared header/body alignment instead.
    for (const width of [1280, 1440, 1920, 2560]) {
      await page.setViewportSize({ width, height: 900 })
      const metrics = await measureAdminWorkbench(page)
      expect(Math.abs(metrics.page.width - (width - metrics.nav.width - 48))).toBeLessThanOrEqual(1)
      expect(Math.abs(metrics.page.x - metrics.nav.right - 24)).toBeLessThanOrEqual(1)
      const status = await page.getByRole('group', { name: 'Admin connection status' }).boundingBox()
      expect(Math.abs(status!.x + 24 - metrics.page.x)).toBeLessThanOrEqual(1)
      const brand = await page.locator('[data-slot="admin-brand"]').boundingBox()
      expect(Math.abs(brand!.width - metrics.nav.width)).toBeLessThanOrEqual(1)
      await expectNoHorizontalOverflow(page)
    }
  })

  test('Admin Settings keeps aligned content and touch targets', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await page.goto('/admin/settings')
    await expect(page.getByRole('heading', { level: 1, name: 'Settings' })).toBeVisible()
    await expect(page.getByRole('link', { name: /Admin overview/ })).toBeVisible()

    const geometry = await page.evaluate(() => {
      const heading = document.querySelector('[data-slot="settings-page"] > h1')
      const sections = document.querySelector('[data-slot="settings-sections"]')
      const breadcrumb = document.querySelector('[data-slot="settings-page"] > p:first-child a')
      if (!heading || !sections || !breadcrumb) throw new Error('Settings geometry surfaces are missing')
      const headingBox = heading.getBoundingClientRect()
      const sectionsBox = sections.getBoundingClientRect()
      const breadcrumbBox = breadcrumb.getBoundingClientRect()
      return { headingLeft: headingBox.left, sectionsLeft: sectionsBox.left, breadcrumbHeight: breadcrumbBox.height }
    })
    expect(Math.abs(geometry.headingLeft - geometry.sectionsLeft)).toBeLessThanOrEqual(1)
    expect(geometry.breadcrumbHeight).toBeGreaterThanOrEqual(44)
    await expectNoHorizontalOverflow(page)
  })

  test('Home and Admin navigation is reachable in both directions', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page).toHaveURL(/\/admin$/)
    await expectShellFitsViewport(page, 'Overview')

    await page.getByRole('link', { name: 'PlatPulse', exact: true }).click()
    await expect(page).toHaveURL(/\/$/)
    await expectShellFitsViewport(page, 'Home')
  })

  test('shell navigation is keyboard-operable with a visible focus ring', async ({ page }) => {
    await loginAs(page)

    // Tab from the brand to the Admin icon and verify the focus ring is visible.
    await page.keyboard.press('Tab')
    await expect(page.getByRole('link', { name: 'PlatPulse' })).toBeFocused()
    // The header also carries the theme control, so the Admin icon is not
    // necessarily the next tab stop. Walk to it, as the public shell's spec does.
    const adminLink = page.getByRole('link', { name: 'Admin', exact: true })
    let adminFocused = false
    for (let step = 0; step < 6 && !adminFocused; step += 1) {
      await page.keyboard.press('Tab')
      adminFocused = await adminLink.evaluate((element) => element === document.activeElement)
    }
    expect(adminFocused, 'the Admin icon is reachable by keyboard').toBe(true)
    await expectFocusedElementHasVisibleFocus(page)

    // Enter activates the focused link without a pointer.
    await page.keyboard.press('Enter')
    await expect(page).toHaveURL(/\/admin$/)
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()

    // Navigation remounts the layout, so find Sign out by tabbing around
    // the (small, wrapping) header focus order.
    const signOut = page.getByRole('button', { name: 'Sign out' })
    for (let tab = 0; tab < 6; tab += 1) {
      await page.keyboard.press('Tab')
      if (await signOut.evaluate((element) => element === document.activeElement)) {
        break
      }
    }
    await expect(signOut).toBeFocused()
    await expectFocusedElementHasVisibleFocus(page)
    await page.keyboard.press('Enter')
    await expect(page).toHaveURL(/\/login$/)
  })
})
