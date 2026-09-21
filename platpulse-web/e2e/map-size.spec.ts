import { expect, test } from '@playwright/test'
import { loginAs } from './helpers'

// Measure painted geography, not just the full-width canvas element.
test('desktop geography fills its map track without overlapping the overview', async ({ page }, testInfo) => {
  test.skip(test.info().project.name !== 'desktop-1440', 'Run the resize sequence once')
  await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' })
  await loginAs(page)
  await page.goto('/admin/settings')
  const provider = page.getByRole('article').filter({ has: page.getByRole('heading', { name: 'Geo provider' }) })
  await expect(provider.getByRole('radio', { checked: true })).toHaveCount(1)
  const local = provider.getByRole('radio', { name: 'Local MMDB' })
  if (!await local.isChecked()) {
    await local.check()
    await provider.getByRole('button', { name: 'Save Geo provider' }).click()
    await expect(provider.getByText('Geo provider is now Local MMDB')).toBeVisible()
  }
  for (const width of [1280, 1440, 2390]) {
    await page.setViewportSize({ width, height: 1097 })
    await page.goto('/')
    const canvas = page.locator('[data-slot="geo-chart"] canvas')
    await expect(canvas).toBeVisible()
    await expect.poll(async () => canvas.evaluate((element) => {
      if (!(element instanceof HTMLCanvasElement)) throw new Error('Expected map canvas')
      const context = element.getContext('2d')!
      const { data } = context.getImageData(0, 0, element.width, element.height)
      let left = element.width
      let right = -1
      for (let y = 0; y < element.height; y++) {
        for (let x = 0; x < element.width; x++) {
          if (data[(y * element.width + x) * 4 + 3] > 0) {
            left = Math.min(left, x)
            right = Math.max(right, x)
          }
        }
      }
      return (right - left + 1) / element.width
    }), { message: 'Painted world should occupy at least 85% of the map width', timeout: 3000 }).toBeGreaterThan(0.85)
    // The map keeps its own 2:1 track and stays clear of the statistics column
    // instead of spilling over the overview band.
    const geometry = await page.evaluate(() => {
      const summary = document.querySelector('[aria-label="Home summary"]')!.getBoundingClientRect()
      const map = document.querySelector('[data-slot="home-map"]')!.getBoundingClientRect()
      const chart = document.querySelector('[data-slot="geo-chart"]')!.getBoundingClientRect()
      return { summaryRight: summary.right, map, chart }
    })
    expect(geometry.map.width / geometry.map.height).toBeCloseTo(2, 0)
    expect(geometry.map.left).toBeGreaterThanOrEqual(geometry.summaryRight)
    expect(geometry.chart.right).toBeLessThanOrEqual(geometry.map.right + 1)
    expect(geometry.chart.bottom).toBeLessThanOrEqual(geometry.map.bottom + 1)
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    // Overflowing geography must not cover foreground controls or Node links.
    await page.getByRole('combobox', { name: 'Sort', exact: true }).click({ trial: true })
    await page.getByRole('combobox', { name: 'Sort', exact: true }).selectOption('name')
    await page.locator('[data-slot="node-card"] a').first().click({ trial: true })
    if (width === 1440) {
      await page.screenshot({ path: testInfo.outputPath('map-size-dark.png'), animations: 'disabled' })
    }
  }
})
