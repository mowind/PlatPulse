// Run against e2e/start-server.sh; freeze the authenticated DTO once for both builds.
import { chromium } from '@playwright/test'
import { readFileSync, writeFileSync, mkdirSync, existsSync } from 'node:fs'
const phase = process.argv[2] || 'after'
const dir = '../docs/visual-migration/mobile-map'
mkdirSync(dir, { recursive: true })
const browser = await chromium.launch({ headless: true })
const page = await browser.newPage({ hasTouch: true, reducedMotion: 'reduce' })
await page.goto('http://127.0.0.1:4173/login')
await page.getByLabel('Username').fill('admin')
await page.getByLabel('Password').fill('platpulse-e2e-admin-2026')
await page.getByRole('button', { name: 'Sign in' }).click()
await page.waitForURL('http://127.0.0.1:4173/')
await page.goto('http://127.0.0.1:4173/admin/settings')
const provider = page.getByRole('article').filter({ has: page.getByRole('heading', { name: 'Geo provider' }) })
await provider.getByRole('radio', { checked: true }).waitFor()
if (!await provider.getByRole('radio', { name: 'Local MMDB' }).isChecked()) {
 await provider.getByRole('radio', { name: 'Local MMDB' }).check()
 await provider.getByRole('button', { name: 'Save Geo provider' }).click()
 await provider.getByText('Geo provider is now Local MMDB').waitFor()
}
const fixturePath = dir + '/networks.json'
if (!existsSync(fixturePath)) {
 const response = await page.request.get('http://127.0.0.1:4173/api/public/v1/networks')
 writeFileSync(fixturePath, JSON.stringify(await response.json(), null, 2))
}
const dense = process.argv.includes('--dense')
const fixture = JSON.parse(readFileSync(fixturePath, 'utf8'))
if (dense) {
 // Same explicitly labelled UI-only fixture as home-geo-map.spec.ts. Country
 // representative points are unchanged between builds; not Peer locations.
 const countries = [
  {countryCode:'BE',count:23,staleCount:0,centroidLat:50.85,centroidLon:4.35},
  {countryCode:'NL',count:42,staleCount:0,centroidLat:52.13,centroidLon:5.29},
  {countryCode:'DE',count:71,staleCount:3,centroidLat:51.16,centroidLon:10.45},
  {countryCode:'KR',count:83,staleCount:0,centroidLat:36.5,centroidLon:127.8},
  {countryCode:'JP',count:94,staleCount:0,centroidLat:36.2,centroidLon:138.25},
  {countryCode:'CN',count:1001,staleCount:0,centroidLat:35.86,centroidLon:104.2},
 ]
 const total = countries.reduce((sum,country) => sum+country.count,0)
 for (const network of fixture) {
  network.geo = {...network.geo,state:'current',scope:'complete',countries,knownCountryCount:total,unknownCountryCount:0,availablePeerCount:total}
  network.peers.freshness = 'current'
 }
}
const body = JSON.stringify(fixture)
await page.route('**/api/public/v1/networks*', route => route.fulfill({ contentType: 'application/json', body }))
// Keep both builds on exactly the frozen projection, not subsequent SSE updates.
await page.route('**/api/public/v1/events*', route => route.abort())
const results = []
for (const theme of ['light', 'dark']) {
 await page.emulateMedia({ colorScheme: theme })
 for (const [width, height] of (dense ? [[390,844]] : [[320,740],[360,800],[390,844],[430,900],[844,390],[1440,900]])) {
  await page.setViewportSize({ width, height })
  await page.goto('http://127.0.0.1:4173/')
  await page.locator('[data-slot="geo-chart"] canvas').waitFor()
  await page.waitForTimeout(600)
  const metrics = await page.evaluate(() => {
   const rect = selector => { const r = document.querySelector(selector).getBoundingClientRect(); return {x:r.x,y:r.y,width:r.width,height:r.height,bottom:r.bottom} }
   return { map: rect('[data-slot="geo-chart"]'), summary:rect('[aria-label="Home summary"]'), overflow: document.documentElement.scrollWidth > innerWidth }
  })
  results.push({width,height,theme,...metrics})
  await page.screenshot({ path: `${dir}/${phase}-${dense ? 'dense-' : ''}${width}-${theme}.png` })
 }
}
writeFileSync(`${dir}/${phase}-${dense ? 'dense-' : ''}metrics.json`, JSON.stringify(results,null,2))
await browser.close()
