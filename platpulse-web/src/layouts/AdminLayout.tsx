import { Link, NavLink, Outlet } from 'react-router'
import SignOutButton from '../components/SignOutButton'
import { ServerStatusNotice } from '../components/ServerStatusNotice'

/**
 * Admin shell: Owner-only management layout, independent from the Home
 * layout. Management pages arrive with Phase 1; nothing is pre-built before
 * those tickets.
 */
export default function AdminLayout() {
  return (
    <div className="app-shell admin-shell">
      <header className="app-header">
        <Link to="/" className="app-brand">
          PlatPulse
        </Link>
        <nav className="app-nav" aria-label="Primary">
          <NavLink to="/">Home</NavLink>
          <NavLink to="/admin" end>
            Admin
          </NavLink>
        </nav>
        <SignOutButton />
      </header>
      <main className="app-main">
        <ServerStatusNotice />
        <Outlet />
      </main>
    </div>
  )
}
