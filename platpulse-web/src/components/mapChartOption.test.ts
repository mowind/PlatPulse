import { describe, expect, it } from 'vitest'
import {
  DARK_PALETTE,
  LIGHT_PALETTE,
  MAP_DOT_MULTIPLE,
  MAP_DOT_SINGLE,
  flagSrc,
  mapChartOption,
  type MapCountry,
} from './mapChartOption'

type MapDatum = { name: string; value: number; itemStyle: { areaColor: string; borderColor: string; borderWidth: number } }
type ScatterDatum = { code: string; value: number[]; symbolSize: number; label: { show: boolean }; itemStyle: { color: string } }

const names = new Map([['SE', 'Sweden'], ['DE', 'Germany']])
const countries: MapCountry[] = [
  { code: 'SE', point: { lat: 60.1282, lon: 18.6435 }, count: 3, staleCount: 0 },
  { code: 'DE', point: { lat: 51, lon: 10 }, count: 2, staleCount: 1 },
  { code: 'XK', point: null, count: 1, staleCount: 0 },
]

function build(overrides: Partial<Parameters<typeof mapChartOption>[0]> = {}) {
  return mapChartOption({
    mapName: 'platpulse-world',
    regionNameByCode: names,
    countries,
    labelFor: (code) => code,
    dark: false,
    reducedMotion: false,
    ...overrides,
  }) as unknown as {
    animation: boolean
    geo: Record<string, unknown>
    series: Array<Record<string, unknown>>
    tooltip: { formatter: (params: unknown) => string; backgroundColor: string; trigger: string; confine: boolean }
  }
}

describe('mapChartOption', () => {
  it('keeps the geo component silent and transparent, exactly as upstream does', () => {
    const option = build()
    expect(option.geo.map).toBe('platpulse-world')
    expect(option.geo.roam).toBe(false)
    expect(option.geo.silent).toBe(true)
    expect(option.geo.left).toBe('center')
    expect(option.geo.top).toBe('center')
    // Both layers must auto-fit the available width AND height.
    for (const layer of [option.geo, option.series[0]]) {
      expect(layer.left).toBe('center')
      expect(layer.top).toBe('center')
      expect(layer.width).toBe('100%')
      expect(layer.height).toBe('100%')
      expect(layer.preserveAspect).toBe('contain')
    }
    expect(option.geo.itemStyle).toEqual({ areaColor: 'transparent', borderColor: 'transparent' })
    // Upstream sets no zoom, center, projection, scaleLimit or boundingCoords,
    // so every ECharts default applies.
    for (const key of ['zoom', 'center', 'projection', 'scaleLimit', 'boundingCoords']) {
      expect(option.geo[key], key + ' must stay an ECharts default').toBeUndefined()
    }
  })

  it('fills an observed country emerald, a stale country yellow, and leaves the rest on the base fill', () => {
    const option = build()
    const mapSeries = option.series[0]
    const data = mapSeries.data as MapDatum[]
    expect(mapSeries.type).toBe('map')
    expect(mapSeries.itemStyle).toEqual({ areaColor: LIGHT_PALETTE.areaColor, borderColor: LIGHT_PALETTE.borderColor, borderWidth: 0.5 })
    expect(mapSeries.label).toEqual({ show: false })
    expect(mapSeries.roam).toBe(false)
    expect(data.map((datum) => datum.name)).toEqual(['Sweden', 'Germany'])
    expect(data[0].itemStyle.areaColor).toBe(LIGHT_PALETTE.activeAreaColor)
    expect(data[0].itemStyle.borderColor).toBe(LIGHT_PALETTE.activeBorderColor)
    expect(data[1].itemStyle.areaColor).toBe(LIGHT_PALETTE.offlineAreaColor)
    expect(data[1].itemStyle.borderColor).toBe(LIGHT_PALETTE.offlineBorderColor)
  })

  it('never paints a country the vendored geometry cannot name', () => {
    const option = build()
    const data = option.series[0].data as MapDatum[]
    // Kosovo has data but no polygon name in this fixture.
    expect(data.some((datum) => datum.name === 'XK')).toBe(false)
  })

  it('sizes the dot by quantity and prints the aggregate numeral only above one record', () => {
    const option = build()
    const scatterSeries = option.series[1]
    const data = scatterSeries.data as ScatterDatum[]
    expect(scatterSeries.type).toBe('scatter')
    expect(scatterSeries.coordinateSystem).toBe('geo')
    expect(scatterSeries.symbol).toBe('circle')
    expect(scatterSeries.itemStyle).toEqual({ color: LIGHT_PALETTE.dotEmerald, borderColor: '#ffffff', borderWidth: 1 })
    // A country without a Server representative point is never plotted.
    expect(data.map((datum) => datum.code)).toEqual(['SE', 'DE'])
    expect(data[0].value).toEqual([18.6435, 60.1282, 3])
    expect(data[0].symbolSize).toBe(MAP_DOT_MULTIPLE)
    expect(data[1].symbolSize).toBe(MAP_DOT_MULTIPLE)
    expect(data[1].label.show).toBe(true)
    const singleRecord = build({ countries: [{ code: 'SE', point: { lat: 1, lon: 2 }, count: 1, staleCount: 0 }] })
    const singleData = singleRecord.series[1].data as ScatterDatum[]
    expect(singleData[0].symbolSize).toBe(MAP_DOT_SINGLE)
    expect(singleData[0].label.show).toBe(false)
    // Stale records take the yellow dot, never the emerald one.
    expect(data[1].itemStyle.color).toBe(LIGHT_PALETTE.dotYellow)
  })

  it('labels the aggregate in white 10px, as upstream does', () => {
    const option = build()
    const label = option.series[1].label as { fontSize: number; color: string; formatter: (params: { value?: unknown }) => string }
    expect(label.fontSize).toBe(10)
    expect(label.color).toBe('#ffffff')
    expect(label.formatter({ value: [1, 2, 42] })).toBe('42')
    expect(label.formatter({ value: undefined })).toBe('0')
  })

  it('renders the upstream tooltip box with a flag, the record count and the stale count', () => {
    const option = build()
    expect(option.tooltip.trigger).toBe('item')
    expect(option.tooltip.confine).toBe(true)
    expect(option.tooltip.backgroundColor).toBe(LIGHT_PALETTE.tooltipBg)
    const html = option.tooltip.formatter({ data: { code: 'DE', value: [10, 51, 2] } })
    expect(html).toContain(flagSrc('DE'))
    expect(html).toContain('/assets/flags/de.svg')
    expect(html).toContain('2 records')
    expect(html).toContain('1 stale')
    const se = option.tooltip.formatter({ data: { code: 'SE', value: [18, 60, 3] } })
    expect(se).toContain('3 records')
    expect(se).not.toContain('stale')
  })

  it('switches palette with the theme and honours reduced motion', () => {
    expect(build({ dark: true }).series[0].itemStyle).toEqual({ areaColor: DARK_PALETTE.areaColor, borderColor: DARK_PALETTE.borderColor, borderWidth: 0.5 })
    expect(build({ dark: true }).tooltip.backgroundColor).toBe(DARK_PALETTE.tooltipBg)
    expect(build().animation).toBe(true)
    expect(build({ reducedMotion: true }).animation).toBe(false)
  })
})
