import type { ReactNode } from 'react'
import { useEffect, useState } from 'react'
import { createBrowserRouter, Navigate, useLocation } from 'react-router'
import { RouterProvider } from 'react-router/dom'
import AdminLayout from './layouts/AdminLayout'
import HomeLayout from './layouts/HomeLayout'
import LoginPage from './pages/LoginPage'
import { NetworkPage, NodePage } from './pages/HomePages'
import AdminHome from './pages/AdminHome'
import AdminAgentsList, { AdminAgentDetail } from './pages/AdminAgents'
import AdminNodesList, { AdminNodeDetail } from './pages/AdminNodes'
import AdminNetworksList, {
  AdminNetworkDetailPage,
} from './pages/AdminNetworks'
import AdminSessions from './pages/AdminSessions'
import AdminAudit from './pages/AdminAudit'
import AdminHistoryWindow from './pages/AdminHistoryWindow'
import AdminSiteAccess from './pages/AdminSiteAccess'
import { AuthProvider, useAuth } from './auth/AuthContext'
import { ensureSiteAccessModeKnown, subscribeSiteAccessMode } from './api/public'
import HomeDashboard from './components/HomeDashboard'
import { useHomeRealtimeContext } from './layouts/HomeLayout'

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
  // Anonymous Home (Guest) is allowed only when the Owner explicitly
  // enabled it (design §12.1); the Server still enforces every read. The
  // setting is cached and re-checked on Public resets by the Home layout.
  const [siteAccessMode, setSiteAccessMode] = useState<'public' | 'private' | null>(null)

  useEffect(() => {
    void ensureSiteAccessModeKnown().then(setSiteAccessMode)
    return subscribeSiteAccessMode(setSiteAccessMode)
  }, [])

  if (status.state === 'loading') {
    return <CheckingAccess />
  }
  if (status.state === 'guest') {
    if (siteAccessMode === null) {
      // Authorization is still resolving; never flash prior data.
      return <CheckingAccess />
    }
    if (siteAccessMode === 'public') {
      return children
    }
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
  const { status, accessLost, hadSession } = useAuth()
  const location = useLocation()

  if (status.state === 'loading') {
    return <CheckingAccess />
  }
  if (status.state === 'guest') {
    if (hadSession) {
      // This app instance previously held a Session (signed out, or the
      // session was lost while Admin was open): go back through the
      // non-leaking login path, explaining revocations (design §3.3).
      return (
        <Navigate
          to="/login"
          state={{ from: location.pathname, sessionExpired: accessLost }}
          replace
        />
      )
    }
    // Never-authenticated Guests never access Admin, even when anonymous
    // Home is enabled (design §12.1: Guest uses only the Public
    // Projection; webui.md §3.2). The outcome is the same stable,
    // non-leaking panel a Viewer sees.
    return <OwnerRequiredPanel />
  }
  if (status.session.role !== 'owner') {
    return <OwnerRequiredPanel />
  }
  return children
}

function OwnerRequiredPanel() {
  return (
    <section className="page">
      <h1>Owner access required</h1>
      <p>The Admin dashboard is restricted to Owners.</p>
      <p className="muted">
        This session cannot view Admin data. Sign in with an Owner account to
        continue.
      </p>
    </section>
  )
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
      { path: 'agents/:agentId', element: <AdminAgentDetail /> },
      { path: 'nodes', element: <AdminNodesList /> },
      { path: 'nodes/:nodeId', element: <AdminNodeDetail /> },
      { path: 'networks', element: <AdminNetworksList /> },
      { path: 'networks/:networkKey', element: <AdminNetworkDetailPage /> },
      { path: 'site-access', element: <AdminSiteAccess /> },
      { path: 'history-window', element: <AdminHistoryWindow /> },
      { path: 'access/sessions', element: <AdminSessions /> },
      { path: 'access/audit', element: <AdminAudit /> },
      // Safe Admin fallback (issue #92): removed legacy/deferred routes
      // and unknown paths resolve here, never to a legacy page; links from
      // retained security mutations must not land on a blank outlet.
      { path: '*', element: <AdminSectionFallback /> },
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
  const { networks, realtime, resetting } = useHomeRealtimeContext()
  return (
    <HomeDashboard
      networks={networks.data ?? []}
      realtimeStatus={realtime.status}
      online={realtime.online}
      resetting={resetting}
      error={networks.error
        ? networks.data
          ? 'Home refresh failed; showing the last successful Home data.'
          : 'Unable to load Active Nodes'
        : null}
      hasLastGood={networks.data !== undefined}
      loading={networks.isPending}
    />
  )
}

function AdminSectionFallback() {
  return (
    <section className="page">
      <h1>Section not found</h1>
      <p>This Admin section is not part of the current MVP surface.</p>
    </section>
  )
}
