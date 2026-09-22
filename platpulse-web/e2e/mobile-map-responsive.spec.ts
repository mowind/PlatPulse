import { readFileSync } from 'node:fs'
import { expect, test } from '@playwright/test'
import { loginAs } from './helpers'

// Frozen real Public DTO from the before/after run; geo overrides below are
// explicitly UI fixtures, not production observations or relocated markers.
const frozen = JSON.parse(readFileSync('../docs/visual-migration/mobile-map/networks.json', 'utf8'))

test('mobile contain layout, touch targets, rotation and honest exception states', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'phone-390-touch', 'One serial viewport sequence')
  test.setTimeout(120_000)
  let networks = structuredClone(frozen)
  networks = [networks[1]]
  networks[0].geo = {
    ...networks[0].geo, state: 'current', scope: 'complete',
    availablePeerCount: 1001, knownCountryCount: 1001, unknownCountryCount: 0,
    countries: [{ countryCode: 'CN', count: 1001, staleCount: 0, centroidLat: 35.86, centroidLon: 104.2 }],
  }
  networks[0].peers.freshness = 'current'
  await page.route('**/api/public/v1/networks*', route => route.fulfill({ json: networks }))
  await page.route('**/api/public/v1/events*', route => route.abort())
  await page.emulateMedia({ reducedMotion: 'reduce' })
  await loginAs(page)
  const chart = page.locator('[data-slot="geo-chart"]')
  const canvas = chart.locator('canvas').first()
  await expect(canvas).toBeVisible()
  const originalCanvas = await canvas.elementHandle()
  for (const theme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: theme })
    for (const width of [320, 360, 390, 430]) {
      await page.setViewportSize({ width, height: 844 })
      await expect.poll(() => chart.evaluate(el => el.clientHeight)).toBe((width - 32) / 2)
      const map = (await chart.boundingBox())!
      const summary = (await page.locator('[aria-label="Home summary"]').boundingBox())!
      // Upstream places the complete map first and the six-card overview after
      // it on a phone, separated by the overview grid's own 16px gap.
      expect(summary.y - map.y - map.height).toBe(16)
      expect(summary.x).toBe(map.x)
      expect(summary.width).toBe(map.width)
      const overflow = await page.evaluate(() => [...document.querySelectorAll('*')].filter(el => el.getBoundingClientRect().right > innerWidth && getComputedStyle(el).position !== 'absolute').map(el => ({tag:el.tagName, cls:el.className, right:el.getBoundingClientRect().right})))
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), JSON.stringify(overflow)).toBe(true)
      const cards = await page.locator('[aria-label="Home summary"] > *').evaluateAll(elements => elements.map(el => { const r = el.getBoundingClientRect(); return {x:r.x,y:r.y} }))
      expect(cards[0].y).toBe(cards[1].y)
      expect(cards[2].y).toBe(cards[3].y)
      expect(cards[0].x).toBe(cards[2].x)
      await page.waitForTimeout(120)
      // Hit actual painted pixels. With the polygons silent (upstream's own
      // option), the opaque emerald marker is the only pointer target; the
      // translucent China polygon no longer opens a tooltip of its own.
      const point = await canvas.evaluate((el) => {
        const c = el as HTMLCanvasElement
        const ctx = c.getContext('2d')!
        const pixels = ctx.getImageData(0, 0, c.width, c.height).data
        const candidates: Array<{x:number;y:number}> = []
        for (let y = 0; y < c.height; y++) for (let x = 0; x < c.width; x++) {
          const i = (y*c.width+x)*4
          const [r,g,b,a] = pixels.slice(i,i+4)
          if (g > r+40 && g > b+20 && a > 150) candidates.push({x,y})
        }
        const p = candidates[Math.floor(candidates.length/2)]
        if (!p) return null
        const rect = c.getBoundingClientRect()
        return {x:rect.x+p.x/c.width*rect.width,y:rect.y+p.y/c.height*rect.height}
      })
      expect(point, 'marker').not.toBeNull()
      await page.touchscreen.tap(point!.x, point!.y)
      const tooltip = chart.locator('div').filter({ has: page.locator('img[src="/assets/flags/cn.svg"]') }).last()
      await expect(tooltip).toContainText('1001 records')
      await expect(tooltip).toBeVisible()
      const box = (await tooltip.boundingBox())!
      expect(box.x).toBeGreaterThanOrEqual(0)
      expect(box.x+box.width).toBeLessThanOrEqual(width)
      await page.touchscreen.tap(map.x+2,map.y+map.height-2)
      await expect(tooltip).not.toBeVisible()
    }
  }
  // Crossing md in both directions resizes/updates the existing instance.
  for (const [width,height] of [[844,390],[390,844],[700,320],[320,700]]) {
    await page.setViewportSize({width,height})
    await page.waitForTimeout(150)
    expect(await canvas.evaluate((el, original) => el === original, originalCanvas)).toBe(true)
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  }
  // A vertical gesture beginning over the map must scroll the page.
  const cdp = await page.context().newCDPSession(page)
  const mapBox = (await chart.boundingBox())!
  const startY = mapBox.y + mapBox.height - 10
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{x:160,y:startY}] })
  for (let i=1;i<=5;i++) {
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{x:160,y:startY-i*20}] })
  }
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] })
  await expect.poll(() => page.evaluate(() => scrollY)).toBeGreaterThan(0)
  await page.evaluate(() => scrollTo(0,0))
  await cdp.detach()
  // Filter changes must not alter the fit or recreate the canvas.
  const documentBox = () => chart.evaluate(el => { const r = el.getBoundingClientRect(); return {x:r.x, y:r.y+scrollY, width:r.width, height:r.height} })
  const before = await documentBox()
  await page.getByRole('tab', { name: networks[0].displayName, exact: true }).click()
  expect(await documentBox()).toEqual(before)
  expect(await canvas.evaluate((el, original) => el === original, originalCanvas)).toBe(true)
  await page.getByRole('tab', { name: 'All Networks', exact: true }).click()
  for (const state of ['stale','unknown','empty']) {
    if (state === 'empty') networks = []
    else if (state === 'stale') {
      networks[0].geo.state = 'stale'
      networks[0].geo.countries[0].staleCount = 1001
    } else {
      networks[0].geo = { ...networks[0].geo, state: 'unknown', scope: 'unobserved', countries: [], availablePeerCount: null, knownCountryCount: null, unknownCountryCount: null }
    }
    await page.reload()
    const map = page.getByRole('region', {name:'Peer countries'})
    await expect(map).toHaveAttribute('data-state', state)
    await expect(map.getByRole('status')).toContainText(state === 'stale' ? 'Map data stale' : state === 'empty' ? 'No data' : /No observations yet|Data unknown/)
    if (state === 'stale') await expect(page.locator('[data-slot="geo-country-list"]')).toContainText('1,001 stale')
  }
})
