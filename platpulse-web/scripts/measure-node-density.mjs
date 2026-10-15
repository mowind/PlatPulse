// Controlled Public Home fixtures against e2e/start-server.sh, never operator data.
// This records geometry/screenshots; routed browser specs provide assertions.
import { chromium } from '@playwright/test'
import { mkdir, writeFile } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'

const baseURL = 'http://127.0.0.1:' + (process.env.E2E_PORT ?? '4173')
const matrix = process.env.DENSITY_MATRIX === '1'
const output = process.env.DENSITY_OUTPUT && resolve(process.env.DENSITY_OUTPUT)
const screenshotPrefix = process.env.DENSITY_SCREENSHOTS && resolve(process.env.DENSITY_SCREENSHOTS)
const viewports = [
  { width: 360, height: 800 }, { width: 390, height: 844 },
  { width: 768, height: 1024 }, { width: 1280, height: 800 }, { width: 1440, height: 900 },
]
const modes = matrix ? [
  { scenario: 'normal', fontSize: 16, reducedMotion: 'no-preference' },
  { scenario: 'status', fontSize: 16, reducedMotion: 'no-preference' },
  { scenario: 'normal', fontSize: 20, reducedMotion: 'no-preference' },
  { scenario: 'normal', fontSize: 16, reducedMotion: 'reduce' },
] : [{ scenario: process.env.DENSITY_SCENARIO ?? 'normal', fontSize: Number(process.env.DENSITY_FONT_SIZE) || 16, reducedMotion: 'no-preference' }]
const records = []
const browser = await chromium.launch({ headless: true })
try {
  const page = await browser.newPage({ baseURL })
  await page.goto('/')
  await page.getByLabel('Username').fill('admin')
  await page.getByLabel('Password').fill(process.env.PLATPULSE_E2E_PASSWORD ?? 'platpulse-e2e-admin-2026')
  await page.getByRole('button', { name: 'Sign in' }).click()
  await page.locator('[data-slot="node-card"]').first().waitFor()
  const networks = await (await page.request.get('/api/public/v1/networks')).json()
  const network = networks.find(item => item.nodes.some(node => node.validator?.blockCount != null))
  const source = network?.nodes.find(node => node.validator?.blockCount != null)
  if (!source) throw new Error('Expected the isolated harness full Validator fixture')
  const complete = {
    ...source.validator, currentValidatorStatus: 'validator', currentValidatorStatusState: 'current',
    state: 'ok', freshness: 'fresh', rank: 2, rankState: 'ranked', rankFreshness: 'fresh',
    blockCount: 255, rewardAmount: '32', activity: 'active', activityState: 'current',
    blockRateState: 'ok', blockRate: '99.5', genBlocksRate: '99.2', delegationRewardPercentage: '20',
  }
  const base = {
    ...source, health: 'healthy', healthReason: 'Fresh component observations',
    rpcState: 'ok', syncState: 'ok', freshness: new Date().toISOString(), resyncState: 'normal',
    currentHead: 1000, historicalHighWatermark: 2000, networkReferenceHead: 1000, networkReferenceConfidence: 'high',
    processCpuPercent: 12, processMemoryPercent: 24, processUptimeMs: 3_600_000,
    nodeDataDirectorySizeBytes: 25_000, nodeDataDirectoryCapacityBytes: 100_000,
    hostNetworkTxBytesPerSec: 1250, hostNetworkRxBytesPerSec: 2500,
    peers: { ...source.peers, peerCount: 3, state: 'ok', freshness: 'current' }, validator: complete,
  }
  let scenario = 'normal'
  await page.route('**/api/public/v1/networks', route => {
    const nodes = ['Hydra', 'Chimera', 'Satyrs', 'Lyra', 'Draco', 'Orion', 'Cygnus'].map((displayName, index) => ({
      ...base, nodeId: 'density-' + index, displayName,
    }))
    if (scenario === 'status') {
      Object.assign(nodes[0], { resyncState: 'resyncing', resyncLastProgressAt: new Date().toISOString() })
      Object.assign(nodes[1], {
        displayName: 'Chimera — long readable name with independent unhealthy diagnostics', health: 'unhealthy',
        healthReason: 'RPC observation failed. This sanitized diagnostic deliberately wraps across several readable lines without stretching unrelated grid rows.',
        validator: { ...complete, state: 'error', freshness: 'stale', rewardAmount: '123456789012345678901234567890.123456789012' },
      })
      nodes[2].validator = { ...complete, blockCount: null, rewardAmount: null, rank: null, rankState: 'unknown', rankFreshness: 'unknown' }
      nodes[3].validator = { ...complete, currentValidatorStatus: 'not_validator', currentValidatorStatusState: 'current' }
      nodes[4].validator = undefined
      nodes[5].validator = { ...complete, currentValidatorStatus: 'not_validator', currentValidatorStatusState: 'stale' }
    }
    return route.fulfill({ json: [{ ...network, displayName: 'Mainnet', nodes }] })
  })
  for (const mode of modes) {
    scenario = mode.scenario
    await page.emulateMedia({ reducedMotion: mode.reducedMotion })
    for (const viewport of viewports) {
      await page.setViewportSize(viewport)
      for (const dark of matrix ? [false, true] : [false]) {
        await page.goto('/?sort=name')
        await page.locator('[data-slot="node-card"]').last().waitFor()
        await page.evaluate(async ({ dark, fontSize }) => {
          document.documentElement.classList.toggle('dark', dark)
          document.documentElement.style.fontSize = fontSize + 'px'
          await document.fonts.ready
          await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))
        }, { dark, fontSize: mode.fontSize })
        await page.mouse.move(0, 0)
        const geometry = await page.locator('[data-slot="node-card"]').evaluateAll(elements => ({
          pageOverflow: document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
          motionReduced: matchMedia('(prefers-reduced-motion: reduce)').matches,
          cards: elements.map(card => {
            const rect = card.getBoundingClientRect()
            const box = slot => card.querySelector('[data-slot="' + slot + '"]')?.getBoundingClientRect()
            const validator = box('linked-validator')
            const group = card.querySelector('[data-slot="linked-validator-metrics"]')
            const summary = [...card.querySelectorAll('[data-slot="validator-summary-cell"]')]
            const overflow = [...card.querySelectorAll('[data-slot="metric-row-value"], [data-slot="resource-value"], [data-slot="resource-label"], [data-slot="validator-summary-value"], [data-slot="validator-summary-label"]')]
              .filter(el => el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1).map(el => el.textContent)
            return {
              name: card.querySelector('h2').textContent, y: rect.y, bottom: rect.bottom, height: rect.height, width: rect.width,
              identity: box('card-x-header')?.height, metadata: box('node-identity-meta')?.height,
              resource: box('node-resource-region')?.height, chain: box('node-business-metrics')?.height,
              validator: validator?.height, summaryColumns: group ? getComputedStyle(group).gridTemplateColumns.split(' ').length : 0,
              summaryRows: new Set(summary.map(cell => Math.round(cell.getBoundingClientRect().top))).size,
              metricCells: summary.length, overflow, contentBeyondCard: Math.max(0, (validator?.bottom ?? rect.bottom) - rect.bottom),
              identityText: card.querySelector('[data-slot="linked-validator-identity"]')?.textContent ?? null,
              headComparison: card.querySelector('[data-slot="node-head-line"]')?.textContent ?? null,
              peerCue: card.querySelector('[data-slot="node-peers-cue"]')?.textContent ?? null,
            }
          }),
        }))
        const record = { scenario, ...viewport, theme: dark ? 'dark' : 'light', fontSize: mode.fontSize, reducedMotion: mode.reducedMotion, ...geometry }
        records.push(record)
        console.log(JSON.stringify(record))
        if (screenshotPrefix && (mode.fontSize === 16 && mode.reducedMotion !== 'reduce')) {
          const path = screenshotPrefix + '-' + scenario + '-' + viewport.width + '-' + record.theme + '.png'
          await mkdir(dirname(path), { recursive: true })
          // An element screenshot scrolls the grid beneath the sticky header on
          // phones. Capture its document-space clip from scroll zero instead: no
          // hidden UI or altered layout, and no header painted over the first row.
          await page.evaluate(() => window.scrollTo(0, 0))
          const clip = await page.locator('[data-slot="node-grid"]').boundingBox()
          if (!clip) throw new Error('Expected a visible Node grid for capture')
          await page.screenshot({ path, clip, fullPage: true })
        }
      }
    }
  }
  if (output) {
    await mkdir(dirname(output), { recursive: true })
    await writeFile(output, JSON.stringify({ capturedAt: new Date().toISOString(), fixture: 'controlled Public Home; no operator data', records }, null, 2) + '\n')
  }
} finally {
  await browser.close()
}
