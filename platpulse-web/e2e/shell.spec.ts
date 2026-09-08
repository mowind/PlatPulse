import { expect, test, type Page } from '@playwright/test'
import {
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow,
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
    const pageContent = main?.querySelector(':scope > .page')
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
      decorationCount: document.querySelectorAll('.background-decoration').length,
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
    await expect(page.getByRole('group', { name: 'Network filter' })).toBeVisible()
    await expect(page.getByRole('button', { name: 'All Networks' })).toHaveAttribute('aria-pressed', 'true')
    await expect(page.getByRole('combobox', { name: 'Sort' })).toBeVisible()
    await expect(page.getByRole('link', { name: 'Admin', exact: true })).toHaveAttribute('href', '/admin')
    await expectNoHorizontalOverflow(page)
  })

  test('Home controls remain semantic and touch-sized', async ({ page }) => {
    await loginAs(page)
    const home = page.getByRole('region', { name: 'Home' })
    await expect(home.getByRole('button', { name: 'All Networks' })).toHaveAttribute('aria-pressed', 'true')
    await home.getByRole('combobox', { name: 'Sort' }).selectOption('head')

    const undersized = await home.locator('button, a, select').evaluateAll((elements) => elements.flatMap((element) => {
      const rect = element.getBoundingClientRect()
      return rect.width < 44 || rect.height < 44 ? [element.textContent?.trim() || element.getAttribute('aria-label') || element.tagName] : []
    }))
    expect(undersized, 'Home interactive targets must be at least 44px').toEqual([])
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

      const parseColor = (value: string) => {
        const channels = value.match(/[\d.]+/g)?.map(Number) ?? []
        if (channels.length < 3) throw new Error(`Unsupported computed color: ${value}`)
        return { red: channels[0], green: channels[1], blue: channels[2], alpha: channels[3] ?? 1 }
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
      const parseColor = (value: string) => {
        const channels = value.match(/[\d.]+/g)?.map(Number) ?? []
        if (channels.length < 3) throw new Error(`Unsupported computed color: ${value}`)
        return { red: channels[0], green: channels[1], blue: channels[2], alpha: channels[3] ?? 1 }
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
      const panel = input.closest('article')
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
    expect(metrics.decorationCount).toBe(0)
    const expectedPadding = metrics.viewport >= 1024 ? 24 : 16
    expect(metrics.headingFontSize).toBeGreaterThanOrEqual(24)
    expect(metrics.headingFontSize).toBeLessThanOrEqual(28)
    expect(metrics.heading.x - metrics.main.x).toBeGreaterThanOrEqual(expectedPadding - 1)
    expect(metrics.heading.x - metrics.main.x).toBeLessThanOrEqual(expectedPadding + 1)

    if (metrics.viewport >= 768) {
      await expect(page.getByRole('columnheader', { name: 'Node' })).toBeVisible()
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

  test('Admin shell expands on ultrawide widths', async ({ page }) => {
    test.skip((page.viewportSize()?.width ?? 0) < 1024, 'Ultrawide measurement belongs to the desktop project')
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()

    const standard = await measureAdminWorkbench(page)
    await page.setViewportSize({ width: 2560, height: 800 })
    const ultrawide = await measureAdminWorkbench(page)
    expect(ultrawide.page.width).toBeGreaterThan(standard.page.width * 1.5)
    expect(Math.abs(ultrawide.heading.x - standard.heading.x)).toBeLessThanOrEqual(1)
    await expectNoHorizontalOverflow(page)
  })

  test('Admin Settings keeps aligned content and touch targets', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await page.goto('/admin/settings')
    await expect(page.getByRole('heading', { level: 1, name: 'Settings' })).toBeVisible()
    await expect(page.getByRole('link', { name: /Admin overview/ })).toBeVisible()

    const geometry = await page.evaluate(() => {
      const heading = document.querySelector('.settings-page > h1')
      const sections = document.querySelector('.settings-sections')
      const breadcrumb = document.querySelector('.settings-page > p:first-child a')
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
    await page.keyboard.press('Tab')
    await expect(page.getByRole('link', { name: 'Admin', exact: true })).toBeFocused()
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
