// THROWAWAY: Three Node Detail layouts on /prototype/emerald/?variant=A|B|C.
// Question: which Emerald hierarchy makes Node diagnosis easiest? No auth or API; only theme preference persists.
import { useEffect, useLayoutEffect, useState, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter, useSearchParams } from 'react-router'
import '@fontsource-variable/inter'
import './prototype.css'

type ThemeMode = 'auto' | 'light' | 'dark'
const themeStorageKey = 'platpulse.emerald-prototype.themeMode'
const themeLabels: Record<ThemeMode, string> = { auto: '跟随系统', light: '浅色', dark: '深色' }
const themeIcons: Record<ThemeMode, string> = { auto: '◐', light: '☀', dark: '☾' }
const nextThemeModes: Record<ThemeMode, ThemeMode> = { auto: 'light', light: 'dark', dark: 'auto' }
function initialThemeMode(): ThemeMode {
  const mode = document.documentElement.dataset.themeMode
  return mode === 'light' || mode === 'dark' ? mode : 'auto'
}

type Scenario = 'normal' | 'stale' | 'failure' | 'unknown'
type Props = { scenario: Scenario }
const variants = ['A', 'B', 'C']
const names = ['连续阅读', '诊断侧栏', '共识观察台']
const scenarios: Record<Scenario, string> = { normal: '正常', stale: '报告过期', failure: 'CPU 采集失败', unknown: '从未观测' }
const stamp = '2026-09-14 10:24:00 UTC'
function value(s: Scenario, text: string) { return s === 'unknown' ? '—' : text }
// Geometry and layered masks adapted from Emerald c2c5e88 Background.vue.
function EmeraldBackground() {
  return <div className="emerald-background" aria-hidden="true"><div className="emerald-atmosphere"><div className="emerald-gradient">
    <svg className="emerald-grid" focusable="false">
      <defs><pattern id="prototype-emerald-grid" width="72" height="56" patternUnits="userSpaceOnUse" x="-12" y="4"><path d="M.5 56V.5H72" fill="none" /></pattern></defs>
      <rect width="100%" height="100%" strokeWidth="0" fill="url(#prototype-emerald-grid)" />
      <svg x="-12" y="4" overflow="visible">
        <rect strokeWidth="0" width="73" height="57" x="288" y="168" />
        <rect strokeWidth="0" width="73" height="57" x="144" y="56" />
        <rect strokeWidth="0" width="73" height="57" x="504" y="168" />
        <rect strokeWidth="0" width="73" height="57" x="720" y="336" />
      </svg>
    </svg>
  </div></div></div>
}
function Panel({ title, children, note }: { title: string; children: ReactNode; note?: string }) {
  return <section className="panel"><h2>{title}</h2>{note && <p className="quiet">{note}</p>}{children}</section>
}
function Row({ label, text }: { label: string; text: string }) { return <div className="metric-row"><span>{label}</span><strong>{text}</strong></div> }
function Summary({ scenario: s }: Props) {
  return <section className={"summary " + s} aria-label="关键摘要">{[
    ['当前 Head', '38,421,906', '此 Node 的最新观测高度'],
    ['同步状态', '已追平', '部署内参考 Head · 高置信度'],
    ['Peer 连接', '28', '入站 16 / 出站 12'],
    ['PlatON 进程运行时长', '12 天 08 小时', '不是 Host 运行时长'],
  ].map(([label, text, detail]) => <div key={label}><span>{label}</span><strong>{value(s, text)}</strong><small>{s === 'unknown' ? '尚无成功观测' : s === 'stale' ? 'Last-good · 10:21:00 UTC' : detail}</small></div>)}</section>
}
function Groups({ scenario: s }: Props) {
  return <div className="groups">
    <Panel title="Node 运行与共识" note="独立于 Host 与其他 Node">
      <Row label="RPC 观测" text={value(s, s === 'stale' ? 'Last-good · 可用' : '可用')} />
      <Row label="QC" text={value(s, '38,421,905')} /><Row label="Locked" text={value(s, '38,421,904')} /><Row label="Committed" text={value(s, '38,421,903')} /><Row label="Validator 成员" text={value(s, 'True')} />
    </Panel>
    <Panel title="PlatON 进程与 Node Data" note="仅此 Node 的进程与数据目录">
      <Row label="进程 CPU" text={value(s, '18.6%')} />{s === 'failure' && <small className="error-text">Error · 保留 10:23:42 UTC 成功值</small>}
      <Row label="进程内存" text={value(s, '32.4%')} /><Row label="Node Data" text={value(s, '186.2 / 512 GiB')} />
      {s !== 'unknown' && <div className="meter"><i style={{ width: '36.4%' }} /></div>}<small>目录大小 / 所在文件系统容量；不是 Host 磁盘用量</small>
      <Row label="进程启动" text={value(s, '09-02 02:24 UTC')} />
    </Panel>
    <Panel title="Host 共享资源" note="整台 Host 汇总 · 非此 Node 独占">
      <Row label="Host CPU" text={value(s, '24.8%')} /><Row label="Host 内存" text={value(s, '48.2%')} /><Row label="Host 存储" text={value(s, '52.1%')} /><Row label="↑ 上传" text={value(s, '128 KiB/s')} /><Row label="↓ 下载" text={value(s, '486 KiB/s')} />
    </Panel>
  </div>
}
const chartSpecs = [
  { title: '进程 CPU', scope: 'PlatON process', current: '18.6%', unit: '%', max: 40 },
  { title: '进程内存', scope: 'PlatON process', current: '32.4%', unit: '%', max: 60 },
  { title: 'Host 网络', scope: 'Host · 共享', current: '↑ 128 / ↓ 486 KiB/s', unit: 'KiB/s', max: 800 },
  { title: 'Peer 连接', scope: 'Node', current: '入站 16 / 出站 12', unit: '连接数', max: 24 },
  { title: '出块间隔', scope: '连续 Block Summary', current: '1.02 s', unit: 's', max: 2 },
  { title: '每块交易数', scope: 'Block Summary', current: '42 tx', unit: 'tx', max: 80 },
]
function Charts({ scenario: s }: Props) {
  return <section className="charts-section"><div className="section-heading"><h2>实时 <span>· 最近 60 秒</span></h2><small>模拟窗口 10:23:00–10:24:00 UTC</small></div><div className="charts">{chartSpecs.map((spec, index) => {
    const failed = s === 'failure' && index === 0
    const missing = s === 'unknown' || s === 'stale'
    function points(second: boolean, from: number, to: number) {
      return Array.from({ length: to - from }, (_, n) => { const i = n + from; const last = failed ? 16 : 24; const current = index === 0 ? 18.6 : index === 1 ? 32.4 : index === 2 ? (second ? 486 : 128) : (second ? 12 : 16); const sample = i === last ? current : index === 3 ? current - (i < 13 ? 1 : 0) : current * (1 + Math.sin(i * 1.8 + index) * (index === 1 ? .015 : .23)); const y = 105 - sample / spec.max * 80; return (12 + i * 11.6) + ',' + y }).join(' ')
    }
    return <article className="panel chart" key={spec.title}><div className="chart-heading"><h3>{spec.title}</h3><small>{spec.scope}</small></div><strong className="chart-value">{value(s, spec.current)}</strong>
      <div className="chart-note">{s === 'unknown' ? 'Unknown · 尚无成功观测' : s === 'stale' ? 'Last-good · 10:21:00 UTC；窗口内无样本' : failed ? 'Error · Last-good 10:23:42 UTC' : '观测于 10:24:00 UTC'}</div>
      <svg viewBox="0 0 310 140" role="img" aria-label={spec.title + '，最近 60 秒模拟数据，单位 ' + spec.unit + (missing ? '，窗口内无样本' : failed ? '，失败区间留空' : '')}>
        {[25, 65, 105].map((y, i) => <g key={y}><line x1="12" x2="294" y1={y} y2={y} className="gridline" /><text x="294" y={y - 5} textAnchor="end">{(spec.max * (2 - i) / 2).toFixed(index === 4 ? 1 : 0)}</text></g>)}
        {!missing && (index >= 4 ? Array.from({ length: 24 }, (_, i) => <rect key={i} x={12 + i * 11.6} y={105 - (index === 4 ? (i === 23 ? 1.02 : 1.02 + Math.sin(i) * .15) : (i === 23 ? 42 : 10 + ((i * 17) % 60))) / spec.max * 80} width="6" height={(index === 4 ? (i === 23 ? 1.02 : 1.02 + Math.sin(i) * .15) : (i === 23 ? 42 : 10 + ((i * 17) % 60))) / spec.max * 80} rx="1" className="bar" />) : <><polyline points={points(false, 0, failed ? 17 : 25)} className="line primary" />{(index === 2 || index === 3) && <polyline points={points(true, 0, 25)} className="line secondary" />}</>)}
        {missing && <text x="155" y="74" textAnchor="middle">{s === 'unknown' ? '尚无观测' : '当前窗口无样本'}</text>}
        <text x="12" y="132">−60s</text><text x="294" y="132" textAnchor="end">0s</text>
      </svg>
      <small>{index === 2 ? '蓝色 ↑ 上传 · 青色 ↓ 下载' : index === 3 ? '蓝色 入站 · 青色 出站' : spec.unit + ' · 不补零，不跨缺口连线'}</small>
    </article>
  })}</div></section>
}
function Diagnostics({ scenario: s }: Props) {
  return <div className="diagnostics"><details className="panel"><summary>Peer Insight / Peer History <span>深入诊断</span></summary><div className="diagnostic-content"><Row label="当前 Peer" text={value(s, '28 · 入站 16 / 出站 12')} /><Row label="Trusted / Static / Consensus" text={value(s, '2 / 4 / 8')} /><h3>Peer History · 此 Node 的聚合记录</h3>{s === 'unknown' ? <p>尚无成功快照。</p> : <><Row label={s === 'stale' ? "10:20:00 UTC" : "10:23:00 UTC"} text="入站 15 / 出站 12" /><Row label={s === 'stale' ? "10:20:30 UTC" : "10:23:30 UTC"} text="入站 16 / 出站 12" /><p className="quiet">{s === 'stale' ? 'Stale · 以上为保留的历史快照，非当前连接。' : '仅聚合计数，不展示 Peer 身份或地址。'}</p></>}</div></details><details className="panel"><summary>标识与技术详情 <span>低频信息</span></summary><div className="diagnostic-content"><Row label="Node ID（模拟）" text="5d2e4401-f383-4f90-bfc7-a723e582ce12" /><Row label="Network" text="PlatON Mainnet" /><p className="quiet">原型不包含 Host 身份、RPC Endpoint、挂载路径或原始 Peer 身份。</p></div></details></div>
}
export function VariantA(p: Props) { return <><Summary {...p} /><Groups {...p} /><Charts {...p} /><Diagnostics {...p} /></> }
export function VariantB(p: Props) { return <div className="sidebar-layout"><aside><Summary {...p} /><p className="quiet">先判断 Node 是否追平，再检查进程与共享资源。</p><a href="#observations">跳至运行观测 ↓</a></aside><div id="observations"><Groups {...p} /><Charts {...p} /><Diagnostics {...p} /></div></div> }
export function VariantC(p: Props) { return <><div className="consensus-band"><span>共识高度观察</span>{['Head · 38,421,906', 'QC · 38,421,905', 'Locked · 38,421,904', 'Committed · 38,421,903'].map(t => <strong key={t}>{p.scenario === 'unknown' ? t.split(' · ')[0] + ' · —' : t}</strong>)}</div><Summary {...p} /><Groups {...p} /><Charts {...p} /><Diagnostics {...p} /></> }
function PrototypeSwitcher({ variant, change }: { variant: string; change: (step: number) => void }) {
  useEffect(() => { const handler = (e: KeyboardEvent) => { if (e.target instanceof Element && e.target.closest('input, textarea, select, button, [contenteditable]')) return; if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') { e.preventDefault(); change(e.key === 'ArrowLeft' ? -1 : 1) } }; window.addEventListener('keydown', handler); return () => window.removeEventListener('keydown', handler) }, [change])
  if (!import.meta.env.DEV) return null
  return <div className="switcher" aria-label="原型布局切换"><button aria-label="上一布局" onClick={() => change(-1)}>←</button><div><small>THROWAWAY PROTOTYPE</small><strong>{variant} / {names[variants.indexOf(variant)]}</strong></div><button aria-label="下一布局" onClick={() => change(1)}>→</button></div>
}
function App() {
  const [params, setParams] = useSearchParams()
  const variant = variants.includes(params.get('variant') ?? '') ? params.get('variant')! : 'A'
  const [themeMode, setThemeMode] = useState<ThemeMode>(initialThemeMode)
  const [theme, setTheme] = useState<'light' | 'dark'>(() => document.documentElement.classList.contains('dark') ? 'dark' : 'light')
  const nextThemeMode = nextThemeModes[themeMode]
  const themeButtonLabel = '当前主题：' + themeLabels[themeMode] + '；切换至' + themeLabels[nextThemeMode]
  useLayoutEffect(() => {
    const media = window.matchMedia?.('(prefers-color-scheme: dark)')
    const apply = () => {
      const resolved = themeMode === 'dark' || (themeMode === 'auto' && media?.matches) ? 'dark' : 'light'
      const root = document.documentElement
      root.classList.toggle('dark', resolved === 'dark')
      root.style.colorScheme = resolved
      root.dataset.themeMode = themeMode
      setTheme(resolved)
    }
    apply()
    if (themeMode !== 'auto' || !media) return
    media.addEventListener('change', apply)
    return () => media.removeEventListener('change', apply)
  }, [themeMode])
  useEffect(() => {
    try { window.localStorage.setItem(themeStorageKey, themeMode) } catch { /* Theme still works when storage is unavailable. */ }
  }, [themeMode])
  const [scenario, setScenario] = useState<Scenario>('normal')
  const [page, setPage] = useState('node')
  function change(step: number) { const next = new URLSearchParams(params); next.set('variant', variants[(variants.indexOf(variant) + step + 3) % 3]); setParams(next, { replace: true }) }
  const Layout = variant === 'B' ? VariantB : variant === 'C' ? VariantC : VariantA
  return <div className={'prototype ' + theme + ' variant-' + variant}><EmeraldBackground /><header className="topbar"><button className="brand" onClick={() => setPage('home')}><span className="brandmark">╱╲</span> PlatPulse<span className="brand-caption">NODE OBSERVATORY</span></button><div className="top-actions"><span className="demo-tag">模拟数据 · 免登录</span><button aria-label={themeButtonLabel} title={themeButtonLabel} onClick={() => setThemeMode(nextThemeMode)}><span aria-hidden="true">{themeIcons[themeMode]}</span> {themeLabels[themeMode]} → {themeLabels[nextThemeMode]}</button></div></header>
    <main><div className="prototype-controls"><span>设计预览 / 不连接真实服务</span><label>模拟状态 <select value={scenario} onChange={e => setScenario(e.target.value as Scenario)}>{Object.entries(scenarios).map(([key, label]) => <option key={key} value={key}>{label}</option>)}</select></label></div>
      {page === 'home' ? <><div className="page-heading"><div><p className="eyebrow">PUBLIC PROJECTION / PREVIEW</p><h1>Node 概览</h1><p className="quiet">原型导航入口 · 选择 Node 查看设计方案</p></div></div><button className="node-link panel" onClick={() => setPage('node')}><span>PlatON Mainnet</span><h2>Atlas / Mainnet 01 →</h2><p>{scenarios[scenario]} · Head {value(scenario, '38,421,906')}</p></button></> : <><button className="back" onClick={() => setPage('home')}>← Node 概览 <span>/ PlatON Mainnet</span></button><div className="page-heading"><div><p className="eyebrow">PLATON MAINNET / NODE DETAIL</p><h1>Atlas <span>/ Mainnet 01</span></h1><p className="quiet">观测一个 Node，理解它此刻的运行状态。</p></div><div className="heading-status"><span className={'status ' + scenario}>{scenario === 'normal' ? '● Healthy' : scenario === 'unknown' ? '○ Unknown' : scenario === 'stale' ? '◷ Stale' : '！需要关注'}</span><small>最后报告</small><time>{scenario === 'unknown' ? '尚未收到报告' : scenario === 'stale' ? '2026-09-14 10:21:00 UTC' : stamp}</time></div></div>
      <div aria-live="polite">{scenario !== 'normal' && <div className={'notice ' + scenario}>{scenario === 'stale' ? '报告已过期 · 实时更新已暂停。保留 10:21:00 UTC 的成功值，不代表当前状态；最近 60 秒没有样本。' : scenario === 'failure' ? '进程 CPU 采集失败 · 保留 10:23:42 UTC 的 18.6%。其他观测仍独立可用，图表失败区间留空。' : '尚无成功观测 · 未知值显示为 —，不会视为 0 或 Healthy。'}</div>}</div><Layout scenario={scenario} /></>}
      <footer className="state-footer">原型状态：布局 {variant}（{names[variants.indexOf(variant)]}） / 主题：{themeLabels[themeMode]}（当前显示：{themeLabels[theme]}） / {scenarios[scenario]} / {page} / 模拟数据仅在内存 / 无 API、无登录 / 仅主题偏好持久化（存储可用时）</footer>
    </main><PrototypeSwitcher variant={variant} change={change} /></div>
}
createRoot(document.getElementById('root')!).render(<BrowserRouter><App /></BrowserRouter>)
