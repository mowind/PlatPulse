import type { ReactNode } from 'react'
import { useEffect, useState } from 'react'
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
import AdminNodeTransfer from './pages/AdminNodeTransfer'
import AdminNetworksList, {
  AdminNetworkDetailPage,
} from './pages/AdminNetworks'
import AdminValidators, { AdminValidatorDetailPage } from './pages/AdminValidators'
import AdminPeople from './pages/AdminPeople'
import AdminSessions from './pages/AdminSessions'
import AdminAudit from './pages/AdminAudit'
import AdminAlertRulesList, {
  AdminAlertRuleDetail,
  AdminAlertRuleEdit,
} from './pages/AdminAlertRules'
import AdminIncidentsList, { AdminIncidentDetail } from './pages/AdminIncidents'
import AdminSilencesList from './pages/AdminSilences'
import AdminMaintenanceList from './pages/AdminMaintenance'
import AdminDeliveriesList, { AdminDeliveryDetail } from './pages/AdminDeliveries'
import AdminChannelsList, { AdminChannelDetail } from './pages/AdminChannels'
import AdminOperationsList from './pages/AdminOperations'
import AdminOperationDetail from './pages/AdminOperation'
import AdminData from './pages/AdminData'
import AdminRetentionList from './pages/AdminRetention'
import AdminRetentionEdit from './pages/AdminRetentionEdit'
import AdminBackupsList from './pages/AdminBackups'
import AdminRestore from './pages/AdminRestore'
import AdminBackupCreate from './pages/AdminBackupCreate'
import AdminBackupDetail from './pages/AdminBackup'
import AdminDoctor from './pages/AdminDoctor'
import { AuthProvider, useAuth } from './auth/AuthContext'
import { ensureGuestEnabledKnown, subscribeGuestEnabled } from './api/public'

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
  const [guestEnabled, setGuestEnabled] = useState<boolean | null>(null)

  useEffect(() => {
    void ensureGuestEnabledKnown().then(setGuestEnabled)
    return subscribeGuestEnabled(setGuestEnabled)
  }, [])

  if (status.state === 'loading') {
    return <CheckingAccess />
  }
  if (status.state === 'guest') {
    if (guestEnabled === null) {
      // Authorization is still resolving; never flash prior data.
      return <CheckingAccess />
    }
    if (guestEnabled) {
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
      { path: 'agents/enroll', element: <AdminAgentEnroll /> },
      { path: 'agents/:agentId', element: <AdminAgentDetail /> },
      { path: 'agents/:agentId/recover', element: <AdminAgentRecover /> },
      { path: 'agents/:agentId/rotate', element: <AdminAgentRotate /> },
      { path: 'nodes', element: <AdminNodesList /> },
      { path: 'nodes/:nodeId', element: <AdminNodeDetail /> },
      { path: 'nodes/:nodeId/visibility', element: <AdminNodeVisibility /> },
      { path: 'nodes/:nodeId/transfer', element: <AdminNodeTransfer /> },
      { path: 'networks', element: <AdminNetworksList /> },
      { path: 'networks/:networkKey', element: <AdminNetworkDetailPage /> },
      { path: 'validators', element: <AdminValidators /> },
      { path: 'validators/:validatorId', element: <AdminValidatorDetailPage /> },
      { path: 'access/people', element: <AdminPeople /> },
      { path: 'access/sessions', element: <AdminSessions /> },
      { path: 'access/audit', element: <AdminAudit /> },
      { path: 'alerts/rules', element: <AdminAlertRulesList /> },
      { path: 'alerts/rules/:ruleKey', element: <AdminAlertRuleDetail /> },
      { path: 'alerts/rules/:ruleKey/edit', element: <AdminAlertRuleEdit /> },
      { path: 'alerts/incidents', element: <AdminIncidentsList /> },
      { path: 'alerts/incidents/:incidentId', element: <AdminIncidentDetail /> },
      { path: 'alerts/silences', element: <AdminSilencesList /> },
      { path: 'alerts/maintenance', element: <AdminMaintenanceList /> },
      { path: 'alerts/deliveries', element: <AdminDeliveriesList /> },
      { path: 'alerts/deliveries/:deliveryId', element: <AdminDeliveryDetail /> },
      { path: 'alerts/channels', element: <AdminChannelsList /> },
      { path: 'alerts/channels/:channelId', element: <AdminChannelDetail /> },
      { path: 'operations', element: <AdminOperationsList /> },
      { path: 'operations/:operationId', element: <AdminOperationDetail /> },
      { path: 'data', element: <AdminData /> },
      { path: 'data/retention', element: <AdminRetentionList /> },
      { path: 'data/retention/edit', element: <AdminRetentionEdit /> },
      { path: 'data/backups', element: <AdminBackupsList /> },
      { path: 'data/backups/create', element: <AdminBackupCreate /> },
      { path: 'data/backups/:artifactId', element: <AdminBackupDetail /> },
      { path: 'data/restore', element: <AdminRestore /> },
      { path: 'data/doctor', element: <AdminDoctor /> },
      // Placeholder for later Phase 2 sections (e.g. Restore): links from
      // security mutations must never land on a blank page.
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
      <p>The Data and Maintenance surfaces arrive with a later slice.</p>
    </section>
  )
}
