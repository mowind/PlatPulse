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

function expectWorldFits(chart: ECharts, width: number, height: number) {
  expect(points.length).toBeGreaterThan(0)
  for (const finder of [{ seriesIndex: 0 }, { geoIndex: 0 }]) {
    const pixels = points.map((point) => chart.convertToPixel(finder, point))
    const xs = pixels.map((point) => point[0])
    const ys = pixels.map((point) => point[1])
    if (width / height === 2) {
      expect((Math.max(...xs) - Math.min(...xs)) / width, 'world fills 2:1 canvas').toBeGreaterThan(0.9)
    }
    expect(Math.min(...xs), 'west edge').toBeGreaterThanOrEqual(-0.000001)
    expect(Math.max(...xs), 'east edge').toBeLessThanOrEqual(width)
    expect(Math.min(...ys), 'north edge').toBeGreaterThanOrEqual(0)
    expect(Math.max(...ys), 'south edge').toBeLessThanOrEqual(height)
  }
  for (const point of [[0, 80], [-70, -55], [175, -40], [18, 60]]) {
    expect(chart.convertToPixel({ geoIndex: 0 }, point)).toEqual(
      chart.convertToPixel({ seriesIndex: 0 }, point),
    )
  }
}

// Chart containers, not browser viewports: compact mobile, shallow desktop,
// and the supplied screenshot's approximately 3.8:1 map aspect ratio.
const sizes = [[288, 144], [328, 164], [358, 179], [398, 199], [440, 200], [760, 200], [850, 200], [1330, 350], [760, 380], [1330, 665]]

describe.each([false, true])('world map viewport (dark=%s)', (dark) => {
  it.each(sizes)('fits every geometry point and aligns markers at %d×%d, then on resize', (width, height) => {
    const chart = init(null, undefined, { renderer: 'svg', ssr: true, width, height })
    try {
      chart.setOption(mapChartOption({
        mapName: WORLD_MAP_NAME,
        regionNameByCode: new Map(),
        countries: [],
        labelFor: (code) => code,
        dark,
        reducedMotion: true,
        mobile: width < 736,
      }))
      expectWorldFits(chart, width, height)
      // Exercise the mounted instance's resize path in both aspect ratios.
      for (const [nextWidth, nextHeight] of [[288, 144], [1330, 350], [width, height]]) {
        chart.resize({ width: nextWidth, height: nextHeight })
        expectWorldFits(chart, nextWidth, nextHeight)
      }
    } finally {
      chart.dispose()
    }
  })
})
