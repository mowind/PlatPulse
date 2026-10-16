import { test, expect, type Page } from '@playwright/test'
import { mkdir, writeFile } from 'node:fs/promises'
import { loginAs } from './helpers'

// Opt-in evidence for the Admin consistency refinement, not screenshot
// baselines. Run once on the baseline revision (before) and once after the
// change against the same seeded local e2e Server; no production service is
// touched and no credential is captured.
const phase = process.env.ADMIN_REFINEMENT_EVIDENCE
test.skip(!phase, 'Set ADMIN_REFINEMENT_EVIDENCE=before or after for matched captures')

const ROOT = '../docs/visual-migration/emerald/admin-refinement'
const PAGES = [
  { name: 'overview', path: '/admin', heading: 'Overview' },
  { name: 'nodes', path: '/admin/nodes', heading: 'Nodes' },
  { name: 'agents', path: '/admin/agents', heading: 'Agents' },
  { name: 'networks', path: '/admin/networks', heading: 'Networks' },
  { name: 'validators', path: '/admin/validators', heading: 'Validators' },
  { name: 'audit', path: '/admin/access/audit', heading: 'Audit' },
  { name: 'settings', path: '/admin/settings', heading: 'Settings' },
]
const WIDTHS = [360, 390, 768, 1280, 1440]

async function open(page: Page, path: string, heading: string, theme: string) {
  await page.goto(path)
  await expect(page.getByRole('heading', { level: 1, name: heading })).toBeVisible({ timeout: 20_000 })
  await page.evaluate(value => document.documentElement.classList.toggle('dark', value === 'dark'), theme)
  await page.waitForLoadState('networkidle').catch(() => undefined)
}

test('matched Admin refinement evidence', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop-1280', 'one project drives every width')
  test.setTimeout(600_000)
  await loginAs(page)
  const out = ROOT + '/' + phase
  await mkdir(out, { recursive: true })
  const geometry: Record<string, unknown> = {}
  for (const theme of ['light', 'dark']) {
    await page.addInitScript(value => localStorage.setItem('platpulse.themeMode', value), theme)
    for (const width of WIDTHS) {
      await page.setViewportSize({ width, height: width < 768 ? 844 : 900 })
      for (const entry of PAGES) {
        await open(page, entry.path, entry.heading, theme)
        if (entry.name === 'audit') {
          geometry['audit-' + width + '-' + theme] = await page.evaluate(() => {
            const table = document.querySelector<HTMLElement>('[data-slot="audit-table"]')
            if (!table) return null
            const parent = table.parentElement!
            return { table: Math.round(table.getBoundingClientRect().width), container: Math.round(parent.clientWidth), scroll: parent.scrollWidth, doc: document.documentElement.scrollWidth, viewport: window.innerWidth }
          })
        }
        await page.screenshot({ path: out + '/' + entry.name + '-' + width + '-' + theme + '.png', fullPage: true, animations: 'disabled' })
      }
    }
  }
  // Expanded details and a refresh failure, light theme, phone and desktop.
  await page.addInitScript(() => localStorage.setItem('platpulse.themeMode', 'light'))
  for (const width of [390, 1280]) {
    await page.setViewportSize({ width, height: width < 768 ? 844 : 900 })
    await open(page, '/admin/nodes', 'Nodes', 'light')
    const nodeToggle = page.locator('[data-slot="node-table"] button[aria-expanded="false"]').first()
    if (await nodeToggle.count()) await nodeToggle.click()
    await page.screenshot({ path: out + '/nodes-expanded-' + width + '-light.png', fullPage: true, animations: 'disabled' })
    await open(page, '/admin/agents', 'Agents', 'light')
    const agentToggle = page.locator('[data-slot="agent-table"] button[aria-expanded="false"]').first()
    if (await agentToggle.count()) await agentToggle.click()
    await page.screenshot({ path: out + '/agents-expanded-' + width + '-light.png', fullPage: true, animations: 'disabled' })
    await open(page, '/admin/access/audit', 'Audit', 'light')
    const auditToggle = page.locator('[data-slot="audit-table"] button[aria-expanded="false"]').first()
    if (await auditToggle.count()) await auditToggle.click()
    await page.screenshot({ path: out + '/audit-expanded-' + width + '-light.png', fullPage: true, animations: 'disabled' })
    await open(page, '/admin/networks', 'Networks', 'light')
    const register = page.getByRole('button', { name: /Register a Network/ })
    if (await register.count()) await register.first().click()
    await page.screenshot({ path: out + '/networks-form-' + width + '-light.png', fullPage: true, animations: 'disabled' })
  }
  await page.setViewportSize({ width: 1280, height: 900 })
  await page.route('**/api/admin/v1/nodes**', route => route.fulfill({ status: 503, contentType: 'application/json', body: '{"error":"unavailable","message":"Evidence: simulated refresh failure"}' }))
  await page.goto('/admin/nodes')
  await expect(page.getByRole('heading', { level: 1, name: 'Nodes' })).toBeVisible()
  await page.waitForTimeout(1500)
  await page.screenshot({ path: out + '/nodes-error-1280-light.png', fullPage: true, animations: 'disabled' })
  await writeFile(out + '/geometry.json', JSON.stringify(geometry, null, 2) + '\n')
})
