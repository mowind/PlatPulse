import type { ReactNode } from 'react'
import { createBrowserRouter, Navigate, useLocation } from 'react-router'
import { RouterProvider } from 'react-router/dom'
import AdminLayout from './layouts/AdminLayout'
import HomeLayout from './layouts/HomeLayout'
import LoginPage from './pages/LoginPage'
import { NetworkPage, NodePage } from './pages/HomePages'
import AdminHome from './pages/AdminHome'
import AdminAgentsList, {
  AdminAgentDetail,
  AdminAgentEnroll,
  AdminAgentRecover,
  AdminAgentRotate,
} from './pages/AdminAgents'
import AdminNodesList, {
  AdminNodeDetail,
  AdminNodeVisibility,
} from './pages/AdminNodes'
import AdminNetworksList, {
  AdminNetworkDetailPage,
} from './pages/AdminNetworks'
import { AuthProvider, useAuth } from './auth/AuthContext'

/**
 * Route gates (design §3.2, §3.3): Home and Admin are private by default.
 * The first protected render is always an explicit access check that never
 * flashes data from a previous session; Guests are guided to the login page
 * (with the requested route preserved) and non-Owners are refused with a
 * stable, non-leaking Owner-required panel.
 */
function CheckingAccess() {
  return (
    <main className="app-main">
      <p role="status">Checking access…</p>
    </main>
  )
}

function RequireSession({ children }: { children: ReactNode }) {
  const { status, accessLost } = useAuth()
  const location = useLocation()

  if (status.state === 'loading') {
    return <CheckingAccess />
  }
  if (status.state === 'guest') {
    return (
      <Navigate
        to="/login"
        state={{ from: location.pathname, sessionExpired: accessLost }}
        replace
      />
    )
  }
  return children
}

function RequireOwner({ children }: { children: ReactNode }) {
  const { status } = useAuth()

  if (status.state === 'loading') {
    return <CheckingAccess />
  }
  if (status.state === 'guest') {
    return <RequireSession>{children}</RequireSession>
  }
  if (status.session.role !== 'owner') {
    return (
      <section className="page">
        <h1>Owner access required</h1>
        <p>The Admin dashboard is restricted to Owners.</p>
        <p className="muted">
          This session cannot view Admin data. Sign out and sign in with an
          Owner account to continue.
        </p>
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
    children: [
      { index: true, element: <HomeIndex /> },
      { path: 'networks/:networkKey', element: <NetworkPage /> },
      { path: 'nodes/:nodeId', element: <NodePage /> },
    ],
  },
  {
    path: '/admin',
    element: (
      <RequireOwner>
        <AdminLayout />
      </RequireOwner>
    ),
    children: [
      { index: true, element: <AdminHome /> },
      { path: 'agents', element: <AdminAgentsList /> },
      { path: 'agents/enroll', element: <AdminAgentEnroll /> },
      { path: 'agents/:agentId', element: <AdminAgentDetail /> },
      { path: 'agents/:agentId/recover', element: <AdminAgentRecover /> },
      { path: 'agents/:agentId/rotate', element: <AdminAgentRotate /> },
      { path: 'nodes', element: <AdminNodesList /> },
      { path: 'nodes/:nodeId', element: <AdminNodeDetail /> },
      { path: 'nodes/:nodeId/visibility', element: <AdminNodeVisibility /> },
      { path: 'networks', element: <AdminNetworksList /> },
      { path: 'networks/:networkKey', element: <AdminNetworkDetailPage /> },
      // Placeholder for later Phase 2 sections (e.g. PAGE-ACCESS-AUDIT):
      // links from security mutations must never land on a blank page.
      { path: '*', element: <AdminComingSoon /> },
    ],
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
  return null
}

function AdminComingSoon() {
  return (
    <section className="page">
      <h1>This section arrives in a later phase</h1>
      <p>The full Audit review surface is delivered with the People, roles, and Sessions slice.</p>
    </section>
  )
}
