import { useEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from 'react'
import { Link, useSearchParams } from 'react-router'

type Scenario =
  | 'healthy'
  | 'freshness'
  | 'transfer'
  | 'alerts'
  | 'maintenance'
  | 'auth'

type Screen = 'overview' | 'transfer' | 'alerts' | 'data' | 'access'

type TransferState = 'Pending' | 'Completed' | 'Cancelled' | 'Expired'

type OperationState =
  | 'Queued'
  | 'Running'
  | 'Succeeded'
  | 'SucceededWithWarnings'
  | 'Failed'

type DemoState = {
  transfer: TransferState
  operation: OperationState
  delivery: 'Delivered' | 'RetryScheduled' | 'DeadLetter'
  silenced: boolean
  maintenance: boolean
  live: boolean
  session: 'active' | 'revoked'
}

const scenarioLabels: Record<Scenario, string> = {
  healthy: 'Healthy Owner',
  freshness: 'Freshness degradation',
  transfer: 'Node Transfer',
  alerts: 'Alert suppression',
  maintenance: 'Backup / Restore / Doctor',
  auth: 'Authorization change',
}

const screenLabels: Record<Screen, string> = {
  overview: 'Overview',
  transfer: 'Transfer',
  alerts: 'Alerts',
  data: 'Data & Maintenance',
  access: 'Access & Audit',
}

const initialState: DemoState = {
  transfer: 'Pending',
  operation: 'Running',
  delivery: 'RetryScheduled',
  silenced: false,
  maintenance: false,
  live: true,
  session: 'active',
}

function isScenario(value: string | null): value is Scenario {
  return value != null && value in scenarioLabels
}

function isScreen(value: string | null): value is Screen {
  return value != null && value in screenLabels
}

export default function Phase2Prototype() {
  const [params, setParams] = useSearchParams()
  const scenario = isScenario(params.get('scenario'))
    ? (params.get('scenario') as Scenario)
    : 'healthy'
  const screen = isScreen(params.get('screen'))
    ? (params.get('screen') as Screen)
    : 'overview'
  const [drawerOpen, setDrawerOpen] = useState(false)
  const closeButtonRef = useRef<HTMLButtonElement>(null)
  const sidebarRef = useRef<HTMLElement>(null)
  const [state, setState] = useState<DemoState>({ ...initialState })
  const [notice, setNotice] = useState<string | null>(null)

  useEffect(() => {
    if (!drawerOpen) return
    const previousOverflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    closeButtonRef.current?.focus()
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        setDrawerOpen(false)
        return
      }
      if (event.key !== 'Tab' || !sidebarRef.current) return
      const focusable = Array.from(sidebarRef.current.querySelectorAll<HTMLElement>('button, a, select, input, textarea')).filter((element) => !element.hasAttribute('disabled'))
      if (focusable.length === 0) return
      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }
    document.addEventListener('keydown', onKeyDown)
    return () => {
      document.body.style.overflow = previousOverflow
      document.removeEventListener('keydown', onKeyDown)
    }
  }, [drawerOpen])

  const setRoute = (next: Partial<{ scenario: Scenario; screen: Screen }>) => {
    const nextParams = new URLSearchParams(params)
    if (next.scenario) nextParams.set('scenario', next.scenario)
    if (next.screen) nextParams.set('screen', next.screen)
    nextParams.set('prototype', 'phase2')
    setParams(nextParams)
    setDrawerOpen(false)
  }

  const setScenario = (next: Scenario) => {
    setState({ ...initialState, session: next === 'auth' ? 'revoked' : 'active' })
    setRoute({ scenario: next })
    setNotice(`Loaded scenario: ${scenarioLabels[next]}`)
  }

  const announce = (message: string) => setNotice(message)

  const resetDemo = () => {
    setState({ ...initialState })
    setNotice('Demo state reset. REST mock data is authoritative.')
  }

  const banner = useMemo(() => {
    if (scenario === 'freshness') {
      return { tone: 'warning', title: 'Some data needs attention', body: 'Node atlas-01 has an RPC error. Last successful value remains visible and is now stale.' }
    }
    if (scenario === 'auth' || state.session === 'revoked') {
      return { tone: 'danger', title: 'Session access changed', body: 'The Admin stream is closed and protected data has been cleared.' }
    }
    if (!state.live) {
      return { tone: 'warning', title: 'Live updates paused', body: 'REST remains available. Realtime refetch resumes when the connection is restored.' }
    }
    return null
  }, [scenario, state.live, state.session])

  return (
    <div className="prototype-shell">
      <header className="prototype-topbar">
        <button
          className="prototype-menu-button"
          type="button"
          aria-label="Open Admin navigation"
          aria-expanded={drawerOpen}
          onClick={() => setDrawerOpen(true)}
        >
          <span aria-hidden="true">☰</span>
        </button>
        <Link className="prototype-brand" to="/admin?prototype=phase2">
          <span className="prototype-brand-mark" aria-hidden="true">P</span>
          <span>PlatPulse <small>prototype</small></span>
        </Link>
        <div className="prototype-top-actions">
          <span className="prototype-live-indicator" data-live={state.live}>
            <span aria-hidden="true">●</span> {state.live ? 'Live' : 'Paused'}
          </span>
          <button className="prototype-quiet-button" type="button" onClick={resetDemo}>Reset demo</button>
        </div>
      </header>

      <aside ref={sidebarRef} className={`prototype-sidebar${drawerOpen ? ' is-open' : ''}`} aria-label="Admin navigation">
        <div className="prototype-sidebar-heading">
          <span className="eyebrow">Owner workspace</span>
          <button ref={closeButtonRef} type="button" className="prototype-close-button" onClick={() => setDrawerOpen(false)} aria-label="Close navigation">×</button>
        </div>
        <nav className="prototype-nav">
          {(Object.keys(screenLabels) as Screen[]).map((item) => (
            <button
              type="button"
              className={screen === item ? 'is-active' : ''}
              key={item}
              aria-current={screen === item ? 'page' : undefined}
              onClick={() => setRoute({ screen: item })}
            >
              <span className="prototype-nav-icon" aria-hidden="true">{item === 'overview' ? '⌂' : item === 'transfer' ? '↔' : item === 'alerts' ? '!' : item === 'data' ? '▣' : '◉'}</span>
              {screenLabels[item]}
              {item === 'alerts' && <span className="prototype-nav-count">3</span>}
            </button>
          ))}
        </nav>
        <div className="prototype-sidebar-footer">
          <span className="avatar" aria-hidden="true">MO</span>
          <span><strong>mowind</strong><small>Owner</small></span>
        </div>
      </aside>

      {drawerOpen && <button className="prototype-drawer-scrim" type="button" aria-label="Close navigation" onClick={() => setDrawerOpen(false)} />}

      <main className="prototype-content">
        <div className="prototype-content-header">
          <div>
            <span className="eyebrow">Admin / {screenLabels[screen]}</span>
            <h1>{screen === 'overview' ? 'Good morning, Owner' : screenLabels[screen]}</h1>
            <p className="prototype-subtitle">Operational truth for your PlatON deployment.</p>
          </div>
          <div className="prototype-header-context">
            <label htmlFor="prototype-scenario">Scenario</label>
            <select id="prototype-scenario" value={scenario} onChange={(event) => setScenario(event.target.value as Scenario)}>
              {(Object.keys(scenarioLabels) as Scenario[]).map((item) => <option value={item} key={item}>{scenarioLabels[item]}</option>)}
            </select>
          </div>
        </div>

        {banner && <div className={`prototype-banner ${banner.tone}`} role={banner.tone === 'danger' ? 'alert' : 'status'}><strong>{banner.title}</strong><span>{banner.body}</span></div>}
        {notice && <div className="prototype-notice" role="status"><span>{notice}</span><button type="button" onClick={() => setNotice(null)} aria-label="Dismiss notice">×</button></div>}

        {screen === 'overview' && <OverviewScreen scenario={scenario} state={state} setState={setState} announce={announce} setRoute={setRoute} />}
        {screen === 'transfer' && <TransferScreen state={state} setState={setState} announce={announce} />}
        {screen === 'alerts' && <AlertsScreen state={state} setState={setState} announce={announce} />}
        {screen === 'data' && <DataScreen state={state} setState={setState} announce={announce} />}
        {screen === 'access' && <AccessScreen state={state} setState={setState} announce={announce} />}

        <footer className="prototype-footer">
          <span>PROTOTYPE — in-memory mock, no production API</span>
          <button type="button" onClick={() => setState((current) => ({ ...current, live: !current.live }))}>
            {state.live ? 'Pause mock SSE' : 'Reconnect mock SSE'}
          </button>
        </footer>
      </main>
    </div>
  )
}

function OverviewScreen({ scenario, state, setState, announce, setRoute }: { scenario: Scenario; state: DemoState; setState: Dispatch<SetStateAction<DemoState>>; announce: (message: string) => void; setRoute: (next: { screen?: Screen; scenario?: Scenario }) => void }) {
  const degraded = scenario === 'freshness'
  const health = degraded ? 'Warning' : scenario === 'auth' ? 'Unknown' : 'Healthy'
  return (
    <>
      <section className="attention-grid" aria-label="Attention queue">
        <article className="attention-card attention-card-primary">
          <div className="attention-card-heading"><span className="eyebrow">Attention queue</span><span className="attention-number">{degraded ? '3' : '1'}</span></div>
          <h2>{degraded ? 'Data needs a closer look' : 'One item worth checking'}</h2>
          <p>{degraded ? 'RPC, stale observation, and a retrying delivery need attention.' : 'A notification delivery is scheduled to retry.'}</p>
          <button type="button" className="text-action" onClick={() => setRoute({ screen: degraded ? 'overview' : 'alerts' })}>Review attention <span aria-hidden="true">→</span></button>
        </article>
        <article className="metric-card"><span className="eyebrow">Network health</span><strong className="metric-value">{health}</strong><span className="metric-reason">{degraded ? '1 Node is stale' : 'All reported dimensions known'}</span></article>
        <article className="metric-card"><span className="eyebrow">Active Nodes</span><strong className="metric-value">04</strong><span className="metric-reason">Across 2 Networks</span></article>
        <article className="metric-card"><span className="eyebrow">Open incidents</span><strong className="metric-value">{degraded ? '02' : '01'}</strong><span className="metric-reason">1 delivery retrying</span></article>
      </section>

      <section className="prototype-panel">
        <div className="panel-heading"><div><span className="eyebrow">Node inventory</span><h2>Current operational view</h2></div><button className="secondary-button" type="button" onClick={() => announce('Refreshing Node inventory…')}>Refresh</button></div>
        <div className="prototype-table-wrap"><table className="prototype-table"><caption className="sr-only">PlatON Node operational status</caption><thead><tr><th scope="col">Node</th><th scope="col">Network</th><th scope="col">Health</th><th scope="col">Head / Sync</th><th scope="col">Freshness</th><th scope="col"><span className="sr-only">Action</span></th></tr></thead><tbody>
          <NodeRow name="atlas-01" network="Mainnet" health={health} healthTone={degraded ? 'warning' : 'healthy'} head="12,842,019" freshness={degraded ? 'Error · 18m stale' : 'Current · 32s ago'} action={degraded ? 'Last successful: RPC peers 12' : 'All dimensions current'} />
          <NodeRow name="atlas-02" network="Mainnet" health="Healthy" healthTone="healthy" head="12,842,021" freshness="Current · 28s ago" action="Process selector disabled" />
          <NodeRow name="testnet-a" network="Testnet" health="Unknown" healthTone="unknown" head="—" freshness="Never observed" action="RPC probe pending" />
        </tbody></table></div>
      </section>

      <section className="two-column-grid">
        <article className="prototype-panel compact-panel"><div className="panel-heading"><div><span className="eyebrow">Realtime contract</span><h2>REST is authoritative</h2></div><span className={`status-pill ${state.live ? 'healthy' : 'warning'}`}>{state.live ? 'SSE connected' : 'Live updates paused'}</span></div><p className="panel-copy">Invalidation events refetch exact REST resources. The demo never pushes a full state payload.</p><button type="button" className="text-action" onClick={() => setState((current) => ({ ...current, live: !current.live }))}>{state.live ? 'Pause live updates' : 'Reconnect live updates'} <span aria-hidden="true">→</span></button></article>
        <article className="prototype-panel compact-panel"><div className="panel-heading"><div><span className="eyebrow">Next action</span><h2>Review the operations loop</h2></div></div><ul className="link-list"><li><button type="button" onClick={() => setRoute({ screen: 'transfer' })}>Node Transfer <span>Pending declaration</span></button></li><li><button type="button" onClick={() => setRoute({ screen: 'alerts' })}>Alert delivery <span>Retry scheduled</span></button></li><li><button type="button" onClick={() => setRoute({ screen: 'data' })}>Doctor checks <span>1 warning</span></button></li></ul></article>
      </section>
    </>
  )
}

function NodeRow({ name, network, health, healthTone, head, freshness, action }: { name: string; network: string; health: string; healthTone: 'healthy' | 'warning' | 'unknown'; head: string; freshness: string; action: string }) {
  return <tr><th scope="row"><strong>{name}</strong><small>Node ID · 7bbd…ba2f3</small></th><td data-label="Network">{network}</td><td data-label="Health"><span className={`status-pill ${healthTone}`}>{health}</span></td><td data-label="Head / Sync">{head}<small>Sync current</small></td><td data-label="Freshness"><strong>{freshness.split(' · ')[0]}</strong><small>{freshness.split(' · ')[1]}</small></td><td data-label="Reason"><span className="muted">{action}</span></td></tr>
}

function TransferScreen({ state, setState, announce }: { state: DemoState; setState: Dispatch<SetStateAction<DemoState>>; announce: (message: string) => void }) {
  const submitTransfer = () => {
    setState((current) => ({ ...current, transfer: 'Completed' }))
    announce('Transfer completed. Target Agent declared the Node and ownership switched atomically.')
  }
  return <>
    <div className="breadcrumb">Agents & Nodes <span>/</span> Node atlas-01 <span>/</span> Transfer</div>
    <section className="workflow-layout"><div className="workflow-main">
      <article className="prototype-panel"><div className="panel-heading"><div><span className="eyebrow">Node Transfer</span><h2>Move atlas-01 to another Agent</h2></div><span className={`status-pill ${state.transfer === 'Completed' ? 'healthy' : 'warning'}`}>{state.transfer}</span></div><p className="panel-copy">The source Agent remains authoritative until the target Agent declares this exact Node ID and passes Network Identity validation.</p>
        <div className="transfer-path"><div><span className="eyebrow">Source Agent</span><strong>agent-west-01</strong><small>Authoritative now</small></div><span className="transfer-arrow" aria-hidden="true">→</span><div><span className="eyebrow">Target Agent</span><strong>agent-east-02</strong><small>Declaration {state.transfer === 'Completed' ? 'accepted' : 'pending'}</small></div></div>
        <div className="field"><label htmlFor="transfer-reason">Operator reason</label><textarea id="transfer-reason" defaultValue="Move validator workload to the east host." rows={3} /></div>
        <div className="field-row"><div className="field"><label htmlFor="transfer-expiry">Pending until</label><input id="transfer-expiry" type="datetime-local" defaultValue="2026-08-18T18:00" /></div><div className="field"><label htmlFor="transfer-target">Target Agent</label><select id="transfer-target" defaultValue="agent-east-02"><option>agent-east-02 · Ready</option><option disabled>agent-south-01 · Transfer already pending</option></select></div></div>
        <div className="confirmation-box"><strong>Before you continue</strong><ul><li>Network identity: <b>matches platon-mainnet</b></li><li>Node history and Node ID remain unchanged</li><li>Server will not push an Endpoint or command</li></ul></div>
        <div className="action-row"><button className="primary-button" type="button" onClick={submitTransfer} disabled={state.transfer === 'Completed'}>{state.transfer === 'Completed' ? 'Transfer complete' : 'Confirm pending transfer'}</button><button className="secondary-button" type="button" onClick={() => { setState((current) => ({ ...current, transfer: 'Cancelled' })); announce('Pending transfer cancelled. Source Agent remains authoritative.') }}>Cancel transfer</button></div>
      </article>
    </div><aside className="workflow-side"><article className="prototype-panel"><span className="eyebrow">Transfer timeline</span><ol className="timeline"><li className="done"><strong>Created</strong><small>Owner · 2 min ago</small></li><li className={state.transfer === 'Completed' ? 'done' : 'current'}><strong>Target declaration</strong><small>{state.transfer === 'Completed' ? 'Accepted · just now' : 'Waiting for valid Inventory'}</small></li><li className={state.transfer === 'Completed' ? 'done' : ''}><strong>Ownership switch</strong><small>{state.transfer === 'Completed' ? 'Atomic Server commit' : 'Not started'}</small></li></ol></article><article className="prototype-panel"><span className="eyebrow">Audit</span><p className="panel-copy">Every create, cancel, expiry, rejection, and completion is immutable and linked to request <code>0195…a84f</code>.</p><button type="button" className="text-action" onClick={() => announce('Audit Event opened in the prototype.')}>View Audit Event →</button></article></aside></section>
  </>
}

function AlertsScreen({ state, setState, announce }: { state: DemoState; setState: Dispatch<SetStateAction<DemoState>>; announce: (message: string) => void }) {
  const silence = () => { setState((current) => ({ ...current, silenced: !current.silenced })); announce(state.silenced ? 'Silence cancelled. Delivery policy is active.' : 'Silence active until 18:00. Incident evaluation continues.') }
  const maintenance = () => { setState((current) => ({ ...current, maintenance: !current.maintenance })); announce(state.maintenance ? 'Maintenance window ended. Current facts will be re-evaluated.' : 'Maintenance window active for atlas-01 until 18:00.') }
  const retry = () => { setState((current) => ({ ...current, delivery: 'Delivered' })); announce('Delivery succeeded on manual retry. Notification Event was not duplicated.') }
  return <>
    <section className="alert-summary-grid"><article className="metric-card"><span className="eyebrow">Open Incidents</span><strong className="metric-value">02</strong><span className="metric-reason">1 evaluation unavailable</span></article><article className="metric-card"><span className="eyebrow">Suppressed</span><strong className="metric-value">{state.silenced || state.maintenance ? '01' : '00'}</strong><span className="metric-reason">Evaluation still running</span></article><article className="metric-card"><span className="eyebrow">Deliveries</span><strong className="metric-value">{state.delivery === 'Delivered' ? 'OK' : '01'}</strong><span className="metric-reason">At-least-once outbox</span></article></section>
    <section className="workflow-layout"><div className="workflow-main"><article className="prototype-panel"><div className="panel-heading"><div><span className="eyebrow">Incident · Open</span><h2>Node RPC unreachable</h2></div><span className="status-pill critical">Critical</span></div><div className="incident-meta"><span>Subject <strong>atlas-01</strong></span><span>Rule <strong>node.rpc_unreachable v4</strong></span><span>Opened <strong>14 min ago</strong></span></div><div className="incident-callout"><strong>Evaluation unavailable</strong><span>RPC probe failed. Last successful reachability was observed 18 minutes ago. This Incident remains open; Unknown cannot resolve it.</span></div><div className="action-row"><button className="primary-button" type="button" onClick={silence}>{state.silenced ? 'Cancel Silence' : 'Silence Incident'}</button><button className="secondary-button" type="button" onClick={maintenance}>{state.maintenance ? 'End Maintenance' : 'Schedule Maintenance'}</button></div></article><article className="prototype-panel"><div className="panel-heading"><div><span className="eyebrow">Delivery</span><h2>Telegram · On-call</h2></div><span className={`status-pill ${state.delivery === 'Delivered' ? 'healthy' : 'warning'}`}>{state.delivery}</span></div><dl className="detail-list"><dt>Destination</dt><dd>•••••• 4821</dd><dt>Attempts</dt><dd>{state.delivery === 'Delivered' ? '3 · delivered' : '2 · next retry in 4m'}</dd><dt>Idempotency</dt><dd><code>incident-184:telegram:on-call</code></dd><dt>Suppression</dt><dd>{state.silenced ? 'Silence until 18:00' : state.maintenance ? 'Maintenance until 18:00' : 'None'}</dd></dl><button className="text-action" type="button" onClick={retry} disabled={state.delivery === 'Delivered'}>{state.delivery === 'Delivered' ? 'Delivery complete' : 'Manual retry'} <span aria-hidden="true">→</span></button></article></div><aside className="workflow-side"><article className="prototype-panel"><span className="eyebrow">Incident timeline</span><ol className="timeline"><li className="done"><strong>Pending</strong><small>for 5m satisfied</small></li><li className="current"><strong>Open</strong><small>Threshold exceeded · 14m ago</small></li><li><strong>Recovering</strong><small>Requires fresh Known recovery</small></li><li><strong>Resolved</strong><small>Never resolved by Unknown</small></li></ol></article><article className="prototype-panel"><span className="eyebrow">Policy state</span><div className="policy-row"><span>Silence</span><strong>{state.silenced ? 'Active' : 'Not active'}</strong></div><div className="policy-row"><span>Maintenance</span><strong>{state.maintenance ? 'Active' : 'Not active'}</strong></div><p className="muted">Both suppress delivery only. Rule evaluation and Incident facts remain durable.</p></article></aside></section>
  </>
}

function DataScreen({ state, setState, announce }: { state: DemoState; setState: Dispatch<SetStateAction<DemoState>>; announce: (message: string) => void }) {
  const runDoctor = () => { setState((current) => ({ ...current, operation: 'SucceededWithWarnings' })); announce('Doctor completed with 1 warning. No automatic fixes were applied.') }
  const createBackup = () => { setState((current) => ({ ...current, operation: 'Succeeded' })); announce('Backup completed and checksum verified.') }
  const restore = () => { setState((current) => ({ ...current, operation: 'Failed' })); announce('Restore refused: Server must be stopped before exclusive database restore.') }
  return <><section className="data-overview-grid"><article className="metric-card"><span className="eyebrow">Database</span><strong className="metric-value">Ready</strong><span className="metric-reason">SQLite · WAL checkpointed</span></article><article className="metric-card"><span className="eyebrow">Latest backup</span><strong className="metric-value">Verified</strong><span className="metric-reason">2h ago · 18.4 MB</span></article><article className="metric-card"><span className="eyebrow">Doctor</span><strong className="metric-value">{state.operation === 'SucceededWithWarnings' ? '1 warning' : 'Ready'}</strong><span className="metric-reason">No automatic fixes</span></article></section><section className="workflow-layout"><div className="workflow-main"><article className="prototype-panel"><div className="panel-heading"><div><span className="eyebrow">Data operations</span><h2>Retention and aggregate health</h2></div><span className={`status-pill ${state.operation === 'Failed' ? 'critical' : state.operation === 'SucceededWithWarnings' ? 'warning' : 'healthy'}`}>{state.operation}</span></div><div className="retention-list"><div><strong>Raw Block Summary</strong><span>7 days · safe minimum 1 day</span><b>Next cleanup in 42m</b></div><div><strong>1-minute aggregate</strong><span>90 days · current through 14:05</span><b>Healthy</b></div><div><strong>Audit Event</strong><span>365 days · protected</span><b>Cannot be deleted here</b></div></div><div className="action-row"><button className="secondary-button" type="button" onClick={() => announce('Retention edit preview: estimated 1.2 GB reclaimed, minimums preserved.')}>Preview retention edit</button><button className="secondary-button" type="button" onClick={runDoctor}>Run Doctor</button></div></article><article className="prototype-panel"><div className="panel-heading"><div><span className="eyebrow">Backup / Restore</span><h2>Safe recovery operations</h2></div><span className="operation-chip">Operation {state.operation}</span></div><div className="backup-artifact"><div><span className="eyebrow">Latest artifact</span><strong>backup-2026-08-18-1405</strong><span>18.4 MB · SHA-256 9fd2…c81a · schema v1</span></div><span className="status-pill healthy">Integrity verified</span></div><div className="action-row"><button className="primary-button" type="button" onClick={createBackup}>Create backup</button><button className="danger-button" type="button" onClick={restore}>Open restore flow</button></div><p className="muted">Restore requires an exclusive stopped Server. It never restores secret files and never overwrites the current DB after a failed integrity check.</p></article></div><aside className="workflow-side"><article className="prototype-panel"><span className="eyebrow">Doctor result</span><ul className="check-list"><li><span className="check-mark pass">✓</span><span><strong>Database integrity</strong><small>Pass · 14:05 UTC</small></span></li><li><span className="check-mark warning">!</span><span><strong>Notification outbox</strong><small>Warning · 1 retry scheduled</small></span></li><li><span className="check-mark pass">✓</span><span><strong>Web assets</strong><small>Pass · index and assets present</small></span></li><li><span className="check-mark pass">✓</span><span><strong>Secret permissions</strong><small>Pass · content not displayed</small></span></li></ul><button type="button" className="text-action" onClick={runDoctor}>Run again →</button></article><article className="prototype-panel"><span className="eyebrow">Audit boundary</span><p className="panel-copy">Operations record progress. Audit records the Owner action. Both are linked; neither reveals secret file contents.</p></article></aside></section></>
}

function AccessScreen({ state, setState, announce }: { state: DemoState; setState: Dispatch<SetStateAction<DemoState>>; announce: (message: string) => void }) {
  const revoke = () => { setState((current) => ({ ...current, session: 'revoked', live: false })); announce('Session revoked. Admin cache cleared and stream closed.') }
  if (state.session === 'revoked') return <section className="session-card"><div className="session-icon" aria-hidden="true">↗</div><span className="eyebrow">Session revoked</span><h2>Check your access</h2><p>Your Admin session changed while this page was open. Protected data was cleared before the route transition.</p><div className="field prototype-login-field"><label htmlFor="prototype-login-username">Username</label><input id="prototype-login-username" defaultValue="owner" autoComplete="username" /></div><div className="field prototype-login-field"><label htmlFor="prototype-login-password">Password</label><input id="prototype-login-password" type="password" placeholder="Enter password" autoComplete="current-password" /></div><button className="primary-button" type="button" onClick={() => { setState((current) => ({ ...current, session: 'active', live: true })); announce('Session revalidated. Admin stream reconnected.') }}>Sign in as Owner</button><p className="muted">Safe return: <code>/admin?prototype=phase2&amp;screen=access</code></p></section>
  return <section className="workflow-layout"><div className="workflow-main"><article className="prototype-panel"><div className="panel-heading"><div><span className="eyebrow">Access & Audit</span><h2>Owner sessions</h2></div><span className="status-pill healthy">Owner</span></div><div className="session-row"><span className="avatar">MO</span><span><strong>Current session</strong><small>Created 12m ago · Active now · Chromium on Linux</small></span><span className="status-pill healthy">Current</span></div><div className="session-row"><span className="avatar muted-avatar">T1</span><span><strong>Other session</strong><small>Last active 2h ago · Firefox · coarse location only</small></span><button className="secondary-button" type="button" onClick={() => announce('Other session revoked.')}>Revoke</button></div><div className="action-row"><button className="danger-button" type="button" onClick={revoke}>Revoke all except current</button><button className="secondary-button" type="button" onClick={() => announce('Logout uses the separate current-session flow.')}>Sign out current</button></div></article><article className="prototype-panel"><div className="panel-heading"><div><span className="eyebrow">Recent Audit</span><h2>Security actions</h2></div><button className="text-action" type="button" onClick={() => announce('Audit list opened.')}>View all →</button></div><div className="audit-row"><strong>Node Transfer completed</strong><span>Owner · request 0195…a84f · just now</span></div><div className="audit-row"><strong>Backup integrity verified</strong><span>local-cli · 2h ago</span></div></article></div><aside className="workflow-side"><article className="prototype-panel"><span className="eyebrow">Login state</span><h2>Owner access is explicit</h2><p className="panel-copy">A session/role change closes the stream, clears Admin DTOs, and never flashes old data while access is checked.</p><button className="text-action" type="button" onClick={revoke}>Simulate revoke →</button></article></aside></section>
}
