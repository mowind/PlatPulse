import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from 'react'
import { Link, NavLink, Outlet, useOutletContext } from 'react-router'
import { QueryClientProvider } from '@tanstack/react-query'
import SignOutButton from '../components/SignOutButton'
import { ServerStatusNotice } from '../components/ServerStatusNotice'
import {
  adminQueryClient,
  resetAdminCache,
  subscribeAdminAccessReset,
  useAdminRealtime,
  type RealtimeState,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'

/**
 * Admin shell: Owner-only management layout with its own query cache and
 * SSE stream, structurally isolated from Public queries and events (design
 * §6.2). Every access-generation change closes the old stream, aborts
 * in-flight Admin requests, clears the sensitive Admin cache, and opens a
 * fresh stream under the new authorization (design §3.3).
 *
 * Navigation follows design §10.1: a persistent sidebar on desktop, a
 * collapsible drawer on tablet and mobile that moves focus inside, traps
 * Tab, closes on Escape, restores focus to the opener, and locks body
 * scroll while open.
 */
export default function AdminLayout() {
  const { generation, recheckSession } = useAuth()
  const [streamKey, setStreamKey] = useState(0)
  const [navOpen, setNavOpen] = useState(false)
  const previousGeneration = useRef(generation)
  const navToggleRef = useRef<HTMLButtonElement>(null)
  const navRef = useRef<HTMLElement>(null)

  // Authorization transition: re-validate the Admin cache (the transition
  // itself already cleared it synchronously in AuthContext) and restart the
  // stream under the new authorization. The initial mount never clears:
  // queries are generation-keyed, so older-generation values cannot render.
  useEffect(() => {
    if (previousGeneration.current === generation) return
    previousGeneration.current = generation
    resetAdminCache(generation)
    setStreamKey((value) => value + 1)
  }, [generation])

  // Server-driven access resets (SSE `reset`, REST `auth_required`): re-check
  // the session, which bumps the generation when the session changed and
  // restarts the stream even when it did not.
  useEffect(() => subscribeAdminAccessReset(() => void recheckSession()), [recheckSession])

  const realtime = useAdminRealtime(streamKey, () => {
    setStreamKey((value) => value + 1)
    void recheckSession()
  })

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
    <div className="app-shell admin-shell">
      <header className="app-header admin-header">
        <Link to="/" className="app-brand">
          PlatPulse
        </Link>
        <nav className="app-nav" aria-label="Global">
          <NavLink to="/">Home</NavLink>
        </nav>
        <button
          ref={navToggleRef}
          type="button"
          className="nav-toggle"
          aria-expanded={navOpen}
          aria-controls="admin-nav"
          onClick={() => setNavOpen((value) => !value)}
        >
          <span aria-hidden="true">☰</span> Menu
        </button>
        <SignOutButton />
      </header>
      <div className="admin-body">
        <nav
          id="admin-nav"
          ref={navRef}
          aria-label="Admin"
          className={navOpen ? 'admin-nav admin-nav-open' : 'admin-nav'}
          onKeyDown={onNavKeyDown}
        >
          <p className="admin-nav-label">Operations</p>
          <NavLink to="/admin" end onClick={closeNav}>
            Overview
          </NavLink>
          <NavLink to="/admin/agents" end onClick={closeNav}>
            Agents
          </NavLink>
          <NavLink to="/admin/nodes" end onClick={closeNav}>
            Nodes
          </NavLink>
          <NavLink to="/admin/networks" end onClick={closeNav}>
            Networks
          </NavLink>
          <p className="admin-nav-label">Alerts</p>
          <NavLink to="/admin/alerts/rules" end onClick={closeNav}>
            Alert Rules
          </NavLink>
          <NavLink to="/admin/alerts/incidents" end onClick={closeNav}>
            Incidents
          </NavLink>
          <NavLink to="/admin/alerts/silences" end onClick={closeNav}>
            Silences
          </NavLink>
          <NavLink to="/admin/alerts/maintenance" end onClick={closeNav}>
            Maintenance
          </NavLink>
          <p className="admin-nav-label">Access</p>
          <NavLink to="/admin/access/people" end onClick={closeNav}>
            People
          </NavLink>
          <NavLink to="/admin/access/sessions" end onClick={closeNav}>
            Sessions
          </NavLink>
          <NavLink to="/admin/access/audit" end onClick={closeNav}>
            Audit
          </NavLink>
          {/* Later Phase 2 groups arrive here: Alerts and Operations,
              Data and Maintenance. */}
        </nav>
        <div
          className={navOpen ? 'admin-nav-scrim admin-nav-scrim-open' : 'admin-nav-scrim'}
          onClick={closeNav}
          aria-hidden="true"
        />
        <QueryClientProvider client={adminQueryClient}>
          <main className="app-main">
            <ServerStatusNotice />
            <Outlet context={{ realtime }} />
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
