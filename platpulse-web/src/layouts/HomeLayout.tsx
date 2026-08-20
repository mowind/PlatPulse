import { Link, NavLink, Outlet, useNavigate, useOutletContext } from 'react-router'
import { useCallback, useEffect, useRef, useState } from 'react'
import SignOutButton from '../components/SignOutButton'
import { useAuth } from '../auth/AuthContext'
import {
  fetchNetworks,
  refreshSiteAccessMode,
  usePublicRealtime,
} from '../api/public'
import type { PublicNetwork } from '../api/generated'
import { ServerStatusNotice } from '../components/ServerStatusNotice'
import { PeerInsight } from '../components/PeerInsight'
import { GeoInsight } from '../components/GeoInsight'

/**
 * Home shell: reads only the Public Projection. Anonymous Guests may use
 * Home when the Owner enabled anonymous access; the Server enforces every
 * read, so this shell only decides whether to render Home or send Guests
 * to the login page. Authorization transitions arrive as Public `reset`
 * events: the shell re-checks the session and the Guest setting before any
 * cached projection can re-render (design §3.3, §6.3).
 */
export type HomeRealtimeContext = { reloadKey: number; resetting: boolean }

export function useHomeRealtimeContext(): HomeRealtimeContext {
  return useOutletContext<HomeRealtimeContext>()
}

export default function HomeLayout() {
  const { status, recheckSession } = useAuth()
  const navigate = useNavigate()
  const isAuthenticated = status.state === 'authenticated'
  const isOwner = status.state === 'authenticated' && status.session.role === 'owner'
  const [networks, setNetworks] = useState<PublicNetwork[]>([])
  const [error, setError] = useState<string | null>(null)
  const [reloadKey, setReloadKey] = useState(0)
  const [resetting, setResetting] = useState(false)
  const requestController = useRef<AbortController | null>(null)
  const accessController = useRef<AbortController | null>(null)
  const authRef = useRef(status)
  authRef.current = status

  useEffect(() => {
    requestController.current?.abort()
    const controller = new AbortController()
    requestController.current = controller
    fetchNetworks(controller.signal)
      .then((data) => {
        if (!controller.signal.aborted) setNetworks(data)
      })
      .catch((caught: Error) => {
        if (!controller.signal.aborted && caught.name !== 'AbortError') setError(caught.message)
      })
    return () => {
      controller.abort()
      if (requestController.current === controller) requestController.current = null
    }
  }, [reloadKey])

  const handleReset = useCallback(() => {
    // A Public reset means an authorization transition: revoke, expiry,
    // role change, Guest disable, or a Public privacy reset. The current
    // projection is cleared BEFORE any recheck so prior data can never
    // flash while the new authorization resolves (design §3.3).
    setNetworks([])
    setError(null)
    setResetting(true)
    requestController.current?.abort()
    accessController.current?.abort()
    const controller = new AbortController()
    accessController.current = controller
    void recheckSession()
    void refreshSiteAccessMode(controller.signal)
      .then((mode) => {
        if (controller.signal.aborted) return
        const current = authRef.current
        if (mode !== 'public' && current.state !== 'authenticated') {
          navigate('/login', { replace: true })
        } else {
          setResetting(false)
          setReloadKey((value) => value + 1)
        }
      })
      .catch((caught: Error) => {
        if (caught.name !== 'AbortError') {
          setResetting(false)
          setError(caught.message)
        }
      })
  }, [navigate, recheckSession])

  const realtimeStatus = usePublicRealtime(() => {
    setError(null)
    setReloadKey((value) => value + 1)
  }, handleReset)

  return (
    <div className="app-shell">
      <header className="app-header">
        <Link to="/" className="app-brand">PlatPulse</Link>
        <nav className="app-nav" aria-label="Primary">
          <NavLink to="/" end>Home</NavLink>
          {isOwner && <NavLink to="/admin">Admin</NavLink>}
        </nav>
        {isAuthenticated && <SignOutButton />}
      </header>
      <main className="app-main">
        <section className="page home-page">
          <ServerStatusNotice />
          <h1>Home</h1>
          <h2>Network overview</h2>
          <p>Published Nodes grouped by Network. Private and retired Nodes are not listed.</p>
          <p className="realtime-notice" role="status" data-live={realtimeStatus === 'connected'}>
            {realtimeStatus === 'connected'
              ? 'Connected'
              : realtimeStatus === 'connecting'
                ? 'Starting'
                : 'Live updates paused'}
          </p>
          {error && <p role="status" className="form-error">{error}</p>}
          {networks.length === 0 && !error && <p role="status">No published Nodes yet.</p>}
          <div className="network-grid">
            {networks.map((network) => <section className="network-card" key={network.networkKey}>
              <h2><Link to={`/networks/${network.networkKey}`}>{network.displayName}</Link></h2>
              <p className="muted">{network.networkKey}</p>
              <PeerInsight insight={network.peers} compact />
              <GeoInsight insight={network.geo} />
              <ul className="node-list">{network.nodes.map((node) => <li key={node.nodeId}><Link to={`/nodes/${node.nodeId}`}>{node.displayName ?? node.nodeId}</Link><span className={`status status-${node.health}`}>{node.health}</span></li>)}</ul>
            </section>)}
          </div>
        </section>
        <Outlet context={{ reloadKey, resetting }} />
      </main>
    </div>
  )
}
