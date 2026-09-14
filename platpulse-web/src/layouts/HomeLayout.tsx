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
import AppFooter from '../components/AppFooter'
import ThemeToggle from '../components/ThemeToggle'
import { EmeraldActionIcon } from '../components/EmeraldActionIcon'
import { Alert, AlertDescription } from '../components/ui/alert'
import { buttonVariants } from '../components/ui/button'
import { useIsScrolled } from '../hooks/use-is-scrolled'
import { cn } from '../lib/utils'
import platpulseMark from '../../../assets/platpulse-mark.png'

/**
 * Home shell: reads only the Public Projection. Anonymous Guests may use
 * Home when the Owner enabled anonymous access; the Server enforces every
 * read, so this shell only decides whether to render Home or send Guests
 * to the login page. Authorization transitions arrive as Public `reset`
 * events: the shell re-checks the session and the Guest setting before any
 * cached projection can re-render (design §3.3, §6.3).
 *
 * The chrome follows Emerald's shell: a sticky h-14 header that gains
 * backdrop-blur-xl only after the page scrolls, and a max-w-[1280px] content
 * column shared by every page.
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
  const scrolled = useIsScrolled()
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
    <div className="flex min-h-screen flex-col">
      <BackgroundDecoration />
      <header
        data-slot="app-header"
        className={cn(
          'sticky top-0 z-10 border-b border-transparent transition-all duration-200',
          scrolled ? 'backdrop-blur-xl' : 'bg-transparent',
        )}
      >
        <div className="mx-auto flex h-14 max-w-[1280px] items-center justify-between px-4">
          <Link to="/" data-slot="app-brand" className="flex min-h-11 items-center gap-3" aria-label="PlatPulse">
            <img className="size-8 shrink-0 rounded-full" src={platpulseMark} alt="" />
            <h3 className="m-0 text-lg font-semibold">PlatPulse</h3>
          </Link>
          <div className="flex items-center gap-2">
            <ThemeToggle />
            {isOwner && (
              <Link
                to="/admin"
                aria-label="Admin"
                title="Open Admin dashboard"
                className={buttonVariants({ variant: 'ghost', size: 'icon-sm' })}
              >
                <EmeraldActionIcon name="setting" />
              </Link>
            )}
          </div>
        </div>
      </header>
      <main className="flex-1">
        <div className="mx-auto max-w-[1280px]">
          <ServerStatusNotice />
          {networksQuery.data && networksQuery.isRefetchError && (
            <div className="px-4">
              <Alert>
                <AlertDescription role="status">
                  Partial: showing the last successful Home data while refresh is unavailable.
                </AlertDescription>
              </Alert>
            </div>
          )}
          {resetting ? (
            <p role="status" className="p-4 text-sm text-muted-foreground">
              Revalidating Home access…
            </p>
          ) : (
            <Outlet context={{ resetting, generation, networks: networksQuery, realtime }} />
          )}
        </div>
      </main>
      <AppFooter />
    </div>
  )
}
