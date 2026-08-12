import type { ReactNode } from 'react'
import { createBrowserRouter, Navigate, useLocation } from 'react-router'
import { RouterProvider } from 'react-router/dom'
import AdminLayout from './layouts/AdminLayout'
import HomeLayout from './layouts/HomeLayout'
import LoginPage from './pages/LoginPage'
import { AuthProvider, useAuth } from './auth/AuthContext'

/**
 * Route gate: Home and Admin are private by default (design §12.2/§13.1).
 * Unauthenticated visitors are guided to the login page and returned to
 * the route they originally requested; non-Owners are refused with an
 * explicit Owner-required panel.
 */
function RequireSession({ children }: { children: ReactNode }) {
  const { status } = useAuth()
  const location = useLocation()

  if (status.state === 'loading') {
    return (
      <main className="app-main">
        <p role="status">Checking session…</p>
      </main>
    )
  }
  if (status.state === 'guest') {
    return (
      <Navigate to="/login" state={{ from: location.pathname }} replace />
    )
  }
  return children
}

function RequireOwner({ children }: { children: ReactNode }) {
  const { status } = useAuth()
  if (status.state !== 'authenticated') {
    return <RequireSession>{children}</RequireSession>
  }
  if (status.session.role !== 'owner') {
    return (
      <section className="page">
        <h1>Owner access required</h1>
        <p>The Admin dashboard is restricted to Owners.</p>
      </section>
    )
  }
  return children
}

const router = createBrowserRouter([
  {
    path: '/login',
    element: <LoginPage />,
  },
  {
    path: '/',
    element: (
      <RequireSession>
        <HomeLayout />
      </RequireSession>
    ),
    children: [{ index: true, element: <HomeIndex /> }],
  },
  {
    path: '/admin',
    element: (
      <RequireOwner>
        <AdminLayout />
      </RequireOwner>
    ),
    children: [{ index: true, element: <AdminIndex /> }],
  },
])

export default function App() {
  return (
    <AuthProvider>
      <RouterProvider router={router} />
    </AuthProvider>
  )
}

function HomeIndex() {
  return (
    <section className="page">
      <h1>Home</h1>
      <p>
        The PlatPulse Home shell renders and navigates on every supported
        viewport.
      </p>
    </section>
  )
}

function AdminIndex() {
  return (
    <section className="page">
      <h1>Admin</h1>
      <p>
        The PlatPulse Admin shell renders and navigates on every supported
        viewport.
      </p>
    </section>
  )
}
