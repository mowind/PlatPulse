import { Link, NavLink, Outlet } from 'react-router'
import { useEffect, useState } from 'react'
import SignOutButton from '../components/SignOutButton'
import { useAuth } from '../auth/AuthContext'
import { fetchNetworks } from '../api/public'
import type { PublicNetwork } from '../api/generated'
import { ServerStatusNotice } from '../components/ServerStatusNotice'

export default function HomeLayout() {
  const { status } = useAuth()
  const isOwner = status.state === 'authenticated' && status.session.role === 'owner'
  const [networks, setNetworks] = useState<PublicNetwork[]>([])
  const [error, setError] = useState<string | null>(null)
  useEffect(() => { fetchNetworks().then(setNetworks).catch((e: Error) => setError(e.message)) }, [])
  return (
    <div className="app-shell">
      <header className="app-header">
        <Link to="/" className="app-brand">PlatPulse</Link>
        <nav className="app-nav" aria-label="Primary">
          <NavLink to="/" end>Home</NavLink>
          {isOwner && <NavLink to="/admin">Admin</NavLink>}
        </nav>
        <SignOutButton />
      </header>
      <main className="app-main">
        <section className="page home-page">
          <ServerStatusNotice />
          <h1>Home</h1>
          <h2>Network overview</h2>
          <p>Published Nodes grouped by Network. Private and retired Nodes are not listed.</p>
          {error && <p role="status" className="form-error">{error}</p>}
          {networks.length === 0 && !error && <p role="status">No published Nodes yet.</p>}
          <div className="network-grid">
            {networks.map((network) => <section className="network-card" key={network.networkKey}>
              <h2><Link to={`/networks/${network.networkKey}`}>{network.displayName}</Link></h2>
              <p className="muted">{network.networkKey}</p>
              <ul className="node-list">{network.nodes.map((node) => <li key={node.nodeId}><Link to={`/nodes/${node.nodeId}`}>{node.displayName ?? node.nodeId}</Link><span className={`status status-${node.health}`}>{node.health}</span></li>)}</ul>
            </section>)}
          </div>
        </section>
        <Outlet />
      </main>
    </div>
  )
}
