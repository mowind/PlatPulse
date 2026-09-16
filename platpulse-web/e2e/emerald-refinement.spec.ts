import { expect, test, type Page } from '@playwright/test'
import { mkdirSync, writeFileSync } from 'node:fs'
import { execSync } from 'node:child_process'
import { loginAs, expectNoHorizontalOverflow, expectVisibleInteractiveTargets } from './helpers'

async function fixture(page: Page, scenario = 'normal') {
  await page.route('**/api/public/v1/networks*', async route => {
    const response = await route.fetch()
    const original = await response.json()
    const first = original[0]
    const node = { ...first.nodes[0], displayName: scenario === 'long-name' ? 'Validator-Europe-Production-Extremely-Long-Node-Name' : 'Alpha', nodeDataDirectorySizeBytes: 12884901888, nodeDataDirectoryCapacityBytes: 51539607552, processUptimeMs: 345600000, hostNetworkRxBytesPerSec: 1048576, hostNetworkTxBytesPerSec: 2097152, health: 'healthy', freshness: 'current', hostCpuPercent: 23, hostMemoryPercent: 45, processCpuPercent: 12, processMemoryPercent: 24, currentHead: 123456, latestBlockTransactionCount: 98, consensus: { state: 'ok', freshness: 'current', validator: true, highestQcBlock: 123456, highestLockBlock: 123455, highestCommitBlock: 123454 } }
    const geo = { state: 'current', scope: 'complete', knownCountryCount: 4, unknownCountryCount: 0, countries: [{ countryCode: 'DE', count: 4, staleCount: 0, centroidLat: 51, centroidLon: 10 }] }
    if (scenario === 'unknown') Object.assign(node, { hostCpuPercent: null, hostMemoryPercent: null, processCpuPercent: null, processMemoryPercent: null, currentHead: null, consensus: null })
    if (scenario === 'long-value') Object.assign(node, { currentHead: 9007199254740991, latestBlockTransactionCount: 9007199254740991, consensus: { ...node.consensus, highestLockBlock: 9007199254740991, highestCommitBlock: 9007199254740991 } })
    if (scenario === 'stale' || scenario === 'error' || scenario === 'disabled' || scenario === 'unknown') geo.state = scenario
    if (scenario === 'partial') geo.scope = 'partial'
    const networks = [{ ...first, networkKey: 'mainnet', displayName: 'Mainnet', nodes: [node], geo, peers: { ...first.peers, freshness: 'current' } }, { ...first, networkKey: 'testnet', displayName: 'Testnet', nodes: [{ ...node, nodeId: 'beta', displayName: 'Beta', health: 'unknown' }, { ...node, nodeId: 'gamma', displayName: 'Gamma' }], geo: { ...geo, countries: [{ countryCode: 'FR', count: 7, staleCount: 0, centroidLat: 46, centroidLon: 2 }] }, peers: { ...first.peers, freshness: 'current' } }]
    await route.fulfill({ response, json: networks })
  })
  await page.emulateMedia({ reducedMotion: 'reduce', colorScheme: 'light' })
  await page.goto('/')
  await expect(page.getByRole('tab', { name: 'Testnet', exact: true })).toBeVisible()
  await page.emulateMedia({ reducedMotion: 'reduce', colorScheme: 'light' })
}

test('Radix selected paint, focus, keyboard and shared network scope', async ({ page }) => {
  await loginAs(page)
  await fixture(page)
  const all = page.getByRole('tab', { name: 'All Networks', exact: true })
  const main = page.getByRole('tab', { name: 'Mainnet', exact: true })
  for (const dark of [false, true]) {
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    await all.focus()
    await all.press('ArrowRight')
    await expect(main).toBeFocused()
    await expect(main).toHaveAttribute('data-state', 'active')
    expect(await main.evaluate(el => el.matches(':focus-visible'))).toBe(true)
    await expect(main).toHaveCSS('box-shadow', /3px/)
    await expectVisibleInteractiveTargets(page)
    const colors = await main.evaluate(el => ({ bg: getComputedStyle(el).backgroundColor, fg: getComputedStyle(el).color, inactive: getComputedStyle(el.nextElementSibling!).color }))
    expect(colors.bg).not.toBe('rgba(0, 0, 0, 0)')
    const expectedBackground = await page.evaluate(isDark => {
      const probe = document.createElement('span')
      probe.style.backgroundColor = isDark ? 'color-mix(in oklab, var(--input) 30%, transparent)' : 'var(--background)'
      document.body.append(probe)
      const color = getComputedStyle(probe).backgroundColor
      probe.remove()
      return color
    }, dark)
    expect(colors.bg).toBe(expectedBackground)
    expect(colors.fg).not.toBe(colors.inactive)
    const expectedColor = await page.evaluate(() => { const probe = document.createElement('span'); probe.className = 'text-emerald-600'; document.body.append(probe); const color = getComputedStyle(probe).color; probe.remove(); return color })
    expect(colors.fg).toBe(expectedColor)
    await expect(page.locator('[data-slot="summary-value"]').first()).toHaveText('1')
    await expect(page.locator('[data-slot="node-card"]')).toHaveCount(1)
    await expect(page.locator('[data-slot="geo-country-list"]')).toContainText('Germany')
    await main.press('End')
    await expect(page.getByRole('tab', { name: 'Testnet', exact: true })).toBeFocused()
    await expect(page.locator('[data-slot="summary-value"]').first()).toHaveText('2')
    await expect(page.locator('[data-slot="node-card"]')).toHaveCount(2)
    await expect(page.locator('[data-slot="geo-country-list"]')).toContainText('France')
    await page.keyboard.press('Home')
    await expect(all).toBeFocused()
    await main.scrollIntoViewIfNeeded()
    const rect = (await main.boundingBox())!
    // The visible 26px trigger has a real, unclipped 44px hit area.
    if (await page.evaluate(() => navigator.maxTouchPoints > 0)) await page.touchscreen.tap(rect.x + rect.width / 2, rect.y - 7)
    else await page.mouse.click(rect.x + rect.width / 2, rect.y - 7)
    await expect(main).toHaveAttribute('data-state', 'active')
    for (const tab of await page.getByRole('tab').all()) {
      await tab.scrollIntoViewIfNeeded()
      const hit = await tab.evaluate(el => { const r = el.getBoundingClientRect(); return [r.top - 8, r.bottom + 8].every(y => el.contains(document.elementFromPoint(r.left + r.width / 2, y))) })
      expect(hit, 'both edges of the 44px target hit this trigger').toBe(true)
    }
  }
})

test('refinement scenarios and measured evidence', async ({ page }, info) => {
  test.setTimeout(90_000)
  await loginAs(page)
  const measurements: Record<string, unknown> = {}
  for (const scenario of ['normal', 'unknown', 'stale', 'error', 'disabled', 'partial', 'long-name', 'long-value', 'multi-network']) {
    await page.unroute('**/api/public/v1/networks*')
    await fixture(page, scenario)
    if (scenario === 'multi-network') await page.getByRole('tab', { name: 'Testnet', exact: true }).click()
    if (['unknown', 'disabled'].includes(scenario)) await expect(page.locator('[data-slot="geo-chart"] canvas')).toHaveCount(0)
    else await expect(page.locator('[data-slot="geo-chart"] canvas').first()).toBeVisible()
    if (!['unknown', 'disabled'].includes(scenario)) await page.waitForFunction(() => {
      const canvas = document.querySelector('[data-slot="geo-chart"] canvas') as HTMLCanvasElement | null
      const context = canvas?.getContext('2d')
      return canvas && context && context.getImageData(0, 0, canvas.width, canvas.height).data.some((value, index) => index % 4 === 3 && value > 0)
    })
    await page.evaluate(() => document.fonts.ready)
    const geometry = await page.locator('[data-slot="tabs-list"], [data-slot="tabs-trigger"], select, header button, header a').evaluateAll(els => els.map(el => ({ slot: el.getAttribute('data-slot'), text: el.textContent, width: el.getBoundingClientRect().width, height: el.getBoundingClientRect().height, paintedHeight: el.getBoundingClientRect().height - parseFloat(getComputedStyle(el).borderTopWidth) - parseFloat(getComputedStyle(el).borderBottomWidth), fontSize: getComputedStyle(el).fontSize })))
    measurements[scenario] = geometry
    if (process.env.REFINEMENT_EVIDENCE) {
      const dir = `../docs/visual-migration/emerald/refinement/${process.env.REFINEMENT_EVIDENCE}/${info.project.name}`
      mkdirSync(dir, { recursive: true })
      await page.screenshot({ path: `${dir}/${scenario}.png`, fullPage: true })
      if (scenario === 'normal') {
        await page.evaluate(() => document.documentElement.classList.add('dark'))
        await page.screenshot({ path: `${dir}/normal-dark.png`, fullPage: true })
        await page.evaluate(() => document.documentElement.classList.remove('dark'))
      }
      writeFileSync(`${dir}/environment.json`, JSON.stringify({ commit: execSync('git rev-parse HEAD').toString().trim(), viewport: page.viewportSize(), browser: page.context().browser()?.version(), node: process.version, platform: process.platform, measurements }, null, 2))
    }
    if (!process.env.REFINEMENT_BASELINE) {
      await expectNoHorizontalOverflow(page)
      if (['stale', 'error', 'disabled', 'partial', 'unknown'].includes(scenario)) await expect(page.locator('[data-slot="map-status"]'), scenario).toBeVisible()
      else await expect(page.locator('[data-slot="map-status"]')).toHaveCount(0)
      const list = geometry.find(item => item.slot === 'tabs-list')!
      expect(list.height).toBe(32)
      expect(geometry.find(item => item.slot === 'select')?.height).toBe(44)
      expect(geometry.find(item => item.slot === 'select')?.paintedHeight).toBe(32)
      expect(geometry.find(item => item.slot === 'theme-toggle')?.height).toBe(44)
      expect(geometry.find(item => item.slot === 'theme-toggle')?.paintedHeight).toBe(32)
      expect(geometry.find(item => item.slot === 'admin-action')?.height).toBe(44)
      expect(geometry.find(item => item.slot === 'admin-action')?.paintedHeight).toBe(32)
      for (const item of geometry.filter(item => item.slot === 'tabs-trigger')) expect(item.height).toBe(26)
      const card = page.locator('[data-slot="node-card"]').first()
      await expect(card.locator('[data-slot="card-x-header"]')).toHaveCSS('padding', '12px 16px')
      await expect(card.locator('[data-slot="card-x-content"]')).toHaveCSS('padding', '0px 16px 16px')
      await expect(card.locator('h2')).toHaveCSS('font-size', '16px')
      if (scenario === 'unknown') {
        await expect(card.getByRole('progressbar')).toHaveCount(1) // Node data stays known
        await expect(card.locator('[data-slot="metric-unknown-space"]')).toHaveCount(2)
        await expect(card.locator('[data-slot="metric-row-value"]').first()).toHaveText('Unknown')
      }
      if (scenario === 'long-value') {
        const labels = page.locator('[data-slot="node-card"]').first().locator('[data-short-label]')
        await expect(labels.first()).toBeVisible()
        expect(await labels.evaluateAll(els => els.every(el => el.getBoundingClientRect().width > 0))).toBe(true)
        expect(await page.locator('[data-slot="metric-row-value"]').evaluateAll(els => els.every(el => el.scrollWidth <= el.clientWidth + 1))).toBe(true)
      }
    }
  }
})

test('shared chrome keeps business layout, bundled outline icons and background', async ({ page }, info) => {
  await page.goto('/login')
  await expect(page.getByRole('heading', { name: 'Sign in to PlatPulse' })).toBeVisible()
  const capture = async (name: string) => {
    if (!process.env.REFINEMENT_EVIDENCE) return
    const dir = `../docs/visual-migration/emerald/refinement/${process.env.REFINEMENT_EVIDENCE}/${info.project.name}`
    mkdirSync(dir, { recursive: true })
    for (const dark of [false, true]) {
      await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
      await page.screenshot({ path: `${dir}/${name}-${dark ? 'dark' : 'light'}.png`, fullPage: true })
    }
  }
  await capture('login')
  await loginAs(page)
  await page.goto('/admin')
  await expect(page.getByRole('heading', { name: 'Overview', exact: true })).toBeVisible()
  await expectNoHorizontalOverflow(page)
  if (!process.env.REFINEMENT_BASELINE) {
    await expect(page.locator('[data-slot="background-decoration"]')).toHaveCount(1)
    await expect(page.locator('[data-slot="admin-nav-icon"] svg')).toHaveCount(7)
  }
  await capture('admin')
})
