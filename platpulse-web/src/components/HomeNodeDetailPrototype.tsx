import type { ReactNode } from 'react'
import { Link } from 'react-router'
import type { PublicNode } from '../api/generated'
import PrototypeSwitcher, { type PrototypeVariant } from './PrototypeSwitcher'

type BlockHistory = Array<{
  blockTimeMs?: number | null
  divergenceKind?: string | null
  divergenceReason?: string | null
  freshness?: string | null
  gapFromHeight?: number | null
  gapKind?: string | null
  gapReason?: string | null
  gapToHeight?: number | null
  height?: number | null
  observedAt?: string | null
  transactionCount?: number | null
}>

type HomeNodeDetailPrototypeProps = {
  node: PublicNode
  history: BlockHistory
}

const variants: readonly PrototypeVariant[] = [
  { key: 'A', label: 'Signal stack' },
  { key: 'B', label: 'Mission control' },
  { key: 'C', label: 'Evidence ledger' },
]

/** PROTOTYPE — three throwaway directions for PAGE-HOME-NODE / SCN-HOME-NODE-DETAIL. */
export default function HomeNodeDetailPrototype({ node, history }: HomeNodeDetailPrototypeProps) {
  const variant = new URLSearchParams(window.location.search).get('variant')?.toUpperCase() ?? 'A'
  const current = variants.some((item) => item.key === variant) ? variant : 'A'

  return (
    <>
      {current === 'A' && <SignalStack node={node} history={history} />}
      {current === 'B' && <MissionControl node={node} history={history} />}
      {current === 'C' && <EvidenceLedger node={node} history={history} />}
      <PrototypeSwitcher current={current} variants={variants} />
    </>
  )
}

function PageFrame({ node, children, className = '' }: { node: PublicNode; children: ReactNode; className?: string }) {
  return <section className={'page node-prototype ' + className} data-prototype="home-node-detail">
    <p className="proto-breadcrumb"><Link to={'/networks/' + node.networkKey}>← {node.networkKey}</Link><span>Home / Node detail</span></p>
    {children}
  </section>
}

function NodeIdentity({ node, eyebrow = 'PLATON NODE' }: { node: PublicNode; eyebrow?: string }) {
  return <div>
    <p className="proto-eyebrow">{eyebrow}</p>
    <h1>{node.displayName ?? 'Unnamed Node'}</h1>
    <p className="proto-node-id">{node.nodeId}</p>
  </div>
}

function StatusMark({ value, label = value }: { value: string; label?: string }) {
  const tone = statusTone(value)
  const symbol = tone === 'good' ? '✓' : tone === 'bad' ? '!' : tone === 'warn' ? '◐' : '–'
  return <span className={'proto-status proto-status-' + tone}><span aria-hidden="true">{symbol}</span>{label}</span>
}

function SignalRow({ label, value, detail, wide = false }: { label: string; value: string; detail?: string; wide?: boolean }) {
  return <div className={'proto-signal-row' + (wide ? ' proto-signal-row-wide' : '')}>
    <div className="proto-signal-label"><span>{label}</span><StatusMark value={value} label={value} /></div>
    {detail && <p>{detail}</p>}
  </div>
}

function Metric({ label, value, detail }: { label: string; value: string; detail?: string }) {
  return <div className="proto-metric"><dt>{label}</dt><dd>{value}</dd>{detail && <small>{detail}</small>}</div>
}

function HealthHero({ node, mode }: { node: PublicNode; mode: 'light' | 'dark' }) {
  return <div className={'proto-health-hero proto-health-hero-' + mode}>
    <div className="proto-health-orbit"><span aria-hidden="true">{statusTone(node.health) === 'good' ? '✓' : '!'}</span></div>
    <div><p className="proto-eyebrow">SERVER-OWNED HEALTH SUMMARY</p><strong>{node.health}</strong><p>{node.healthReason}</p></div>
  </div>
}

function SharedMetrics({ node }: { node: PublicNode }) {
  return <dl className="proto-metrics">
    <Metric label="Current head" value={formatNumber(node.currentHead)} detail={'Sync ' + safeText(node.syncState)} />
    <Metric label="Network reference" value={formatNumber(node.networkReferenceHead)} detail={safeText(node.networkReferenceConfidence)} />
    <Metric label="Freshness" value={safeText(node.freshness, 'Never observed')} detail="Server timestamp" />
    <Metric label="History window" value={formatNumber(node.historicalHighWatermark)} detail="Historical high-water mark" />
  </dl>
}

function PeerSummary({ node }: { node: PublicNode }) {
  const peers = node.peers
  const peerValue = peers.peerCount == null ? 'Unknown' : String(peers.peerCount)
  const detail = peers.peerCount == null ? 'No successful Peer Count Observation' : safeText(peers.freshness, 'Freshness unavailable')
  return <section className="proto-side-section" aria-labelledby="proto-peer-title">
    <div className="proto-section-kicker"><span className="proto-section-number">03</span><h2 id="proto-peer-title">Peer Count</h2></div>
    <p className="proto-big-number">{peerValue}</p>
    <p className="muted">{detail}</p>
    <p className="proto-note">Peer identities and Presence data are intentionally not part of Home.</p>
  </section>
}

function HostSummary({ node }: { node: PublicNode }) {
  return <section className="proto-side-section" aria-labelledby="proto-host-title">
    <div className="proto-section-kicker"><span className="proto-section-number">04</span><h2 id="proto-host-title">Shared Host</h2></div>
    <div className="proto-host-value"><strong>{node.hostCpuPercent == null ? 'Unknown' : node.hostCpuPercent.toFixed(1) + '%'}</strong><span>CPU</span></div>
    <p className="muted">Sanitized percentage from the Agent Host Observation. Not duplicated per Node.</p>
  </section>
}

function HistoryPanel({ history, compact = false }: { history: BlockHistory; compact?: boolean }) {
  const items = history.slice(0, compact ? 4 : 6)
  return <section className={'proto-history ' + (compact ? 'proto-history-compact' : '')} aria-labelledby="proto-history-title">
    <header className="proto-section-heading"><div><p className="proto-eyebrow">RECENT EVIDENCE</p><h2 id="proto-history-title">Block History</h2></div><span className="proto-window-label">Server bounded</span></header>
    {items.length === 0 ? <p className="proto-empty">No Block History observed yet. Absence is not zero activity.</p> : <ol className="proto-history-list">
      {items.map((block, index) => <li key={(block.height ?? 'missing') + '-' + index}>
        <span className="proto-history-dot" aria-hidden="true" />
        <div><strong>{block.height == null ? 'Height unknown' : 'Block ' + formatNumber(block.height)}</strong><span>{block.transactionCount == null ? 'Transactions not observed' : block.transactionCount + ' transactions'}</span></div>
        <time>{formatTime(block.observedAt)}</time>
      </li>)}
    </ol>}
    <p className="proto-history-footnote">Missing blocks remain absent. The high-water mark is {formatNumber(history[0]?.height ?? null)} in this sample.</p>
  </section>
}

function SignalStack({ node, history }: HomeNodeDetailPrototypeProps) {
  return <PageFrame node={node} className="node-prototype-a">
    <div className="proto-a-heading"><NodeIdentity node={node} /><HealthHero node={node} mode="light" /></div>
    <SharedMetrics node={node} />
    <div className="proto-a-columns">
      <main>
        <section className="proto-panel" aria-labelledby="proto-signals-title">
          <header className="proto-section-heading"><div><p className="proto-eyebrow">INDEPENDENT OBSERVATIONS</p><h2 id="proto-signals-title">Node signals</h2></div><span className="proto-panel-meta">Last report: {safeText(node.freshness, 'Never')}</span></header>
          <div className="proto-signal-grid">
            <SignalRow label="RPC endpoint" value={node.rpcState} detail="Collection state" />
            <SignalRow label="Chain sync" value={node.syncState} detail={'Head ' + formatNumber(node.currentHead)} />
            <SignalRow label="Consensus" value={node.consensusState} detail="Node chain observation" />
            <SignalRow label="Node process" value={node.processState} detail="Process observation" />
          </div>
        </section>
        <HistoryPanel history={history} />
      </main>
      <aside className="proto-side-rail"><PeerSummary node={node} /><HostSummary node={node} /><section className="proto-side-section"><div className="proto-section-kicker"><span className="proto-section-number">05</span><h2>Resync</h2></div><StatusMark value={node.resyncState} /><p className="muted">{node.resyncProgress ?? 'No active resync progress reported.'}</p></section></aside>
    </div>
  </PageFrame>
}

function MissionControl({ node, history }: HomeNodeDetailPrototypeProps) {
  return <PageFrame node={node} className="node-prototype-b">
    <div className="proto-b-workspace">
      <aside className="proto-b-rail"><Link className="proto-b-back" to={'/networks/' + node.networkKey}>← Network</Link><div className="proto-b-rail-title"><span className="proto-rail-marker" aria-hidden="true" /><div><p className="proto-eyebrow">NODE CONTROL</p><strong>{node.displayName ?? 'Unnamed Node'}</strong></div></div><HealthHero node={node} mode="dark" /><nav aria-label="Node detail sections"><a href="#mission-signals">01 Signals</a><a href="#mission-history">02 History</a><a href="#mission-context">03 Context</a></nav></aside>
      <main className="proto-b-main">
        <div className="proto-b-topline"><NodeIdentity node={node} eyebrow="NETWORK / NODE" /><span className="proto-b-live"><span aria-hidden="true">●</span> Read-only projection</span></div>
        <section className="proto-b-command" id="mission-signals"><div className="proto-b-command-copy"><p className="proto-eyebrow">CURRENT OPERATING PICTURE</p><h2>{node.healthReason}</h2><p>Four dimensions stay separate so one collection error cannot hide the rest of the Node context.</p></div><div className="proto-b-head"><span>HEAD</span><strong>{formatNumber(node.currentHead)}</strong><small>{safeText(node.syncState)}</small></div></section>
        <section className="proto-ledger" aria-label="Node signal ledger"><SignalRow label="RPC" value={node.rpcState} detail="The endpoint collection dimension" wide /><SignalRow label="Sync" value={node.syncState} detail={'Network reference ' + formatNumber(node.networkReferenceHead)} wide /><SignalRow label="Consensus" value={node.consensusState} detail={safeText(node.networkReferenceConfidence)} wide /><SignalRow label="Process" value={node.processState} detail="Node process observation" wide /></section>
        <section id="mission-history"><HistoryPanel history={history} /></section>
        <section className="proto-b-context" id="mission-context"><div><p className="proto-eyebrow">CONTEXT</p><h2>What the public projection knows</h2></div><dl className="proto-context-list"><Metric label="Peer Count" value={node.peers.peerCount == null ? 'Unknown' : String(node.peers.peerCount)} detail={safeText(node.peers.freshness, 'No freshness')} /><Metric label="Host CPU" value={node.hostCpuPercent == null ? 'Unknown' : node.hostCpuPercent.toFixed(1) + '%'} detail="Shared Host observation" /><Metric label="Freshness" value={safeText(node.freshness, 'Never observed')} detail="Last accepted report" /></dl></section>
      </main>
    </div>
  </PageFrame>
}

function EvidenceLedger({ node, history }: HomeNodeDetailPrototypeProps) {
  const evidence = [
    ['01', 'Health summary', node.health, node.healthReason],
    ['02', 'Chain head', formatNumber(node.currentHead), 'Sync is ' + safeText(node.syncState)],
    ['03', 'Consensus', node.consensusState, 'Independent from RPC and Sync'],
    ['04', 'Process', node.processState, 'Node Process Observation'],
    ['05', 'Peers', node.peers.peerCount == null ? 'Unknown' : String(node.peers.peerCount), safeText(node.peers.freshness, 'No successful Peer Count Observation')],
  ]
  return <PageFrame node={node} className="node-prototype-c">
    <header className="proto-c-header"><div className="proto-c-index" aria-hidden="true">NODE<br />DETAIL</div><NodeIdentity node={node} eyebrow="PUBLIC PROJECTION / OBSERVATION LEDGER" /><div className="proto-c-health"><StatusMark value={node.health} /><span>{node.healthReason}</span></div></header>
    <div className="proto-c-body">
      <main><section className="proto-ledger-timeline" aria-labelledby="evidence-title"><header className="proto-section-heading"><div><p className="proto-eyebrow">OBSERVATION LEDGER</p><h2 id="evidence-title">Every signal, separately</h2></div><span className="proto-panel-meta">{safeText(node.freshness, 'Never observed')}</span></header>{evidence.map(([number, label, value, detail]) => <article className="proto-ledger-entry" key={number}><div className="proto-ledger-index">{number}</div><div className="proto-ledger-rule" aria-hidden="true" /><div className="proto-ledger-copy"><p className="proto-eyebrow">{label}</p><h3>{value}</h3><p>{detail}</p></div><StatusMark value={value} label="Observed" /></article>)}</section><HistoryPanel history={history} compact /></main>
      <aside className="proto-c-aside"><section><p className="proto-eyebrow">CURRENT HEAD</p><strong className="proto-c-head">{formatNumber(node.currentHead)}</strong><p>Network reference {formatNumber(node.networkReferenceHead)}</p></section><section><p className="proto-eyebrow">HOST PERCENTAGE</p><strong className="proto-c-head">{node.hostCpuPercent == null ? 'Unknown' : node.hostCpuPercent.toFixed(1) + '%'}</strong><p>Shared, sanitized Host Observation</p></section><section><p className="proto-eyebrow">HISTORY BOUNDARY</p><strong>{formatNumber(node.historicalHighWatermark)}</strong><p>Server window. No synthetic zeroes.</p></section></aside>
    </div>
  </PageFrame>
}

function statusTone(value: string): 'good' | 'warn' | 'bad' | 'neutral' {
  const normalized = value.toLowerCase()
  if (/(healthy|current|connected|ready|synced|active|running|ok|fresh)/.test(normalized)) return 'good'
  if (/(stale|starting|unknown|unsupported|disabled|empty|resync|degraded)/.test(normalized)) return 'warn'
  if (/(error|failed|unhealthy|offline|retired|unavailable)/.test(normalized)) return 'bad'
  return 'neutral'
}

function safeText(value: string | null | undefined, fallback = 'Unknown') {
  return value && value.trim() ? value : fallback
}

function formatNumber(value: number | null | undefined) {
  return value == null ? 'Unknown' : value.toLocaleString()
}

function formatTime(value: string | null | undefined) {
  if (!value) return 'Time unknown'
  const date = new Date(value)
  return Number.isNaN(date.valueOf()) ? value : date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
}
