import { Link, NavLink, Outlet, useNavigate, useOutletContext } from 'react-router'
import { QueryClientProvider } from '@tanstack/react-query'
import { useCallback, useEffect, useRef, useState } from 'react'
import SignOutButton from '../components/SignOutButton'
import { useAuth } from '../auth/AuthContext'
import {
  getSiteAccessGeneration,
  publicQueryClient,
  refreshSiteAccessSettings,
  resetPublicCache,
  subscribeSiteAccessGeneration,
  usePublicNetworks,
  usePublicRealtime,
} from '../api/public'
import { ServerStatusNotice } from '../components/ServerStatusNotice'
import { PeerInsight } from '../components/PeerInsight'
import { GeoInsight } from '../components/GeoInsight'
import { TransportError } from '../api/transport'

/**
 * Home shell: reads only the Public Projection. Anonymous Guests may use
 * Home when the Owner enabled anonymous access; the Server enforces every
 * read, so this shell only decides whether to render Home or send Guests
 * to the login page. Authorization transitions arrive as Public `reset`
 * events: the shell re-checks the session and the Guest setting before any
 * cached projection can re-render (design §3.3, §6.3).
 */
export type HomeRealtimeContext = { resetting: boolean; generation: number }

export function useHomeRealtimeContext(): HomeRealtimeContext {
  return useOutletContext<HomeRealtimeContext>()
}

export default function HomeLayout() {
  return (
    <QueryClientProvider client={publicQueryClient}>
      <HomeLayoutContent />
    </QueryClientProvider>
  )
}

function HomeLayoutContent() {
  const { status, recheckSession } = useAuth()
  const navigate = useNavigate()
  const isAuthenticated = status.state === 'authenticated'
  const isOwner = status.state === 'authenticated' && status.session.role === 'owner'
  const [generation, setGeneration] = useState(getSiteAccessGeneration() ?? 0)
  const [resetting, setResetting] = useState(false)
  const authRef = useRef(status)
  authRef.current = status

  useEffect(() => subscribeSiteAccessGeneration(setGeneration), [])

  const networksQuery = usePublicNetworks(generation)

  const handleReset = useCallback(() => {
    // A Public reset means an authorization transition: revoke, expiry,
    // role change, Guest disable, or a Public privacy reset. The current
    // projection is cleared BEFORE any recheck so prior data can never
    // flash while the new authorization resolves (design §3.3).
    setResetting(true)
    resetPublicCache(generation + 1)
    void recheckSession()
    void refreshSiteAccessSettings()
      .then(({ mode, authorizationGeneration }) => {
        const current = authRef.current
        if (mode !== 'public' && current.state !== 'authenticated') {
          navigate('/login', { replace: true })
        } else {
          setResetting(false)
          setGeneration(authorizationGeneration)
        }
      })
      .catch((caught: Error) => {
        setResetting(false)
        if (caught.name !== 'AbortError') setGeneration(generation + 1)
      })
  }, [generation, navigate, recheckSession])

  const realtime = usePublicRealtime(handleReset)
  const error = networksQuery.error instanceof Error ? networksQuery.error.message : null
  const forbidden = networksQuery.error instanceof TransportError &&
    (networksQuery.error.code === 'forbidden' || networksQuery.error.code === 'owner_required')

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
          <p className="realtime-notice" role="status" data-live={realtime.status === 'connected'}>
            {resetting
              ? 'Revalidating Home access…'
              : !realtime.online
              ? 'You are offline'
              : realtime.status === 'connected'
              ? 'Connected'
              : realtime.status === 'connecting'
                ? 'Starting'
                : 'Live updates paused'}
          </p>
          {forbidden && <p role="alert" className="form-error">Forbidden: Home data is unavailable for this account.</p>}
          {error && !forbidden && <p role="status" className="form-error">{error}</p>}
          {networksQuery.data && networksQuery.isRefetchError && <p role="status" className="form-error">Partial: showing the last successful Home data while refresh is unavailable.</p>}
          {networksQuery.isPending && <p role="status">Loading published Nodes…</p>}
          {networksQuery.isSuccess && networksQuery.data.length === 0 && <p role="status">No published Nodes yet.</p>}
          <div className="network-grid">
            {networksQuery.data?.map((network) => <section className="network-card" key={network.networkKey}>
              <h2><Link to={`/networks/${network.networkKey}`}>{network.displayName}</Link></h2>
              <p className="muted">{network.networkKey}</p>
              <PeerInsight insight={network.peers} compact />
              <GeoInsight insight={network.geo} />
              <ul className="node-list">{network.nodes.map((node) => <li key={node.nodeId}><Link to={`/nodes/${node.nodeId}`}>{node.displayName ?? node.nodeId}</Link><span className={`status status-${node.health}`}>{node.health}</span></li>)}</ul>
            </section>)}
          </div>
        </section>
        <Outlet context={{ resetting, generation }} />
      </main>
    </div>
  )
}
