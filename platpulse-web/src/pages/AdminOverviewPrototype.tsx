import { useEffect, useState } from 'react'
import { Link, useSearchParams } from 'react-router'
import platpulseMark from '../../../assets/platpulse-mark.png'
import './AdminOverviewPrototype.css'

export type OverviewPrototypeVariant = 'A' | 'B' | 'C'

type Attention = {
  severity: 'critical' | 'warning'
  kind: string
  subject: string
  subjectType: 'Node' | 'Agent' | 'Network'
  message: string
  age: string
}

type NodeRow = {
  id: string
  name: string
  network: string
  health: 'Healthy' | 'Unhealthy' | 'Unknown'
  freshness: 'Current' | 'Stale' | 'Unknown'
  head: string
  sync: string
  reason: string
  resync: string
  agent: string
}

type AgentRow = {
  id: string
  state: 'Current' | 'Error' | 'Unknown'
  nodes: string
  host: string
  spool: string
  detail: string
}

const ATTENTION: Attention[] = [
  { severity: 'critical', kind: 'node_identity_mismatch', subject: 'validator-eu-01', subjectType: 'Node', message: 'Observed Network Identity differs from PlatON Mainnet registry', age: '4 min ago' },
  { severity: 'critical', kind: 'agent_spool_overflow', subject: 'agent-edge-02', subjectType: 'Agent', message: 'Durable Spool is at capacity; reports may be discarded', age: '9 min ago' },
  { severity: 'warning', kind: 'node_health_unknown', subject: 'archive-ap-02', subjectType: 'Node', message: 'Not enough current component observations to evaluate health', age: '18 min ago' },
  { severity: 'warning', kind: 'agent_offline', subject: 'agent-edge-03', subjectType: 'Agent', message: 'No accepted Agent Report since the last known-good report', age: '26 min ago' },
  { severity: 'warning', kind: 'node_resync', subject: 'validator-us-01', subjectType: 'Node', message: 'Resync is open; the current head is behind the highest observed block', age: '31 min ago' },
  { severity: 'warning', kind: 'agent_shutdown_incomplete', subject: 'agent-lab-01', subjectType: 'Agent', message: 'Shutdown evidence is incomplete for the previous boot', age: '42 min ago' },
]

const NODES: NodeRow[] = [
  { id: 'node-7e91', name: 'validator-eu-01', network: 'PlatON Mainnet', health: 'Unhealthy', freshness: 'Current', head: '18,420,912', sync: '−12 blocks', reason: 'Network Identity mismatch', resync: 'Open', agent: 'agent-edge-01' },
  { id: 'node-2f34', name: 'validator-us-01', network: 'PlatON Mainnet', health: 'Healthy', freshness: 'Stale', head: '18,420,900', sync: '−24 blocks', reason: 'RPC current · data stale', resync: 'Open', agent: 'agent-edge-02' },
  { id: 'node-9c18', name: 'archive-ap-02', network: 'PlatON Mainnet', health: 'Unknown', freshness: 'Unknown', head: 'Unknown', sync: 'Unknown', reason: 'Never observed by Server', resync: 'Not observed', agent: 'agent-edge-03' },
  { id: 'node-44ad', name: 'sentinel-sg-01', network: 'PlatON Testnet', health: 'Healthy', freshness: 'Current', head: '9,884,201', sync: 'Current', reason: 'RPC, sync, and consensus current', resync: 'Normal', agent: 'agent-lab-01' },
]

const AGENTS: AgentRow[] = [
  { id: 'agent-edge-02', state: 'Error', nodes: '2 active · 1 unknown', host: 'CPU 74% · Mem 62%', spool: '100% · overflow', detail: 'Report #128 · 9 min ago' },
  { id: 'agent-edge-03', state: 'Error', nodes: '1 active · 1 unknown', host: 'CPU 31% · Mem 48%', spool: '12% · normal', detail: 'Report #93 · 26 min ago' },
  { id: 'agent-edge-01', state: 'Current', nodes: '1 active', host: 'CPU 18% · Mem 41%', spool: '4% · normal', detail: 'Report #411 · 4 min ago' },
  { id: 'agent-lab-01', state: 'Unknown', nodes: '1 active', host: 'Unknown host values', spool: 'Unknown', detail: 'No accepted report' },
]

const GROUPS = ATTENTION.map((item) => ({ subject: item.subject, type: item.subjectType, items: [item] }))

const VARIANTS: Array<{ key: OverviewPrototypeVariant; name: string }> = [
  { key: 'A', name: 'Triage stack' },
  { key: 'B', name: 'Command rail' },
  { key: 'C', name: 'Operations canvas' },
]

/** Standalone development route: no Server session or credentials are required. */
export function AdminOverviewPrototypeStandalone() {
  const [searchParams] = useSearchParams()
  const requested = searchParams.get('variant')?.toUpperCase()
  const variant: OverviewPrototypeVariant = requested === 'B' || requested === 'C' ? requested : 'A'

  return (
    <div className="app-shell admin-shell prototype-standalone-shell">
      <header className="app-header admin-header">
        <span className="app-brand">
          <img className="app-brand-logo" src={platpulseMark} alt="" />
          <span>PlatPulse</span>
        </span>
        <span className="prototype-shell-badge">Standalone prototype · no login</span>
      </header>
      <div className="admin-body">
        <nav className="admin-nav prototype-static-nav" aria-label="Prototype Admin navigation">
          <p className="admin-nav-label">Operations</p>
          <a className="active" href="#overview">Overview</a>
          <a href="#agents">Agents</a>
          <a href="#nodes">Nodes</a>
          <a href="#networks">Networks</a>
          <a href="#settings">Settings</a>
          <p className="admin-nav-label">Access</p>
          <a href="#sessions">Sessions</a>
          <a href="#audit">Audit</a>
        </nav>
        <main className="app-main" id="overview">
          <AdminOverviewPrototype variant={variant} />
        </main>
      </div>
    </div>
  )
}

/** Three throwaway Admin Overview variants, using representative non-persistent state. */
export default function AdminOverviewPrototype({ variant }: { variant: OverviewPrototypeVariant }) {
  const [searchParams, setSearchParams] = useSearchParams()
  const [expandedSubject, setExpandedSubject] = useState<string | null>(null)
  const [showAll, setShowAll] = useState(false)
  const [refreshed, setRefreshed] = useState(false)
  const currentIndex = VARIANTS.findIndex((item) => item.key === variant)

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null
      if (target?.matches('input, textarea, select, [contenteditable="true"]')) return
      if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return
      event.preventDefault()
      const nextIndex = event.key === 'ArrowRight'
        ? (currentIndex + 1) % VARIANTS.length
        : (currentIndex - 1 + VARIANTS.length) % VARIANTS.length
      const next = new URLSearchParams(searchParams)
      next.set('variant', VARIANTS[nextIndex].key)
      setSearchParams(next)
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [currentIndex, searchParams, setSearchParams])

  const visibleGroups = showAll ? GROUPS : GROUPS.slice(0, 3)
  const criticalCount = ATTENTION.filter((item) => item.severity === 'critical').length

  return (
    <section className={'page overview-prototype prototype-' + variant.toLowerCase()}>
      <div className="prototype-note"><span>PROTOTYPE</span> Three Admin Overview compositions · representative review state · no mutations</div>
      {variant === 'A' && <VariantA expandedSubject={expandedSubject} setExpandedSubject={setExpandedSubject} visibleGroups={visibleGroups} showAll={showAll} setShowAll={setShowAll} criticalCount={criticalCount} refreshed={refreshed} setRefreshed={setRefreshed} />}
      {variant === 'B' && <VariantB expandedSubject={expandedSubject} setExpandedSubject={setExpandedSubject} criticalCount={criticalCount} />}
      {variant === 'C' && <VariantC expandedSubject={expandedSubject} setExpandedSubject={setExpandedSubject} />}
      <PrototypeSwitcher variant={variant} />
    </section>
  )
}

function VariantA({ expandedSubject, setExpandedSubject, visibleGroups, showAll, setShowAll, criticalCount, refreshed, setRefreshed }: {
  expandedSubject: string | null
  setExpandedSubject: (subject: string | null) => void
  visibleGroups: typeof GROUPS
  showAll: boolean
  setShowAll: (value: boolean) => void
  criticalCount: number
  refreshed: boolean
  setRefreshed: (value: boolean) => void
}) {
  return (
    <>
      <OverviewHeader eyebrow="Owner triage" title="Overview" copy="A compact read on what needs intervention across your PlatON estate." refreshed={refreshed} setRefreshed={setRefreshed} />
      <section className="a-attention overview-panel">
        <PanelTitle eyebrow="01 · Attention" title="Attention queue" meta={ATTENTION.length + ' issues · ' + GROUPS.length + ' subjects'} />
        <div className="a-attention-grid">
          <div className="a-attention-summary"><strong>{criticalCount}</strong><span>critical issues</span><p>Resolve integrity and storage risks first. Warnings stay visible without being promoted.</p><Link to="/admin/nodes?health=unhealthy" className="prototype-link">Review unhealthy Nodes <span>↗</span></Link></div>
          <div className="attention-groups">{visibleGroups.map((group) => <AttentionGroup key={group.subject} group={group} expanded={expandedSubject === group.subject} onToggle={() => setExpandedSubject(expandedSubject === group.subject ? null : group.subject)} />)}</div>
        </div>
        <button type="button" className="quiet-button" onClick={() => setShowAll(!showAll)}>{showAll ? 'Collapse queue' : 'Show ' + (GROUPS.length - visibleGroups.length) + ' more'}</button>
      </section>
      <SummaryCards />
      <section className="overview-panel a-nodes"><PanelTitle eyebrow="02 · Node Health Summary" title="Active Nodes" meta="Showing 4 of 9 Active Nodes" action="View all Nodes" /><NodeLedger expandedSubject={expandedSubject} setExpandedSubject={setExpandedSubject} /></section>
      <section className="a-agent-section"><PanelTitle eyebrow="03 · Agent inventory" title="Agents" meta="Showing 4 of 6 Agents" action="View all Agents" /><div className="agent-cards">{AGENTS.map((agent) => <AgentCard key={agent.id} agent={agent} />)}</div></section>
    </>
  )
}

function VariantB({ expandedSubject, setExpandedSubject, criticalCount }: { expandedSubject: string | null; setExpandedSubject: (subject: string | null) => void; criticalCount: number }) {
  return (
    <>
      <OverviewHeader eyebrow="Operations / at a glance" title="System posture" copy="A split-screen command rail keeps the highest-risk subjects in view while inventory stays scannable." />
      <section className="b-attention-mobile overview-panel"><PanelTitle eyebrow="Intervention queue" title="Attention first" meta={ATTENTION.length + ' issues'} />{ATTENTION.slice(0, 3).map((item) => <AttentionRow key={item.kind + item.subject} item={item} />)}</section>
      <div className="b-layout">
        <main className="b-main-column">
          <section className="b-hero"><div><span className="eyebrow">Current posture</span><h2>Stable enough to operate.</h2><p>9 Active Nodes · 4 Agents · 2 Networks</p></div><div className="posture-ring"><strong>67%</strong><span>clear</span></div></section>
          <section className="b-metrics"><Metric label="Healthy Nodes" value="6" note="of 9 active" tone="good" /><Metric label="Unknown" value="1" note="needs observation" tone="neutral" /><Metric label="Retired" value="2" note="outside live health" tone="muted" /><Metric label="Network mismatch" value="1" note="critical" tone="bad" /></section>
          <section className="overview-panel b-node-panel"><PanelTitle eyebrow="Inventory / priority order" title="Node Health Summary" meta="4 of 9 active" action="Open inventory" /><NodeLedger expandedSubject={expandedSubject} setExpandedSubject={setExpandedSubject} compact /></section>
          <section className="b-agent-rail"><div><span className="eyebrow">Agent inventory</span><h2>Reporting fabric</h2></div><Link to="/admin/agents" className="prototype-link">Open Agents ↗</Link>{AGENTS.map((agent) => <AgentLine key={agent.id} agent={agent} />)}</section>
        </main>
        <aside className="b-attention-rail"><div className="rail-top"><span className="eyebrow">Intervention queue</span><span className="rail-count">{ATTENTION.length}</span></div><h2>{criticalCount} critical<br />items first.</h2><p className="muted">Ordered by Server severity and authoritative observation time.</p><div className="rail-list">{ATTENTION.map((item) => <AttentionRow key={item.kind + item.subject} item={item} />)}</div><Link to="/admin/nodes?health=unhealthy" className="rail-footer">Review all attention <span>↗</span></Link></aside>
      </div>
    </>
  )
}

function VariantC({ expandedSubject, setExpandedSubject }: { expandedSubject: string | null; setExpandedSubject: (subject: string | null) => void }) {
  return (
    <>
      <OverviewHeader eyebrow="Control room" title="Fleet overview" copy="A wide operational canvas turns the dashboard into a quiet ledger: counts first, evidence underneath." />
      <section className="c-attention-strip"><div><span className="severity-mark">!</span><strong>2 critical</strong><span>Network Identity and Durable Spool integrity need intervention.</span></div><Link to="/admin/nodes?health=unhealthy">Review queue ↗</Link></section>
      <section className="c-kpis"><Kpi label="Active Nodes" value="9" detail="6 healthy · 2 unhealthy · 1 unknown" accent="violet" link="/admin/nodes?lifecycle=active" /><Kpi label="Agents" value="4" detail="2 current · 1 error · 1 unknown" accent="green" link="/admin/agents" /><Kpi label="Retired Nodes" value="2" detail="Excluded from live health" accent="slate" link="/admin/nodes?lifecycle=retired" /><Kpi label="Networks" value="2" detail="1 identity mismatch" accent="red" link="/admin/networks" /></section>
      <div className="c-canvas">
        <section className="c-inbox overview-panel"><PanelTitle eyebrow="Inbox" title="Attention queue" meta="6 issues across 6 subjects" /><div className="c-timeline">{ATTENTION.map((item, index) => <div className="timeline-item" key={item.kind + item.subject}><span className={'timeline-dot ' + item.severity} /><span className="timeline-index">{'0' + (index + 1)}</span><div><strong>{item.subject}</strong><p>{item.message}</p><small>{item.kind} · {item.age}</small></div><span className={'severity-word ' + item.severity}>{item.severity}</span></div>)}</div></section>
        <section className="c-ledger overview-panel"><div className="c-ledger-head"><PanelTitle eyebrow="Ledger" title="Node Health Summary" meta="Priority subset · 4 of 9" /><Link to="/admin/nodes" className="prototype-link">View all ↗</Link></div><NodeLedger expandedSubject={expandedSubject} setExpandedSubject={setExpandedSubject} wide /></section>
      </div>
      <section className="c-agents"><div className="c-ledger-head"><PanelTitle eyebrow="Reporting fabric" title="Agent inventory" meta="Host observations shown once per Agent" /><Link to="/admin/agents" className="prototype-link">View all ↗</Link></div><div className="c-agent-list">{AGENTS.map((agent, index) => <div className="c-agent-row" key={agent.id}><span className="c-agent-number">{'0' + (index + 1)}</span><span className={'state-dot ' + agent.state.toLowerCase()} /><strong>{agent.id}</strong><span>{agent.nodes}</span><span>{agent.host}</span><span>{agent.spool}</span><small>{agent.detail}</small><Link to={'/admin/agents/' + agent.id} aria-label={'View ' + agent.id}>↗</Link></div>)}</div></section>
    </>
  )
}

function OverviewHeader({ eyebrow, title, copy, refreshed, setRefreshed }: { eyebrow: string; title: string; copy: string; refreshed?: boolean; setRefreshed?: (value: boolean) => void }) {
  return <header className="prototype-header"><div><span className="eyebrow">{eyebrow}</span><h1>{title}</h1><p>{copy}</p></div><div className="header-status"><span className="live-dot" /> Last good snapshot · 2 min ago {setRefreshed && <button type="button" className="refresh-button" onClick={() => setRefreshed(true)}>{refreshed ? 'Snapshot refreshed' : 'Refresh'}</button>}</div></header>
}

function PanelTitle({ eyebrow, title, meta, action }: { eyebrow: string; title: string; meta: string; action?: string }) {
  return <div className="prototype-panel-title"><div><span className="eyebrow">{eyebrow}</span><h2>{title}</h2></div><div className="panel-title-meta">{meta}{action && <Link to="/admin/nodes" className="prototype-link">{action} ↗</Link>}</div></div>
}

function SummaryCards() {
  const cards = [
    { label: 'Agents', value: '4', detail: '2 current · 1 error · 1 unknown', href: '/admin/agents', accent: 'violet' },
    { label: 'Active Nodes', value: '9', detail: '6 healthy · 2 unhealthy · 1 unknown', href: '/admin/nodes?lifecycle=active', accent: 'green' },
    { label: 'Retired Nodes', value: '2', detail: 'Outside live health buckets', href: '/admin/nodes?lifecycle=retired', accent: 'slate' },
    { label: 'Networks', value: '2', detail: '1 identity mismatch', href: '/admin/networks', accent: 'red' },
  ]
  return <div className="summary-cards">{cards.map((card) => <Link className={'summary-card accent-' + card.accent} to={card.href} key={card.label}><span className="eyebrow">{card.label}</span><strong>{card.value}</strong><span>{card.detail}</span><span className="card-arrow">↗</span></Link>)}</div>
}

function AttentionGroup({ group, expanded, onToggle }: { group: (typeof GROUPS)[number]; expanded: boolean; onToggle: () => void }) {
  const item = group.items[0]
  return <div className={'attention-group ' + item.severity}><button type="button" onClick={onToggle} aria-expanded={expanded}><span className="severity-mark">{item.severity === 'critical' ? '!' : '·'}</span><span><strong>{group.subject}</strong><small>{group.type} · {group.items.length} issue</small></span><span className="group-chevron">{expanded ? '⌃' : '›'}</span></button>{expanded && <div className="attention-expanded"><p>{item.message}</p><small>{item.kind} · {item.age}</small><Link to={group.type === 'Node' ? '/admin/nodes' : '/admin/agents'} className="prototype-link">Open {group.type} ↗</Link></div>}</div>
}

function AttentionRow({ item }: { item: Attention }) {
  return <div className={'rail-attention-row ' + item.severity}><div><span className="severity-mark">{item.severity === 'critical' ? '!' : '·'}</span><strong>{item.subject}</strong></div><p>{item.message}</p><small>{item.kind} · {item.age}</small></div>
}

function NodeLedger({ expandedSubject, setExpandedSubject, compact = false, wide = false }: { expandedSubject: string | null; setExpandedSubject: (subject: string | null) => void; compact?: boolean; wide?: boolean }) {
  return <div className={'node-ledger ' + (compact ? 'compact ' : '') + (wide ? 'wide' : '')}><div className="node-ledger-head"><span>Node</span><span>Health / freshness</span><span>Head / sync</span><span>Resync</span><span /></div>{NODES.map((node) => <div className="node-ledger-row" key={node.id}><div className="node-identity"><span className="node-pulse" /><div><strong>{node.name}</strong><small>{node.id} · {node.network}</small></div></div><div><span className={'health-text ' + node.health.toLowerCase()}>{node.health}</span><small>{node.freshness} · {node.reason}</small></div><div><strong>{node.head}</strong><small>{node.sync}</small></div><div><span className={node.resync === 'Normal' ? 'resync-normal' : 'resync-open'}>{node.resync}</span><small>{node.agent}</small></div><button type="button" className="node-expand" aria-expanded={expandedSubject === node.name} onClick={() => setExpandedSubject(expandedSubject === node.name ? null : node.name)}>{expandedSubject === node.name ? 'Hide' : 'Details'}</button>{expandedSubject === node.name && <div className="node-diagnostic"><span>RPC <strong>{node.health === 'Unknown' ? 'Unknown' : 'Current'}</strong></span><span>Sync <strong>{node.sync}</strong></span><span>Consensus <strong>{node.health === 'Unhealthy' ? 'Unknown' : 'Current'}</strong></span><Link to={'/admin/nodes/' + node.id}>View Node ↗</Link></div>}</div>)}</div>
}

function AgentCard({ agent }: { agent: AgentRow }) {
  return <article className="agent-card-prototype"><div className="agent-card-head"><div><span className={'state-dot ' + agent.state.toLowerCase()} /><strong>{agent.id}</strong></div><span className={'state-label ' + agent.state.toLowerCase()}>{agent.state}</span></div><p>{agent.detail}</p><div className="agent-stats"><span><small>Nodes</small>{agent.nodes}</span><span><small>Host</small>{agent.host}</span><span><small>Spool</small>{agent.spool}</span></div><Link to={'/admin/agents/' + agent.id} className="prototype-link">View Agent ↗</Link></article>
}

function AgentLine({ agent }: { agent: AgentRow }) {
  return <div className="agent-line"><span className={'state-dot ' + agent.state.toLowerCase()} /><strong>{agent.id}</strong><span>{agent.nodes}</span><span className={'state-label ' + agent.state.toLowerCase()}>{agent.state}</span></div>
}

function Metric({ label, value, note, tone }: { label: string; value: string; note: string; tone: string }) {
  return <div className={'b-metric ' + tone}><span className="eyebrow">{label}</span><strong>{value}</strong><small>{note}</small></div>
}

function Kpi({ label, value, detail, accent, link }: { label: string; value: string; detail: string; accent: string; link: string }) {
  return <Link to={link} className={'c-kpi c-kpi-' + accent}><span className="eyebrow">{label}</span><strong>{value}</strong><span>{detail}</span><span className="card-arrow">↗</span></Link>
}

function PrototypeSwitcher({ variant }: { variant: OverviewPrototypeVariant }) {
  const [searchParams, setSearchParams] = useSearchParams()
  const index = VARIANTS.findIndex((item) => item.key === variant)
  const change = (offset: number) => {
    const next = new URLSearchParams(searchParams)
    next.set('variant', VARIANTS[(index + offset + VARIANTS.length) % VARIANTS.length].key)
    setSearchParams(next)
  }
  return <nav className="prototype-switcher" aria-label="Prototype variants"><button type="button" onClick={() => change(-1)} aria-label="Previous prototype variant">←</button><span><b>{variant}</b> · {VARIANTS[index].name}</span><button type="button" onClick={() => change(1)} aria-label="Next prototype variant">→</button></nav>
}
