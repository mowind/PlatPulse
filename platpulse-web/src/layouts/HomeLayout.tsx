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
      <header className="app-header">
        <Link to="/" className="app-brand" aria-label="PlatPulse">PlatPulse</Link>
        {isOwner && <Link to="/admin" className="admin-icon-link" aria-label="Admin" title="Open Admin dashboard"><span aria-hidden="true">⚙</span></Link>}
      </header>
      <main className="app-main">
        <ServerStatusNotice />
        {networksQuery.data && networksQuery.isRefetchError && <p role="status" className="form-error">Partial: showing the last successful Home data while refresh is unavailable.</p>}
        {resetting ? <p role="status">Revalidating Home access…</p> : <Outlet context={{ resetting, generation, networks: networksQuery, realtime }} />}
      </main>
    </div>
  )
}
