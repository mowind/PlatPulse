import type { EChartsOption } from 'echarts'

/** Upstream's scatter sizes: 8px for a single record, 14px once a numeral prints. */
export const MAP_DOT_SINGLE = 8
export const MAP_DOT_MULTIPLE = 14

export type MapCountry = {
  code: string
  /** Server-provided representative point; null when the country has none. */
  point: { lat: number; lon: number } | null
  count: number
  staleCount: number
}

export type MapPalette = {
  areaColor: string
  borderColor: string
  hoverBorderColor: string
  activeAreaColor: string
  offlineAreaColor: string
  activeBorderColor: string
  offlineBorderColor: string
  dotEmerald: string
  dotYellow: string
  text: string
  textSecondary: string
  tooltipBg: string
  tooltipShadow: string
}

/**
 * Upstream's map palette, copied literally from
 * komari-theme-emerald@c2c5e88 src/components/NodeEarthMaps.vue.
 *
 * PlatPulse substitution: upstream tints a country by "has online servers" and
 * "has offline servers". A Peer country has no online/offline dimension — it
 * has observed records and stale records — so the emerald fill means "has Peer
 * records in scope" and the yellow fill means "some of those records are
 * stale". No count is invented and no state is added.
 */
export const LIGHT_PALETTE: MapPalette = {
  areaColor: 'rgba(15, 23, 42, 0.06)',
  borderColor: 'rgba(15, 23, 42, 0.06)',
  hoverBorderColor: 'rgba(5, 150, 105, 0.85)',
  activeAreaColor: 'rgba(16, 185, 129, 0.36)',
  offlineAreaColor: 'rgba(202, 138, 4, 0.22)',
  activeBorderColor: 'rgba(5, 150, 105, 0.92)',
  offlineBorderColor: 'rgba(202, 138, 4, 0.88)',
  dotEmerald: 'rgba(5, 150, 105, 0.9)',
  dotYellow: 'rgba(202, 138, 4, 0.9)',
  text: 'rgba(0, 0, 0, 0.85)',
  textSecondary: 'rgba(0, 0, 0, 0.55)',
  tooltipBg: 'rgba(255, 255, 255, 0.8)',
  tooltipShadow: 'rgba(0, 0, 0, 0.06)',
}

export const DARK_PALETTE: MapPalette = {
  areaColor: 'rgba(255, 255, 255, 0.08)',
  borderColor: 'rgba(255, 255, 255, 0.06)',
  hoverBorderColor: 'rgba(16, 185, 129, 0.9)',
  activeAreaColor: 'rgba(16, 185, 129, 0.52)',
  offlineAreaColor: 'rgba(234, 179, 8, 0.32)',
  activeBorderColor: 'rgba(16, 185, 129, 0.95)',
  offlineBorderColor: 'rgba(234, 179, 8, 0.8)',
  dotEmerald: 'rgba(16, 185, 129, 0.92)',
  dotYellow: 'rgba(234, 179, 8, 0.92)',
  text: 'rgba(255, 255, 255, 0.85)',
  textSecondary: 'rgba(255, 255, 255, 0.55)',
  tooltipBg: 'rgba(40, 40, 40, 0.95)',
  tooltipShadow: 'rgba(0, 0, 0, 0.4)',
}

/** Flag paths are lowercased on the wire; the vendored set is flag-icons. */
export const flagSrc = (code: string) => '/assets/flags/' + code.toLowerCase() + '.svg'

export type MapOptionInput = {
  mapName: string
  /** ISO2 to the ECharts region name, so a datum matches the right polygon. */
  regionNameByCode: Map<string, string>
  countries: MapCountry[]
  /** Localized country name for tooltips; never a fabricated place. */
  labelFor: (code: string) => string
  dark: boolean
  reducedMotion: boolean
  mobile?: boolean
}

/**
 * The ECharts option, mirroring upstream's `chartOption` literal: a silent,
 * transparent `geo` used only as the scatter's coordinate system; a `map`
 * series that paints the polygons; a `scatter` with upstream's symbol, 1px
 * white ring and 10px white aggregate numeral; and upstream's tooltip box.
 * Both layers fill the canvas with preserveAspect: 'contain': this avoids the
 * implicit 20% padding of automatic sizing without cropping or stretching the
 * world. Home supplies an independent desktop canvas, not the shallow summary
 * height. Zoom, geographic center, projection and scaleLimit keep their defaults.
 */
export function mapChartOption({
  mapName,
  regionNameByCode,
  countries,
  labelFor,
  dark,
  reducedMotion,
  mobile = false,
}: MapOptionInput): EChartsOption {
  const base = dark ? DARK_PALETTE : LIGHT_PALETTE
  const colors = mobile ? {
    ...base,
    borderColor: dark ? 'rgba(255,255,255,0.15)' : 'rgba(15,23,42,0.16)',
    activeAreaColor: dark ? 'rgba(16,185,129,0.18)' : 'rgba(16,185,129,0.14)',
    offlineAreaColor: dark ? 'rgba(234,179,8,0.15)' : 'rgba(202,138,4,0.12)',
    activeBorderColor: dark ? 'rgba(16,185,129,0.35)' : 'rgba(5,150,105,0.35)',
    offlineBorderColor: 'rgba(202,138,4,0.35)',
  } : base
  // Both coordinate systems must use the same contain fit, independent of data.
  const layout = {
    left: 'center', top: 'center', width: '100%', height: '100%',
    preserveAspect: 'contain' as const,
  }
  const byCode = new Map(countries.map(country => [country.code, country]))

  // One datum per country, matching ECharts' region name. A datum whose
  // polygon is absent from the asset simply does not paint; its Peer count
  // still reaches the user through the accessible country list.
  const mapData = countries.flatMap((country) => {
    const name = regionNameByCode.get(country.code)
    if (!name) return []
    const stale = country.staleCount > 0
    return [{
      name,
      code: country.code,
      value: country.count,
      itemStyle: {
        areaColor: stale ? colors.offlineAreaColor : colors.activeAreaColor,
        borderColor: stale ? colors.offlineBorderColor : colors.activeBorderColor,
        borderWidth: 0.5,
      },
      emphasis: {
        itemStyle: {
          areaColor: stale ? colors.offlineAreaColor : colors.activeAreaColor,
          borderColor: colors.hoverBorderColor,
          borderWidth: 0.5,
        },
      },
    }]
  })

  const scatterData = countries.flatMap((country) => {
    if (!country.point) return []
    const stale = country.staleCount > 0
    return [{
      name: labelFor(country.code),
      code: country.code,
      value: [country.point.lon, country.point.lat, country.count],
      symbolSize: mobile ? 7 : country.count <= 1 ? MAP_DOT_SINGLE : MAP_DOT_MULTIPLE,
      label: { show: !mobile && country.count > 1 },
      itemStyle: { color: stale ? colors.dotYellow : colors.dotEmerald },
    }]
  })

  return {
    animation: !mobile && !reducedMotion,
    animationDurationUpdate: 300,
    animationEasingUpdate: 'cubicOut',
    tooltip: {
      trigger: 'item',
      triggerOn: mobile ? 'click' : 'mousemove|click|mousewheel',
      enterable: false,
      transitionDuration: mobile ? 0 : 0.4,
      confine: true,
      backgroundColor: colors.tooltipBg,
      borderColor: 'transparent',
      borderWidth: 0,
      borderRadius: 6,
      textStyle: { color: colors.text, fontSize: 12, lineHeight: 20 },
      extraCssText:
        'max-width:calc(100vw - 48px);white-space:normal;overflow-wrap:anywhere;padding: 3px 6px;backdrop-filter: blur(5px);z-index:9;box-shadow:0 0 0 0.5px ' +
        colors.tooltipShadow + ', 0 0 16px ' + colors.tooltipShadow,
      formatter: (params) => {
        const single = Array.isArray(params) ? params[0] : params
        const data = (single as unknown as { data?: { code?: string } } | undefined)?.data
        const country = data?.code ? byCode.get(data.code) : undefined
        // Map values are scalars, scatter values are tuples. Resolve both via
        // the same observed country; an unmatched region is NOT zero records.
        if (!country) return ''
        const { code, count } = country
        const flag = '<img src="' + flagSrc(code) + '" style="width:16px;height:16px;vertical-align:middle;margin-right:2px" />'
        const dot = (color: string, label: string) =>
          '<span style="display:flex;gap:4px;align-items:center"><span style="display:inline-block;width:6px;height:6px;border-radius:50%;background:' +
          color + '"></span> ' + label + '</span>'
        const stale = country && country.staleCount > 0
          ? dot(colors.dotYellow, String(country.staleCount) + ' stale')
          : ''
        return (
          '<div style="line-height:1.4"><span style="display:flex;gap:2px;align-items:center;">' +
          flag + labelFor(code) +
          '</span><div style="display:flex;gap:8px;align-items:center;color:' + colors.textSecondary + '">' +
          dot(colors.dotEmerald, String(count) + ' records') + stale +
          '</div></div>'
        )
      },
    },
    geo: {
      map: mapName,
      roam: false,
      ...layout,
      silent: true,
      itemStyle: { areaColor: 'transparent', borderColor: 'transparent' },
      emphasis: {
        itemStyle: { areaColor: 'transparent', borderColor: 'transparent' },
        label: { show: false },
      },
      label: { show: false },
    },
    series: [
      {
        type: 'map',
        map: mapName,
        roam: false,
        selectedMode: false,
        ...layout,
        tooltip: { show: true },
        emphasis: {
          label: { show: false },
          itemStyle: { areaColor: colors.borderColor, borderColor: colors.hoverBorderColor, borderWidth: 0.5 },
        },
        itemStyle: { areaColor: colors.areaColor, borderColor: colors.borderColor, borderWidth: 0.5 },
        data: mapData,
        label: { show: false },
      },
      {
        type: 'scatter',
        coordinateSystem: 'geo',
        data: scatterData,
        symbol: 'circle',
        itemStyle: { color: colors.dotEmerald, borderColor: '#ffffff', borderWidth: 1 },
        label: {
          fontSize: 10,
          color: '#ffffff',
          formatter: (params: { value?: unknown }) => {
            const value = params.value
            if (!Array.isArray(value)) return '0'
            const total = value[2]
            return String(typeof total === 'number' ? total : 0)
          },
        },
        emphasis: { scale: 1.3 },
      },
    ],
  }
}
