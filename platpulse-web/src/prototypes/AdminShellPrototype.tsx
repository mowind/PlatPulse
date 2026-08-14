import { useCallback, useEffect } from 'react'
import { useSearchParams } from 'react-router'
import './admin-shell-prototype.css'

type VariantKey = 'a' | 'b' | 'c'

type PrototypeVariant = {
  key: VariantKey
  name: string
  description: string
}

const variants: PrototypeVariant[] = [
  { key: 'a', name: 'Operations rail', description: 'Persistent navigation with a dense operations overview.' },
  { key: 'b', name: 'Command deck', description: 'Top-level priorities first, with a calmer dashboard canvas.' },
  { key: 'c', name: 'Split workspace', description: 'A list-and-detail workspace for repeated node diagnosis.' },
]

const nodes = [
  { name: 'Validator North', agent: 'edge-host-01', network: 'Mainnet', status: 'Healthy', detail: 'Head 2,184,220 · observed 18s ago' },
  { name: 'Validator South', agent: 'edge-host-01', network: 'Mainnet', status: 'Warning', detail: 'Sync stale · last good value 4m ago' },
  { name: 'Observer West', agent: 'edge-host-02', network: 'Testnet', status: 'Unknown', detail: 'RPC capability probe unavailable' },
]

const navGroups = [
  { label: 'Operate', items: ['Overview', 'Agents & Nodes', 'Networks'] },
  { label: 'Respond', items: ['Alerts', 'Access & Audit', 'Data & Maintenance'] },
]

export default function AdminShellPrototype() {
  const [searchParams, setSearchParams] = useSearchParams()
  const rawVariant = searchParams.get('variant')
  const current = variants.some((variant) => variant.key === rawVariant) ? rawVariant as VariantKey : 'a'
  const currentVariant = variants.find((variant) => variant.key === current) ?? variants[0]

  const setVariant = useCallback((key: VariantKey) => {
    const next = new URLSearchParams(searchParams)
    next.set('variant', key)
    setSearchParams(next, { replace: true })
  }, [searchParams, setSearchParams])

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      const target = event.target as HTMLElement | null
      if (target?.matches('input, textarea, select, [contenteditable="true"]')) return
      if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return
      event.preventDefault()
      const index = variants.findIndex((variant) => variant.key === current)
      const delta = event.key === 'ArrowRight' ? 1 : -1
      const next = variants[(index + delta + variants.length) % variants.length]
      if (next) setVariant(next.key)
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [current, setVariant])

  return (
    <div className={`prototype-page prototype-variant-${current}`}>
      <div className="prototype-notice" role="note">
        <strong>Prototype</strong>
        <span>Visual shell exploration — no production data or mutations.</span>
      </div>
      {current === 'a' && <OperationsRail />}
      {current === 'b' && <CommandDeck />}
      {current === 'c' && <SplitWorkspace />}
      <PrototypeSwitcher current={currentVariant} onChange={setVariant} />
    </div>
  )
}

function OperationsRail() {
  return (
    <div className="shell shell-rail">
      <aside className="rail-sidebar" aria-label="Admin navigation">
        <div className="shell-brand"><span className="brand-mark">P</span><span>PlatPulse</span></div>
        <p className="eyebrow">Admin dashboard</p>
        <nav className="rail-nav">
          <a className="nav-active" href="#overview">Overview <span>⌂</span></a>
          {navGroups.map((group) => (
            <div className="nav-group" key={group.label}>
              <p className="nav-group-label">{group.label}</p>
              {group.items.map((item) => <a href={`#${item.toLowerCase().replaceAll(' ', '-')}`} key={item}>{item}</a>)}
            </div>
          ))}
        </nav>
        <div className="sidebar-footer"><span className="status-dot dot-healthy" /> Server ready <span className="muted">·</span> Owner</div>
      </aside>
      <main className="shell-content">
        <ShellHeader title="Operations overview" context="All Agents · All Networks" />
        <section className="hero-row">
          <div><p className="eyebrow">Wednesday, 12 August 2026</p><h1>Good morning, Owner.</h1><p className="lede">Your deployment has one item that needs attention.</p></div>
          <button className="quiet-button" type="button">Review alerts <span aria-hidden="true">→</span></button>
        </section>
        <StatusStrip />
        <MetricGrid />
        <NodeTable />
      </main>
    </div>
  )
}

function CommandDeck() {
  return (
    <div className="shell shell-deck">
      <main className="shell-content">
        <ShellHeader title="Admin" context="Operations overview" compact />
        <section className="deck-hero">
          <div><p className="eyebrow">Live operations</p><h1>Everything is accounted for.</h1><p className="lede">3 PlatON Nodes · 2 Agents · 2 Networks</p></div>
          <div className="hero-health"><span className="status-icon icon-healthy">✓</span><div><strong>System healthy</strong><span>Updated 18 seconds ago</span></div></div>
        </section>
        <div className="deck-grid">
          <section className="deck-card deck-card-wide"><div className="card-heading"><div><p className="eyebrow">Attention queue</p><h2>Needs a decision</h2></div><a href="#alerts">View all</a></div><div className="attention-item"><span className="status-icon icon-warning">!</span><div><strong>Validator South is behind</strong><span>Sync observation is stale by 4 minutes.</span></div><button className="text-button" type="button">Inspect →</button></div><div className="attention-item"><span className="status-icon icon-unknown">?</span><div><strong>Observer West capability probe</strong><span>RPC response is currently unknown.</span></div><button className="text-button" type="button">Inspect →</button></div></section>
          <section className="deck-card"><div className="card-heading"><div><p className="eyebrow">Inventory</p><h2>At a glance</h2></div></div><div className="ring-stat"><strong>3</strong><span>active<br />PlatON Nodes</span></div><div className="mini-legend"><span><i className="legend-healthy" /> Healthy 1</span><span><i className="legend-warning" /> Warning 1</span><span><i className="legend-unknown" /> Unknown 1</span></div></section>
          <section className="deck-card deck-card-wide"><div className="card-heading"><div><p className="eyebrow">PlatON Nodes</p><h2>Current state</h2></div><a href="#nodes">Open list</a></div><div className="node-rows">{nodes.map((node) => <NodeRow node={node} key={node.name} />)}</div></section>
        </div>
      </main>
    </div>
  )
}

function SplitWorkspace() {
  const selected = nodes[1]
  return (
    <div className="shell shell-workspace">
      <aside className="workspace-list">
        <div className="workspace-list-header"><div className="shell-brand"><span className="brand-mark">P</span><span>PlatPulse</span></div><button className="icon-button" type="button" aria-label="Open navigation">☰</button></div>
        <div className="workspace-title"><p className="eyebrow">PlatON Nodes</p><h1>3 monitored</h1><label className="search-field"><span aria-hidden="true">⌕</span><input aria-label="Filter nodes" placeholder="Filter nodes" /></label></div>
        <div className="workspace-items">{nodes.map((node, index) => <button className={`workspace-item ${index === 1 ? 'selected' : ''}`} type="button" key={node.name}><span className={`status-badge status-${node.status.toLowerCase()}`}>{node.status}</span><strong>{node.name}</strong><span>{node.agent} · {node.network}</span></button>)}</div>
        <div className="workspace-list-footer"><a href="#agents">All Agents</a><a href="#alerts">Alerts <span className="count-pill">2</span></a></div>
      </aside>
      <main className="workspace-detail">
        <ShellHeader title="PlatON Node" context="Agents & Nodes / Validator South" />
        <div className="detail-heading"><div><div className="breadcrumb">edge-host-01 <span>›</span> Mainnet</div><h1>{selected.name}</h1><p className="lede">Node ID 0195f2a1…0015 · owned by edge-host-01</p></div><button className="primary-button" type="button">Open diagnostics</button></div>
        <div className="detail-alert"><span className="status-icon icon-warning">!</span><div><strong>Sync observation is stale</strong><span>Showing last good value from 4 minutes ago. The current collection attempt failed.</span></div><a href="#history">View history</a></div>
        <div className="detail-grid"><section className="detail-panel"><p className="eyebrow">Current observations</p><div className="observation-grid"><Observation label="Health" value="Warning" /><Observation label="RPC" value="Ok" /><Observation label="Sync" value="Stale" /><Observation label="Current head" value="2,184,019" /><Observation label="Consensus" value="Unknown" /><Observation label="Process" value="Disabled" /></div></section><section className="detail-panel"><div className="card-heading"><div><p className="eyebrow">Recent activity</p><h2>History timeline</h2></div><a href="#history">Open history</a></div><div className="timeline"><span className="timeline-line" /><p><b>18s ago</b> RPC observation accepted</p><p><b>4m ago</b> Sync last-good value recorded</p><p><b>12m ago</b> Host observation accepted</p></div></section></div>
        </main>
    </div>
  )
}

function ShellHeader({ title, context, compact = false }: { title: string; context: string; compact?: boolean }) {
  return <header className={`shell-header ${compact ? 'shell-header-compact' : ''}`}><div className="mobile-menu"><button className="icon-button" type="button" aria-label="Open navigation">☰</button></div><div><p className="breadcrumb">{context}</p><h2>{title}</h2></div><div className="header-actions"><button className="icon-button" type="button" aria-label="Notifications">◌</button><button className="avatar-button" type="button" aria-label="Open Owner menu">O</button></div></header>
}

function StatusStrip() {
  return <div className="status-strip"><span className="status-icon icon-warning">!</span><div><strong>1 PlatON Node needs attention</strong><span>Validator South has a stale sync observation.</span></div><a href="#alerts">Review</a></div>
}

function MetricGrid() {
  return <div className="metric-grid"><Metric label="PlatON Nodes" value="3" detail="2 healthy · 1 needs attention" /><Metric label="Agents" value="2" detail="All reporting" /><Metric label="Networks" value="2" detail="Mainnet · Testnet" /><Metric label="Open incidents" value="0" detail="No active incidents" /></div>
}

function Metric({ label, value, detail }: { label: string; value: string; detail: string }) {
  return <section className="metric-card"><p className="eyebrow">{label}</p><strong>{value}</strong><span>{detail}</span></section>
}

function NodeTable() {
  return <section className="content-panel"><div className="card-heading"><div><p className="eyebrow">Inventory</p><h2>PlatON Nodes</h2></div><a href="#nodes">View all</a></div><div className="table-scroll"><table><thead><tr><th scope="col">PlatON Node</th><th scope="col">Agent</th><th scope="col">Network</th><th scope="col">Status</th><th scope="col">Last update</th></tr></thead><tbody>{nodes.map((node) => <tr key={node.name}><th scope="row"><a href="#node">{node.name}</a></th><td>{node.agent}</td><td>{node.network}</td><td><span className={`status-badge status-${node.status.toLowerCase()}`}>{node.status}</span></td><td>{node.detail}</td></tr>)}</tbody></table></div></section>
}

function NodeRow({ node }: { node: typeof nodes[number] }) {
  return <a className="node-row" href="#node"><span className={`status-badge status-${node.status.toLowerCase()}`}>{node.status}</span><span><strong>{node.name}</strong><small>{node.agent} · {node.network}</small></span><span className="row-detail">{node.detail}</span><span aria-hidden="true">→</span></a>
}

function Observation({ label, value }: { label: string; value: string }) {
  return <div className="observation"><span>{label}</span><strong>{value}</strong></div>
}

function PrototypeSwitcher({ current, onChange }: { current: PrototypeVariant; onChange: (key: VariantKey) => void }) {
  const index = variants.findIndex((variant) => variant.key === current.key)
  const previous = variants[(index - 1 + variants.length) % variants.length]
  const next = variants[(index + 1) % variants.length]
  return <nav className="prototype-switcher" aria-label="Prototype variants"><button type="button" onClick={() => previous && onChange(previous.key)} aria-label="Previous variant">←</button><div><strong>{current.key.toUpperCase()} — {current.name}</strong><span>{current.description}</span></div><button type="button" onClick={() => next && onChange(next.key)} aria-label="Next variant">→</button></nav>
}
