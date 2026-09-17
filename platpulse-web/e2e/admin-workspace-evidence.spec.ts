import { test, expect } from '@playwright/test'
import { access, mkdir, readFile, writeFile } from 'node:fs/promises'
import { loginAs, expectNoHorizontalOverflow } from './helpers'

// Opt-in evidence, not screenshot baselines. Browser-local frozen REST responses
// give both revisions the same data without writing to any production service.
const phase = process.env.ADMIN_WORKSPACE_EVIDENCE
test.skip(!phase, 'Set ADMIN_WORKSPACE_EVIDENCE=before or after for matched captures')
test('matched Admin workspace evidence', async ({ page }, info) => {
  test.setTimeout(180_000)
  const root = '../docs/visual-migration/emerald/admin-workspace'
  await mkdir(`${root}/fixtures`, { recursive: true })
  await loginAs(page)
  for (const name of ['overview', 'nodes', 'agents']) {
    const path = `${root}/fixtures/${name}.json`
    if (phase === 'before' && info.project.name === 'desktop-1280' && !(await access(path).then(() => true, () => false))) {
      const response = await page.request.get(`/api/admin/v1/${name}`)
      expect(response.ok()).toBe(true)
      await writeFile(path, JSON.stringify(await response.json(), null, 2))
    }
    const body = await readFile(path, 'utf8')
    await page.route(`**/api/admin/v1/${name}`, route => route.fulfill({ contentType: 'application/json', body }))
  }
  await page.clock.setFixedTime(new Date('2026-08-12T08:01:00Z'))
  for (const width of [360, 390, 768, 1280, 1440, 1920]) {
    await page.setViewportSize({ width, height: width < 768 ? 844 : 900 })
    for (const theme of ['light', 'dark']) {
      await page.addInitScript(value => localStorage.setItem('platpulse.themeMode', value), theme)
      await page.goto('/admin')
      await expect(page.getByRole('heading', { name: 'Node Health Summary' })).toBeVisible()
      await expect(page.getByRole('button', { name: 'Refresh', exact: true })).toBeEnabled()
      await page.evaluate(value => document.documentElement.classList.toggle('dark', value === 'dark'), theme)
      if (width === 1920) {
        const content = await page.locator('[data-slot="admin-page"]').boundingBox()
        if (phase === 'before') expect(content!.width).toBe(1280)
        else expect(content!.width).toBe(1656)
      }
      await expectNoHorizontalOverflow(page)
      await mkdir(`${root}/${phase}`, { recursive: true })
      await page.screenshot({ path: `${root}/${phase}/${width}-${theme}.png`, fullPage: true, animations: 'disabled' })
    }
  }
})
