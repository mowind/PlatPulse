// Run against e2e/start-server.sh, never against operator data. This is a
// geometry recorder, not an assertion suite. All five Node names are fixtures.
import { chromium } from 'playwright'

const baseURL = 'http://127.0.0.1:' + (process.env.E2E_PORT ?? '4173')
const browser = await chromium.launch({ headless: true })
try {
  const page = await browser.newPage({ baseURL })
  await page.goto('/')
  await page.getByLabel('Username').fill('admin')
  await page.getByLabel('Password').fill('platpulse-e2e-admin-2026')
  await page.getByRole('button', { name: 'Sign in' }).click()
  await page.locator('[data-slot="node-card"]').first().waitFor()
  const response = await page.request.get('/api/public/v1/networks')
  const [network] = await response.json()
  const base = network.nodes.find(node => node.validator?.blockCount != null)
  if (!base) throw new Error('Expected the test harness full Validator fixture')
  const complete = {
    ...base.validator, state: 'fresh', freshness: 'fresh', rank: 5, rankState: 'ranked', rankFreshness: 'fresh',
    blockCount: 123456, rewardAmount: '123456.78', blockRateState: 'ok',
    blockRate: '99.5', genBlocksRate: '99.2', delegationRewardPercentage: '20',
  }
  const empty = {
    ...base.validator, state: 'empty', freshness: 'fresh', blockCount: null,
    rewardAmount: null, rank: null, rankState: 'unranked', blockRate: null,
    blockRateState: 'unknown', genBlocksRate: null, delegationRewardPercentage: null,
  }
  const scenario = process.env.DENSITY_SCENARIO ?? 'normal'
  const nodes = ['Hydra', 'Chimera', 'Satyrs', 'Sync', 'Sync LEB'].map((displayName, index) => ({
    ...base, nodeId: 'density-' + index, displayName, health: 'healthy', healthReason: null,
    validator: index < 3 ? complete : empty,
  }))
  if (scenario === 'status') {
    Object.assign(nodes[0], { resyncState: 'resyncing', currentHead: 50, historicalHighWatermark: 100 })
    Object.assign(nodes[1], {
      health: 'unhealthy',
      healthReason: 'Collection failed. This longer sanitized diagnostic must wrap naturally without clipping any Node observations.',
      validator: { ...complete, state: 'error', freshness: 'stale' },
    })
    nodes[3].validator = { ...empty, state: 'loading' }
    nodes[4].validator = { ...empty, state: 'error' }
  }
  await page.route('**/api/public/v1/networks*', route => route.fulfill({
    json: [{ ...network, displayName: 'PlatON', nodes }],
  }))
  for (const width of [1440, 1280, 768, 390, 360]) {
    await page.setViewportSize({ width, height: 900 })
    await page.goto('/')
    await page.locator('[data-slot="node-card"]').first().waitFor()
    await page.evaluate(async fontSize => {
      if (fontSize) document.documentElement.style.fontSize = fontSize + 'px'
      await document.fonts.ready
      await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))
    }, Number(process.env.DENSITY_FONT_SIZE) || null)
    const cards = await page.locator('[data-slot="node-card"]').evaluateAll(elements => elements.map(card => {
      const rect = card.getBoundingClientRect()
      const box = slot => card.querySelector('[data-slot="' + slot + '"]').getBoundingClientRect()
      const meta = box('node-identity-meta')
      const resource = box('node-resource-region')
      const chain = box('node-business-metrics')
      const validator = box('linked-validator')
      const rows = [...card.querySelectorAll('[data-slot="node-business-metrics"] [data-slot="metric-row"]')]
        .map(el => ({ top: el.getBoundingClientRect().top - chain.top, height: el.getBoundingClientRect().height }))
      const overflow = [...card.querySelectorAll('[data-slot="metric-row-value"], [data-slot="metric-row-label"], [data-slot="validator-metric"] strong')]
        .filter(el => el.scrollWidth > el.clientWidth + 1).map(el => el.textContent)
      const region = card.querySelector('[data-node-region="validator"]')
      return {
        name: card.querySelector('h2').textContent, height: rect.height, width: rect.width,
        identity: box('card-x-header').height, uptimeToCpu: resource.top - meta.bottom,
        resource: resource.height, chain: chain.height, validator: validator.height,
        validatorAvailable: region?.getBoundingClientRect().height ?? validator.height,
        resourceTop: resource.top - rect.top, chainTop: chain.top - rect.top,
        validatorTop: validator.top - rect.top, rows, overflow,
        contentBeyondCard: Math.max(0, validator.bottom - rect.bottom),
        metricCells: card.querySelectorAll('[data-slot="linked-validator-metrics"] > *').length,
        emptyText: card.querySelector('[data-slot="linked-validator-empty"]')?.textContent ?? null,
      }
    }))
    console.log(JSON.stringify({ scenario, width, fontSize: process.env.DENSITY_FONT_SIZE ?? '16', cards }, null, 2))
    if (process.env.DENSITY_SCREENSHOTS && [1440, 360].includes(width)) {
      await page.locator('[data-slot="node-grid"]').screenshot({ path: process.env.DENSITY_SCREENSHOTS + '-' + width + '.png' })
    }
  }
} finally {
  await browser.close()
}
