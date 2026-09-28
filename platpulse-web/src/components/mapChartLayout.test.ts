// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { init, registerMap, use as registerEChartsModules, type ECharts } from 'echarts/core'
import { MapChart, ScatterChart } from 'echarts/charts'
import { GeoComponent, TooltipComponent } from 'echarts/components'
import { SVGRenderer } from 'echarts/renderers'
import world from '../../public/assets/geo/world-countries-echarts-www-v1.json'
import { WORLD_MAP_NAME } from '../worldGeometry'
import { mapChartOption } from './mapChartOption'

// Exercise ECharts' real layout with the shipped geometry, not a mocked option.
// SVG SSR uses the same geo layout as the browser's canvas renderer.
registerEChartsModules([MapChart, ScatterChart, GeoComponent, TooltipComponent, SVGRenderer])
registerMap(WORLD_MAP_NAME, world as unknown as Parameters<typeof registerMap>[1])

function positions(value: unknown): number[][] {
  if (!Array.isArray(value)) return []
  if (typeof value[0] === 'number' && typeof value[1] === 'number') return [[value[0], value[1]]]
  return value.flatMap(positions)
}
const points = world.features.flatMap((feature) => positions(feature.geometry.coordinates))

/**
 * Both layers fill the whole track box, so the world's full longitude span maps
 * to the canvas width and its full latitude span to the canvas height. Deriving
 * the height from the map's natural ~1.94:1 ratio instead overflowed the 2:1
 * phone/tablet track and cropped the poles, so the invariant is "every vertex
 * stays inside the box" plus the world filling both dimensions — nothing
 * cropped, nothing letterboxed.
 */
function expectWorldFillsBox(chart: ECharts, width: number, height: number) {
  expect(points.length).toBeGreaterThan(0)
  for (const finder of [{ seriesIndex: 0 }, { geoIndex: 0 }]) {
    const pixels = points.map((point) => chart.convertToPixel(finder, point))
    const xs = pixels.map((point) => point[0])
    const ys = pixels.map((point) => point[1])
    const spanX = Math.max(...xs) - Math.min(...xs)
    const spanY = Math.max(...ys) - Math.min(...ys)
    // The full -180..180 span maps to the width and the full latitude span to
    // the height; nothing is cropped or letterboxed on any edge.
    expect(spanX, 'world fills the canvas width').toBeGreaterThan(width - 1)
    expect(spanX, 'world never overflows the canvas width').toBeLessThan(width + 1)
    expect(spanY, 'world fills the canvas height').toBeGreaterThan(height - 1)
    expect(spanY, 'world never overflows the canvas height').toBeLessThan(height + 1)
    expect(Math.min(...xs), 'west edge').toBeGreaterThanOrEqual(-0.5)
    expect(Math.max(...xs), 'east edge').toBeLessThanOrEqual(width + 0.5)
    expect(Math.min(...ys), 'north edge').toBeGreaterThanOrEqual(-0.5)
    expect(Math.max(...ys), 'south edge').toBeLessThanOrEqual(height + 0.5)
  }
  for (const point of [[0, 80], [-70, -55], [175, -40], [18, 60]]) {
    expect(chart.convertToPixel({ geoIndex: 0 }, point)).toEqual(
      chart.convertToPixel({ seriesIndex: 0 }, point),
    )
  }
}

// Chart containers, not browser viewports: compact mobile, the desktop 2:1
// track, and the earlier shallow screenshot band.
const sizes = [[288, 144], [328, 164], [358, 179], [398, 199], [440, 200], [760, 200], [850, 200], [1330, 350], [760, 380], [1330, 665]]

describe.each([false, true])('world map viewport (dark=%s)', (dark) => {
  it.each(sizes)('fills %dx%d without cropping, then on resize', (width, height) => {
    const chart = init(null, undefined, { renderer: 'svg', ssr: true, width, height })
    try {
      chart.setOption(mapChartOption({
        mapName: WORLD_MAP_NAME,
        regionNameByCode: new Map(),
        countries: [],
        labelFor: (code) => code,
        dark,
        reducedMotion: true,
      }))
      expectWorldFillsBox(chart, width, height)
      // Exercise the mounted instance's resize path in both aspect ratios.
      for (const [nextWidth, nextHeight] of [[288, 144], [1330, 350], [width, height]]) {
        chart.resize({ width: nextWidth, height: nextHeight })
        expectWorldFillsBox(chart, nextWidth, nextHeight)
      }
    } finally {
      chart.dispose()
    }
  })
})
