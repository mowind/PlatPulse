import { Link, NavLink, Outlet } from 'react-router'
import SignOutButton from '../components/SignOutButton'
import { useAuth } from '../auth/AuthContext'

/**
 * Home shell: the private, read-only monitoring layout. Viewers never see
 * the Admin link; the Server still enforces the role boundary regardless
 * of what the UI shows (design §13.1). The `main` region will host
 * Network/Node views from Phase 1; nothing is pre-built before those
 * tickets.
 */
export default function HomeLayout() {
  const { status } = useAuth()
  // The shell only renders for authenticated sessions, so the role check
  // is a plain narrowing of the AuthStatus union.
  const isOwner = status.state === 'authenticated' && status.session.role === 'owner'

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
          {isOwner && <NavLink to="/admin">Admin</NavLink>}
        </nav>
        <SignOutButton />
      </header>
      <main className="app-main">
        <Outlet />
      </main>
    </div>
  )
}
