import { expect, test } from '@playwright/test'
import { loginAs, expectNoHorizontalOverflow } from './helpers'

// One desktop project drives the full geometry matrix; drawer/focus interactions
// remain covered in shell and admin-overview in the fixed touch projects.
test('Admin pages share shell alignment in both themes at every acceptance width', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop-1280', 'Single read-only geometry matrix')
  test.setTimeout(180_000)
  await loginAs(page)
  for (const width of [360, 390, 768, 1280, 1440, 1920]) {
    await page.setViewportSize({ width, height: 900 })
    for (const theme of ['light', 'dark']) {
      await page.evaluate(value => localStorage.setItem('platpulse.themeMode', value), theme)
      for (const route of ['', '/agents', '/nodes', '/networks', '/settings', '/access/sessions', '/access/audit']) {
        await page.goto(`/admin${route}`)
        await expect(page.locator('main h1')).toBeVisible()
        await expect(page.locator('[data-slot="admin-page"]')).not.toContainText('Loading…')
        const geometry = await page.evaluate(() => {
          const main = document.querySelector('main')!
          const content = document.querySelector('[data-slot="admin-page"]')!
          const root = content.firstElementChild!
          const heading = main.querySelector('h1')!
          return {
            main: main.getBoundingClientRect().x,
            heading: heading.getBoundingClientRect().x,
            root: root.getBoundingClientRect().width,
            content: content.getBoundingClientRect().width,
          }
        })
        expect(Math.abs(geometry.heading - geometry.main - (width >= 1024 ? 24 : 16)), `${width} ${theme} ${route}`).toBeLessThanOrEqual(1)
        expect(Math.abs(geometry.root - geometry.content)).toBeLessThanOrEqual(1)
        await expectNoHorizontalOverflow(page)
      }
    }
  }
})
