import { test, expect, type Page } from '@playwright/test'
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { loginAs } from './helpers'
import { findRepoRoot } from './repo-root'

// Opt-in matched evidence for the Admin information-layering round. Run once
// per revision with ADMIN_LAYERING_EVIDENCE=before|after against the local
// seeded e2e Server; no production service is touched and no credential is
// captured. The same local route fixture is applied in both phases so every
// capture holds the same data: the seeded normal (Node A), stale/RPC error and
// identity mismatch (Node B), never-observed Unknown (Nodes C, E, F, G),
// retired (Node D), non-zero recorded history (Agent 0011 gaps and security
// events), plus one long display name per inventory added by the fixture.
// These are captures, not baselines: the behavioural assertions live in
// admin-layering.spec.ts.
const phase = process.env.ADMIN_LAYERING_EVIDENCE
test.skip(!phase, 'Set ADMIN_LAYERING_EVIDENCE=before or after for matched captures')

const ROOT = join(findRepoRoot(), 'docs/visual-migration/emerald/admin-layering')
const NODE_A = '0195f2a1-0014-4014-8014-000000000014'
const NODE_C = '0195f2a1-0016-4016-8016-000000000016'
const AGENT = '0195f2a1-0011-4011-8011-000000000011'
const TARGET_AGENT = '0195f2a1-0021-4021-8021-000000000021'
export const LONG_NODE_NAME =
  'Archival validator sentinel with an unusually long operator-assigned display name for layout review'
export const LONG_AGENT_NAME = 'Frankfurt bare-metal reporting Agent with an unusually long display name'

const PAGES = [
  { name: 'overview', path: '/admin' },
  { name: 'nodes', path: '/admin/nodes' },
  { name: 'agents', path: '/admin/agents' },
  { name: 'validators', path: '/admin/validators' },
  { name: 'networks', path: '/admin/networks' },
  { name: 'node-detail', path: '/admin/nodes/' + NODE_A },
  { name: 'agent-detail', path: '/admin/agents/' + AGENT },
]
const WIDTHS = [390, 768, 1280, 1440]
const THEMES = ['light', 'dark'] as const

async function applyTheme(page: Page, theme: string) {
  await page.addInitScript((value) => localStorage.setItem('platpulse.themeMode', value), theme)
}

/** The long-name fixture: the real Server response with one display name
 *  lengthened, identical in both phases. */
export async function installLongNameFixture(page: Page) {
  await page.route(/\/api\/admin\/v1\/nodes(\?|$)/, async (route) => {
    const response = await route.fetch()
    const body = await response.json()
    for (const node of body as Array<Record<string, unknown>>) {
      if (node.node_id === NODE_C) node.display_name = LONG_NODE_NAME
    }
    await route.fulfill({ response, json: body })
  })
  await page.route(/\/api\/admin\/v1\/agents(\?|$)/, async (route) => {
    const response = await route.fetch()
    const body = await response.json()
    for (const agent of body as Array<Record<string, unknown>>) {
      if (agent.agent_id === TARGET_AGENT) agent.display_name = LONG_AGENT_NAME
    }
    await route.fulfill({ response, json: body })
  })
}

async function open(page: Page, path: string) {
  await page.goto(path)
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible({ timeout: 20_000 })
  // Let every independent Admin query settle before the capture.
  await expect(page.getByText(/^(Loading|Checking access)/)).toHaveCount(0, { timeout: 20_000 })
  await page.waitForTimeout(600)
}

async function capture(page: Page, out: string, name: string) {
  await page.screenshot({ path: out + '/' + name + '.png', fullPage: true, animations: 'disabled' })
}

const ERROR_BODY = '{"error":"unavailable","message":"Evidence: simulated refresh failure"}'

test('matched Admin layering evidence', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop-1280', 'one project drives every width')
  test.setTimeout(900_000)
  await loginAs(page)
  await installLongNameFixture(page)
  const out = ROOT + '/' + phase
  await mkdir(out, { recursive: true })
  const metrics: Record<string, unknown> = {}

  for (const theme of THEMES) {
    await applyTheme(page, theme)
    for (const width of WIDTHS) {
      await page.setViewportSize({ width, height: width < 768 ? 844 : 900 })
      for (const entry of PAGES) {
        await open(page, entry.path)
        // Default-view weight: page height, table header cells and visible
        // text length, measured the same way in both phases.
        metrics[entry.name + '-' + width + '-' + theme] = await page.evaluate(() => {
          const main = document.querySelector('main') ?? document.body
          return {
            height: document.documentElement.scrollHeight,
            columnHeaders: [...main.querySelectorAll('thead th')]
              .filter((th) => (th as HTMLElement).offsetParent !== null).length,
            visibleTextLength: (main as HTMLElement).innerText.length,
            overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
          }
        })
        await capture(page, out, entry.name + '-' + width + '-' + theme)
      }
    }
  }

  // Refresh failure keeps last-good content with an explicit error.
  await applyTheme(page, 'light')
  await page.setViewportSize({ width: 1280, height: 900 })
  await open(page, '/admin')
  const overviewQueries = /\/api\/admin\/v1\/(overview|agents|nodes)(\?|$)/
  await page.route(overviewQueries, (route) =>
    route.fulfill({ status: 503, contentType: 'application/json', body: ERROR_BODY }),
  )
  await page.locator('[data-slot="overview-header"]').getByRole('button', { name: 'Refresh' }).click()
  await expect(page.getByText('Failed to refresh; showing the last successful Agent values.')).toBeVisible({
    timeout: 30_000,
  })
  await capture(page, out, 'overview-refresh-error-1280-light')
  await page.unroute(overviewQueries)

  await writeFile(out + '/metrics.json', JSON.stringify(metrics, null, 2) + '\n')
})
