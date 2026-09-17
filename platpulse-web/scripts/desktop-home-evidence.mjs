// UI-only frozen DTO fixtures; run before/after against the production bundle.
import { chromium, expect } from '@playwright/test'
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
const phase = process.argv[2] || 'after'
const dir = '../docs/visual-migration/desktop-home'
mkdirSync(dir, { recursive: true })
const source = JSON.parse(readFileSync('../docs/visual-migration/mobile-map/networks.json', 'utf8'))
const browser = await chromium.launch()
const page = await browser.newPage({ reducedMotion: 'reduce' })
// The long-running E2E Server caches index.html; use this build's entry HTML.
await page.route(/^http:\/\/127\.0\.0\.1:4173\/(?:login)?$/, route => route.fulfill({contentType:'text/html',body:readFileSync('dist/index.html','utf8')}))
await page.goto('http://127.0.0.1:4173/login')
await page.getByLabel('Username').fill('admin')
await page.getByLabel('Password').fill('platpulse-e2e-admin-2026')
await page.getByRole('button', { name: 'Sign in' }).click()
await page.waitForURL('http://127.0.0.1:4173/')
let fixture
await page.route('**/api/public/v1/networks*', route => route.fulfill({ json: fixture }))
await page.route('**/api/public/v1/events*', route => route.abort())
const results = []
for (const theme of ['light', 'dark']) for (const width of [360,390,768,1024,1280,1440,1920]) for (const count of [1,4,8]) {
 console.log(phase,theme,width,count)
 fixture = structuredClone(source.slice(0,1))
 const network = fixture[0]
 network.displayName = 'PlatON Mainnet'
 network.nodes = Array.from({length:count}, (_,i) => ({...structuredClone(source[0].nodes[i % source[0].nodes.length]), nodeId:`fixture-${i}`, displayName:i === 0 ? 'Validator — A Very Long Node Name That Must Not Overflow' : `Node ${i}`, ...(i % 2 === 0 ? {processCpuPercent:24.5,processMemoryPercent:61.2,nodeDataDirectorySizeBytes:120e9,nodeDataDirectoryCapacityBytes:500e9} : {})}))
 network.geo = {...network.geo,state:'current',scope:'complete',knownCountryCount:42,unknownCountryCount:0,availablePeerCount:42,countries:[{countryCode:'AU',count:42,staleCount:0,centroidLat:-25.27,centroidLon:133.77}]}
 network.peers.freshness = 'current'
 await page.emulateMedia({colorScheme:theme})
 await page.setViewportSize({width,height:900})
 await page.goto('http://127.0.0.1:4173/')
 await page.locator('[data-slot="geo-chart"] canvas').waitFor()
 await expect(page.locator('[data-slot="node-card"]')).toHaveCount(count)
 await page.waitForTimeout(180)
 const metrics = await page.evaluate(() => {
  const rect = el => {const r=el.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height,bottom:r.bottom}}
  const select=document.querySelector('select'), toolbar=document.querySelector('[aria-label="Node filters and sorting"]'), card=document.querySelector('[data-slot="node-card"]')
  const rows=[...card.querySelectorAll('[data-slot="metric-row"]')]; const data=rows.find(r=>r.textContent.startsWith('Node data'))
  const r=select.getBoundingClientRect()
  return {overflow:document.documentElement.scrollWidth>innerWidth,select:rect(select),toolbar:rect(toolbar),card:rect(card),data:rect(data),resources:rect(data.parentElement.closest('[aria-label]')),tabs:rect(document.querySelector('[aria-label="Network filter"]')),cpu:rect(rows[0]),memory:rect(rows[1]),dataParts:[...data.children].map(rect),business:[...card.querySelectorAll('[data-slot="node-business-metrics"] [data-slot="metric-row"]')].slice(0,4).map(rect),selectBackground:getComputedStyle(select).backgroundColor,selectHit:document.elementFromPoint(r.x+r.width/2,r.y+r.height/2)===select}
 })
 if(phase==='after') {
  expect(metrics.overflow).toBe(false);expect(metrics.selectHit).toBe(true)
  if(width>=768) {
   expect(Math.abs(metrics.data.width-metrics.resources.width)).toBeLessThan(1)
   expect(metrics.cpu.y).toBe(metrics.memory.y)
   expect(metrics.cpu.width).toBe(metrics.memory.width)
   expect(metrics.select.y+metrics.select.height/2).toBe(metrics.tabs.y+metrics.tabs.height/2)
   expect(metrics.selectBackground).not.toMatch(/rgba.*0\.6/)
   const [label,value,bar,detail]=metrics.dataParts
   expect(label.x).toBe(metrics.resources.x)
   expect(value.x+value.width).toBe(metrics.resources.x+metrics.resources.width)
   expect(bar.width).toBe(metrics.resources.width)
   expect(detail.x).toBe(label.x);expect(detail.y).toBeGreaterThan(bar.y)
   for(let i=1;i<metrics.business.length;i++)expect(metrics.business[i].y).toBeGreaterThan(metrics.business[i-1].y)
  }
  if(count>1) {
   const unknown=page.locator('[data-slot="node-card"]').filter({has:page.getByRole('heading',{name:'Node 1',exact:true})}).locator('[data-slot="node-data-resource"]')
   await expect(unknown).toContainText('Unknown')
   await expect(unknown.getByRole('progressbar')).toHaveCount(0)
  }
  if(width>=1024 && count===1) expect(metrics.card.width).toBeLessThan(width/2)
 }
 results.push({width,count,theme,...metrics})
 if (([390,1280,1440,1920].includes(width) && count===1) || (width===1440 && theme==='light')) await page.screenshot({path:`${dir}/${phase}-${width}-${count}-${theme}.png`,fullPage:true})
 if(width>=1024 && count===1) {
  // Locate the actual painted scatter pixels, then hover (not dispatchAction).
  const point=await page.locator('[data-slot="geo-chart"]').evaluate(el=>{
   for(const canvas of el.querySelectorAll('canvas')) {const ctx=canvas.getContext('2d'),d=ctx.getImageData(0,0,canvas.width,canvas.height).data,r=canvas.getBoundingClientRect();let sx=0,sy=0,n=0;
    for(let y=0;y<canvas.height;y++) for(let x=0;x<canvas.width;x++){const i=(y*canvas.width+x)*4;if(d[i+3]>110&&d[i+1]>d[i]+35&&d[i+1]>d[i+2]+15){sx+=x;sy+=y;n++}}
    if(n)return {x:r.x+sx/n/canvas.width*r.width,y:r.y+sy/n/canvas.height*r.height}
   }
  })
  await page.mouse.move(point.x,point.y);await page.waitForTimeout(250)
  const tip=page.getByText('Australia', {exact:true})
  const tooltipVisible=await tip.isVisible();results.at(-1).tooltipVisible=tooltipVisible
  if(phase==='after')expect(tooltipVisible).toBe(true)
  if(width===1440)await page.screenshot({path:`${dir}/${phase}-tooltip-${theme}.png`})
  await page.getByRole('combobox').click()
  await page.keyboard.press('Escape')
  await page.getByRole('combobox').focus()
  await page.keyboard.press('ArrowDown');await page.keyboard.press('Enter')
  await expect(page.getByRole('combobox')).toHaveValue('name')
  await page.keyboard.press('Alt+ArrowDown')
  if(width===1440)await page.screenshot({path:`${dir}/${phase}-sort-${theme}.png`})
  await page.keyboard.press('Escape')
  await page.getByRole('tab',{name:'PlatON Mainnet',exact:true}).click()
  await expect(page.locator('[aria-label="Peer countries"]')).toHaveAttribute('data-network-filter',network.networkKey)
  await expect(page.locator('[data-slot="node-card"]')).toHaveCount(count)
 }
}
writeFileSync(`${dir}/${phase}-metrics.json`,JSON.stringify(results,null,2))
await browser.close()
