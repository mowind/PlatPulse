import { Link, Outlet, useNavigate, useOutletContext } from 'react-router'
import { QueryClientProvider } from '@tanstack/react-query'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useAuth } from '../auth/AuthContext'
import {
  getSiteAccessGeneration,
  publicQueryClient,
  revalidateSiteAccessSettings,
  resetPublicCache,
  subscribeSiteAccessGeneration,
  usePublicNetworks,
  usePublicRealtime,
} from '../api/public'
import { ServerStatusNotice } from '../components/ServerStatusNotice'
import BackgroundDecoration from '../components/BackgroundDecoration'
import platpulseMark from '../../../assets/platpulse-mark.png'

/**
 * Home shell: reads only the Public Projection. Anonymous Guests may use
 * Home when the Owner enabled anonymous access; the Server enforces every
 * read, so this shell only decides whether to render Home or send Guests
 * to the login page. Authorization transitions arrive as Public `reset`
 * events: the shell re-checks the session and the Guest setting before any
 * cached projection can re-render (design §3.3, §6.3).
 */
export type HomeRealtimeContext = {
  resetting: boolean
  generation: number
  networks: ReturnType<typeof usePublicNetworks>
  realtime: ReturnType<typeof usePublicRealtime>
}

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
  const isOwner = status.state === 'authenticated' && status.session.role === 'owner'
  const [generation, setGeneration] = useState(getSiteAccessGeneration() ?? 0)
  const [resetting, setResetting] = useState(false)
  const authRef = useRef(status)
  authRef.current = status

  useEffect(() => subscribeSiteAccessGeneration(setGeneration), [])

  useEffect(() => {
    // A fresh Home shell must fetch an authoritative Public Projection after
    // returning from Admin. The first Public SSE stream starts from the REST
    // cursor captured for that projection, so retaining this cache across the
    // shell boundary could otherwise preserve a pre-mutation snapshot.
    return () => resetPublicCache(getSiteAccessGeneration() ?? 0)
  }, [])

  const networksQuery = usePublicNetworks(generation, !resetting)

  const handleReset = useCallback(() => {
    // A Public reset means an authorization transition: revoke, expiry,
    // role change, Guest disable, or a Public privacy reset. The current
    // projection is cleared BEFORE any recheck so prior data can never
    // flash while the new authorization resolves (design §3.3).
    setResetting(true)
    resetPublicCache(generation + 1)
    void Promise.all([recheckSession(), revalidateSiteAccessSettings()])
      .then(([confirmed, { mode, authorizationGeneration }]) => {
        if (!confirmed) return
        const current = authRef.current
        if (mode !== 'public' && current.state !== 'authenticated') {
          navigate('/login', { replace: true })
        } else {
          setResetting(false)
          setGeneration(authorizationGeneration)
        }
      })
      .catch(() => {})
  }, [generation, navigate, recheckSession])

  const realtime = usePublicRealtime(handleReset, !resetting, generation)
  return (
    <div className="app-shell home-shell">
      <BackgroundDecoration />
      <header className="app-header">
        <div className="home-shell-container app-header-inner">
          <Link to="/" className="app-brand" aria-label="PlatPulse"><img className="app-brand-logo" src={platpulseMark} alt="" /><span>PlatPulse</span></Link>
          {isOwner && <Link to="/admin" className="admin-icon-link" aria-label="Admin" title="Open Admin dashboard">
            <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="6.1" /><circle cx="12" cy="12" r="2.4" /><path d="M12 2.9v3M12 18.1v3M21.1 12h-3M5.9 12h-3M18.36 5.64l-2.05 2.05M5.64 5.64l2.05 2.05M18.36 18.36l-2.05-2.05M5.64 18.36l2.05-2.05" /></svg>
          </Link>}
        </div>
      </header>
      <main className="app-main">
        <div className="home-shell-container app-main-inner">
          <ServerStatusNotice />
          {networksQuery.data && networksQuery.isRefetchError && <p role="status" className="form-error">Partial: showing the last successful Home data while refresh is unavailable.</p>}
          {resetting ? <p role="status">Revalidating Home access…</p> : <Outlet context={{ resetting, generation, networks: networksQuery, realtime }} />}
        </div>
      </main>
    </div>
  )
}
