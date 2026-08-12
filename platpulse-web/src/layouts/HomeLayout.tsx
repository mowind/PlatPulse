import { Link, NavLink, Outlet } from 'react-router'
import SignOutButton from '../components/SignOutButton'

/**
 * Home shell: the private, read-only monitoring layout. The `main` region
 * will host Network/Node views from Phase 1; nothing is pre-built before
 * those tickets.
 */
export default function HomeLayout() {
  return (
    <div className="app-shell">
      <header className="app-header">
        <Link to="/" className="app-brand">
          PlatPulse
        </Link>
        <nav className="app-nav" aria-label="Primary">
          <NavLink to="/" end>
            Home
          </NavLink>
          <NavLink to="/admin">Admin</NavLink>
        </nav>
        <SignOutButton />
      </header>
      <main className="app-main">
        <Outlet />
      </main>
    </div>
  )
}
