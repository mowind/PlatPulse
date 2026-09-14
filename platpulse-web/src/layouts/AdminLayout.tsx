import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from 'react'
import { Link, NavLink, Outlet, useOutletContext } from 'react-router'
import { QueryClientProvider } from '@tanstack/react-query'
import SignOutButton from '../components/SignOutButton'
import ThemeToggle from '../components/ThemeToggle'
import { ServerStatusNotice } from '../components/ServerStatusNotice'
import { RealtimeNotice } from '../components/RealtimeNotice'
import {
  adminQueryClient,
  resetAdminCache,
  subscribeAdminAccessReset,
  useAdminRealtime,
  type RealtimeState,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { Button } from '../components/ui/button'
import { useIsScrolled } from '../hooks/use-is-scrolled'
import { cn } from '../lib/utils'
import platpulseMark from '../../../assets/platpulse-mark.png'

/** Emerald navigation row: a quiet muteless link that lifts on hover and marks
 * the current page group through the accent surface. */
const NAV_LINK =
  'flex min-h-11 items-center gap-3 rounded-md px-3 py-2 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50'
const NAV_LINK_ACTIVE = 'bg-accent font-semibold text-foreground'

/**
 * Admin shell: Owner-only management layout with its own query cache and
 * SSE stream, structurally isolated from Public queries and events (design
 * §6.2). Every access-generation change closes the old stream, aborts
 * in-flight Admin requests, clears the sensitive Admin cache, and opens a
 * fresh stream under the new authorization (design §3.3).
 *
 * The chrome follows Emerald's shell: a sticky h-14 header that gains
 * backdrop-blur-xl only after the page scrolls, a max-w-[1280px] inner row,
 * and the Admin page groups as a vertical list of min-h-11 rows on the
 * left. Navigation follows webui.md §10.1: a persistent sidebar on desktop, a
 * collapsible drawer on tablet and mobile that moves focus inside, traps
 * Tab, closes on Escape, restores focus to the opener, and locks body
 * scroll while open. Each page-group link carries a leading decorative glyph
 * that is `aria-hidden`, inherits the label color, and never carries status
 * meaning (webui.md §10.3).
 */
export default function AdminLayout() {
  const { generation, recheckSession } = useAuth()
  const [streamKey, setStreamKey] = useState(0)
  const [resetting, setResetting] = useState(false)
  const [navOpen, setNavOpen] = useState(false)
  const previousGeneration = useRef(generation)
  const streamKeyRef = useRef(0)
  const accessGenerationRef = useRef(generation)
  const navToggleRef = useRef<HTMLButtonElement>(null)
  const navRef = useRef<HTMLElement>(null)
  const scrolled = useIsScrolled()

  // Authorization transition: re-validate the Admin cache (the transition
  // itself already cleared it synchronously in AuthContext) and restart the
  // stream under the new authorization. The initial mount never clears:
  // queries are generation-keyed, so older-generation values cannot render.
  useEffect(() => {
    if (previousGeneration.current === generation) return
    previousGeneration.current = generation
    accessGenerationRef.current = generation
    resetAdminCache(generation)
    streamKeyRef.current += 1
    setStreamKey(streamKeyRef.current)
  }, [generation])

  const handleAccessReset = useCallback(() => {
    // Close the current stream and clear sensitive query data before the
    // session probe starts; the next stream opens only under a fresh key.
    setResetting(true)
    accessGenerationRef.current = Math.max(accessGenerationRef.current, generation) + 1
    resetAdminCache(accessGenerationRef.current)
    void recheckSession().then((confirmed) => {
      if (confirmed) setResetting(false)
    })
  }, [generation, recheckSession])

  // Server-driven access resets (SSE `reset`, REST `auth_required`): the
  // reset handler performs close -> cancel/clear -> recheck synchronously.
  useEffect(() => subscribeAdminAccessReset(handleAccessReset), [handleAccessReset])

  const realtime = useAdminRealtime(
    generation,
    streamKey,
    handleAccessReset,
    !resetting && accessGenerationRef.current === generation,
  )

  const closeNav = useCallback(() => setNavOpen(false), [])

  // Body scroll lock while the mobile drawer is open.
  useEffect(() => {
    if (!navOpen) return
    const previousOverflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    return () => {
      document.body.style.overflow = previousOverflow
    }
  }, [navOpen])

  // Drawer opening moves focus inside; closing restores focus to the opener
  // (design §10.1). The initial mount never steals focus.
  const wasNavOpen = useRef(false)
  useEffect(() => {
    if (navOpen) {
      wasNavOpen.current = true
      navRef.current?.querySelector<HTMLElement>('a[href]')?.focus()
    } else if (wasNavOpen.current) {
      wasNavOpen.current = false
      navToggleRef.current?.focus()
    }
  }, [navOpen])

  // Escape closes the drawer; Tab is trapped inside while it is open.
  const onNavKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === 'Escape') {
      event.stopPropagation()
      closeNav()
      return
    }
    if (event.key !== 'Tab' || !navRef.current || !navOpen) return
    const focusables = Array.from(
      navRef.current.querySelectorAll<HTMLElement>('a[href], button:not([disabled])'),
    )
    if (focusables.length === 0) return
    const first = focusables[0]
    const last = focusables[focusables.length - 1]
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault()
      last.focus()
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault()
      first.focus()
    }
  }

  return (
    <div className="flex min-h-dvh flex-col" data-slot="admin-shell">
      <header
        data-slot="admin-header"
        className={cn(
          'sticky top-0 z-20 border-b border-transparent transition-all duration-200',
          scrolled ? 'backdrop-blur-xl' : 'bg-transparent',
        )}
      >
        <div className="mx-auto flex max-w-[1280px] flex-wrap items-center gap-x-2 gap-y-1 px-4 py-2 lg:h-14 lg:flex-nowrap lg:py-0">
          <Link
            to="/"
            data-slot="admin-brand"
            className="order-1 flex min-h-11 min-w-0 items-center gap-3"
            aria-label="PlatPulse"
          >
            <img className="size-8 shrink-0 rounded-full" src={platpulseMark} alt="" />
            <span className="min-w-0 truncate text-lg font-semibold">PlatPulse</span>
          </Link>
          <div
            role="group"
            aria-label="Admin connection status"
            className="order-3 flex w-full min-w-0 flex-wrap items-center gap-2 [&_p]:m-0 lg:order-2 lg:w-auto lg:flex-1"
          >
            <span className="text-xs font-medium tracking-wider text-muted-foreground">Realtime</span>
            {!resetting && <RealtimeNotice realtime={realtime} surface="admin" />}
            {resetting && (
              <span className="text-xs whitespace-nowrap text-muted-foreground">Checking access…</span>
            )}
            <ServerStatusNotice />
          </div>
          <div className="order-2 ml-auto flex shrink-0 items-center gap-2 lg:order-3 lg:ml-0">
            <Button
              ref={navToggleRef}
              type="button"
              variant="ghost"
              size="sm"
              data-slot="admin-nav-toggle"
              className="lg:hidden"
              aria-expanded={navOpen}
              aria-controls="admin-nav"
              onClick={() => setNavOpen((value) => !value)}
            >
              <span aria-hidden="true">☰</span> Menu
            </Button>
            <ThemeToggle />
            <div
              data-slot="admin-signout"
              className="flex items-center [&>button]:min-h-11 [&>button]:min-w-11 [&>button]:rounded-md [&>button]:px-3 [&>button]:text-sm [&>button]:font-medium [&>button]:text-muted-foreground [&>button]:transition-colors hover:[&>button]:bg-accent hover:[&>button]:text-foreground"
            >
              <SignOutButton />
            </div>
          </div>
        </div>
      </header>
      <div className="flex min-h-0 flex-1">
        <nav
          id="admin-nav"
          ref={navRef}
          data-slot="admin-nav"
          aria-label="Admin"
          className={cn(
            'fixed inset-y-0 left-0 z-40 flex w-[min(80vw,18rem)] flex-col gap-1 overflow-y-auto border-r border-border/60 bg-background/95 p-4 backdrop-blur-xl transition-transform duration-200 ease-out',
            navOpen ? 'visible translate-x-0' : 'invisible -translate-x-full',
            'lg:sticky lg:top-14 lg:z-auto lg:h-[calc(100dvh-3.5rem)] lg:w-[13.5rem] lg:flex-none lg:visible lg:translate-x-0 lg:border-r lg:bg-transparent lg:backdrop-blur-none',
          )}
          onKeyDown={onNavKeyDown}
        >
          <p className="px-3 pb-1 text-xs font-medium tracking-wider text-muted-foreground">
            Operations
          </p>
          <NavLink
            to="/admin"
            end
            onClick={closeNav}
            className={({ isActive }) => cn(NAV_LINK, isActive && NAV_LINK_ACTIVE)}
          >
            <span data-slot="admin-nav-icon" className="w-6 shrink-0 text-center" aria-hidden="true">
              ▦
            </span>
            Overview
          </NavLink>
          <NavLink
            to="/admin/agents"
            onClick={closeNav}
            className={({ isActive }) => cn(NAV_LINK, isActive && NAV_LINK_ACTIVE)}
          >
            <span data-slot="admin-nav-icon" className="w-6 shrink-0 text-center" aria-hidden="true">
              ◈
            </span>
            Agents
          </NavLink>
          <NavLink
            to="/admin/nodes"
            onClick={closeNav}
            className={({ isActive }) => cn(NAV_LINK, isActive && NAV_LINK_ACTIVE)}
          >
            <span data-slot="admin-nav-icon" className="w-6 shrink-0 text-center" aria-hidden="true">
              ◉
            </span>
            Nodes
          </NavLink>
          <NavLink
            to="/admin/networks"
            onClick={closeNav}
            className={({ isActive }) => cn(NAV_LINK, isActive && NAV_LINK_ACTIVE)}
          >
            <span data-slot="admin-nav-icon" className="w-6 shrink-0 text-center" aria-hidden="true">
              ⬡
            </span>
            Networks
          </NavLink>
          <NavLink
            to="/admin/settings"
            end
            onClick={closeNav}
            className={({ isActive }) => cn(NAV_LINK, isActive && NAV_LINK_ACTIVE)}
          >
            <span data-slot="admin-nav-icon" className="w-6 shrink-0 text-center" aria-hidden="true">
              ⚙
            </span>
            Settings
          </NavLink>
          <p className="px-3 pt-3 pb-1 text-xs font-medium tracking-wider text-muted-foreground">
            Access
          </p>
          <NavLink
            to="/admin/access/sessions"
            end
            onClick={closeNav}
            className={({ isActive }) => cn(NAV_LINK, isActive && NAV_LINK_ACTIVE)}
          >
            <span data-slot="admin-nav-icon" className="w-6 shrink-0 text-center" aria-hidden="true">
              ◫
            </span>
            Sessions
          </NavLink>
          <NavLink
            to="/admin/access/audit"
            end
            onClick={closeNav}
            className={({ isActive }) => cn(NAV_LINK, isActive && NAV_LINK_ACTIVE)}
          >
            <span data-slot="admin-nav-icon" className="w-6 shrink-0 text-center" aria-hidden="true">
              ☷
            </span>
            Audit
          </NavLink>
          {/* MVP Admin surface (issue #92): deferred groups (alerts,
              operations, data/maintenance, validators, people, transfer,
              enrollment/recovery/rotation) are not linked here. */}
        </nav>
        <div
          data-slot="admin-nav-scrim"
          className={cn(
            'fixed inset-0 z-30 bg-black/40 transition-opacity lg:hidden',
            navOpen ? 'visible opacity-100' : 'invisible opacity-0',
          )}
          onClick={closeNav}
          aria-hidden="true"
        />
        <QueryClientProvider client={adminQueryClient}>
          <main data-slot="admin-main" className="min-w-0 flex-1 px-4 py-6 lg:px-6">
            <div className="mx-auto w-full max-w-[1280px]">
              {resetting ? (
                <p role="status" className="text-sm text-muted-foreground">
                  Revalidating Admin access…
                </p>
              ) : (
                <Outlet context={{ realtime }} />
              )}
            </div>
          </main>
        </QueryClientProvider>
      </div>
    </div>
  )
}

/** Realtime state shared with Admin pages through the Outlet context. */
export function useAdminRealtimeContext(): { realtime: RealtimeState } {
  return useOutletContext<{ realtime: RealtimeState }>()
}
