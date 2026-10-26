import type { ReactNode } from 'react'
import { useEffect, useState } from 'react'
import { createBrowserRouter, Navigate, useLocation } from 'react-router'
import { RouterProvider } from 'react-router/dom'
import HomeLayout from './layouts/HomeLayout'
import { NodePage } from './pages/HomePages'
import { AuthProvider, useAuth } from './auth/AuthContext'
import { ThemeProvider } from './theme/ThemeProvider'
import { Toaster } from './components/ui/sonner'
import { BackTop } from './components/ui/back-top'
import { ensureSiteAccessModeKnown, subscribeSiteAccessMode, usePublicRewardWallet } from './api/public'
import { CardX } from './components/ui/card-x'
import { SURFACE_CARD_STATIC } from './lib/surface'
import HomeDashboard from './components/HomeDashboard'
import { useHomeRealtimeContext } from './layouts/HomeLayout'
import { RouteErrorFallback, withSuspense } from './components/RouteFallback'
import { lazyNamed, lazyRoute, preloadLazyRoutes } from './lazyRoute'

// Lazy-loaded routes (issue #275): Admin management pages, AdminLayout and LoginPage
// are dynamically imported so the public Home screen does not download or parse
// the ~27 admin route components upfront; main.tsx preloads them once the app
// has started (see lazyRoute.tsx).
const LoginPage = lazyRoute(() => import('./pages/LoginPage'))
const AdminLayout = lazyRoute(() => import('./layouts/AdminLayout'))
const AdminHome = lazyRoute(() => import('./pages/AdminHome'))
const AdminAgentsList = lazyRoute(() => import('./pages/AdminAgents'))
const AdminAgentDetail = lazyNamed(() => import('./pages/AdminAgents'), 'AdminAgentDetail')
const AdminAgentEnroll = lazyNamed(() => import('./pages/AdminAgents'), 'AdminAgentEnroll')
const AdminNodesList = lazyRoute(() => import('./pages/AdminNodes'))
const AdminNodeDetail = lazyNamed(() => import('./pages/AdminNodes'), 'AdminNodeDetail')
const AdminNodeInvestigation = lazyRoute(() => import('./pages/AdminNodeInvestigation'))
const AdminNetworksList = lazyRoute(() => import('./pages/AdminNetworks'))
const AdminNetworkDetailPage = lazyNamed(() => import('./pages/AdminNetworks'), 'AdminNetworkDetailPage')
const AdminValidatorsList = lazyRoute(() => import('./pages/AdminValidators'))
const AdminValidatorDetail = lazyNamed(() => import('./pages/AdminValidators'), 'AdminValidatorDetail')
const AdminSessions = lazyRoute(() => import('./pages/AdminSessions'))
const AdminAudit = lazyRoute(() => import('./pages/AdminAudit'))
const AdminSettings = lazyRoute(() => import('./pages/AdminSettings'))
const AdminIncidentsList = lazyRoute(() => import('./pages/AdminIncidents'))
const AdminIncidentDetailPage = lazyNamed(() => import('./pages/AdminIncidents'), 'AdminIncidentDetailPage')
const AdminRulesList = lazyRoute(() => import('./pages/AdminRules'))
const AdminRuleDetailPage = lazyNamed(() => import('./pages/AdminRules'), 'AdminRuleDetailPage')
const AdminSilencesList = lazyRoute(() => import('./pages/AdminSilences'))
const AdminMaintenanceList = lazyRoute(() => import('./pages/AdminMaintenance'))
const AdminOperationsList = lazyRoute(() => import('./pages/AdminOperations'))
const AdminOperationDetailPage = lazyNamed(() => import('./pages/AdminOperations'), 'AdminOperationDetailPage')
const AdminBackupsList = lazyRoute(() => import('./pages/AdminBackups'))
const AdminBackupDetailPage = lazyNamed(() => import('./pages/AdminBackups'), 'AdminBackupDetailPage')
const AdminDoctor = lazyRoute(() => import('./pages/AdminDoctor'))
const AdminRetention = lazyRoute(() => import('./pages/AdminRetention'))
const AdminNotifications = lazyRoute(() => import('./pages/AdminNotifications'))
const AdminNotificationEvents = lazyRoute(() => import('./pages/AdminNotificationEvents'))
const AdminNotificationEventDetailPage = lazyNamed(() => import('./pages/AdminNotificationEvents'), 'AdminNotificationEventDetailPage')
const AdminNotificationDeliveries = lazyRoute(() => import('./pages/AdminNotificationDeliveries'))
const AdminNotificationDeliveryDetailPage = lazyNamed(() => import('./pages/AdminNotificationDeliveries'), 'AdminNotificationDeliveryDetailPage')
const AdminNotificationChannels = lazyRoute(() => import('./pages/AdminNotificationChannels'))

if (import.meta.env.MODE === 'test') {
  await preloadLazyRoutes()
}

/**
 * Route gates (webui.md §3.2, §3.3): Home and Admin are private by default.
 * The first protected render is always an explicit access check that never
 * flashes data from a previous session; Guests are guided to the login page
 * (with the requested route preserved) and non-Owners are refused with a
 * stable, non-leaking Owner-required panel.
 */
function CheckingAccess() {
  return (
    <main className="mx-auto max-w-[1280px] p-4">
      <p role="status" className="text-sm text-muted-foreground">
        Checking access…
      </p>
    </main>
  )
}

/**
 * The Server could not be asked who the reader is (restarting, 502/503, a
 * database failure, a dropped connection). That is an outage, not a sign-out:
 * the gate holds the reader here, shows no prior data, and the auth provider
 * keeps probing until the Server answers (issue #267).
 */
function ServerUnavailable() {
  return (
    <main className="mx-auto max-w-[1280px] p-4">
      <p role="status" className="text-sm text-muted-foreground">
        The Server is temporarily unavailable. Retrying automatically.
      </p>
    </main>
  )
}

function RequireSession({ children }: { children: ReactNode }) {
  const { status, accessLost } = useAuth()
  const location = useLocation()
  // Anonymous Home (Guest) is allowed only when the Owner explicitly
  // enabled it (design §10.2); the Server still enforces every read. The
  // setting is cached and re-checked on Public resets by the Home layout.
  const [siteAccessMode, setSiteAccessMode] = useState<'public' | 'private' | null>(null)

  useEffect(() => {
    void ensureSiteAccessModeKnown().then(setSiteAccessMode)
    return subscribeSiteAccessMode(setSiteAccessMode)
  }, [])

  if (status.state === 'loading') {
    return <CheckingAccess />
  }
  if (status.state === 'unavailable') {
    return <ServerUnavailable />
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
  if (status.state === 'unavailable') {
    return <ServerUnavailable />
  }
  if (status.state === 'guest') {
    if (hadSession) {
      // This app instance previously held a Session (signed out, or the
      // session was lost while Admin was open): go back through the
      // non-leaking login path, explaining revocations (webui.md §3.3).
      return (
        <Navigate
          to="/login"
          state={{ from: location.pathname, sessionExpired: accessLost }}
          replace
        />
      )
    }
    // Never-authenticated Guests never access Admin, even when anonymous
    // Home is enabled (design §10.2: Guest uses only the Public
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
    <section className="mx-auto max-w-[1280px] p-4">
      <CardX bordered={false} className={SURFACE_CARD_STATIC}>
        <h1 className="text-lg font-semibold">Owner access required</h1>
        <p className="mt-2 text-sm">The Admin dashboard is restricted to Owners.</p>
        <p className="mt-2 text-sm text-muted-foreground">
          This session cannot view Admin data. Sign in with an Owner account to continue.
        </p>
      </CardX>
    </section>
  )
}

const router = createBrowserRouter([
  {
    path: '/login',
    errorElement: <RouteErrorFallback />,
    element: withSuspense(<LoginPage />),
  },
  {
    path: '/',
    errorElement: <RouteErrorFallback />,
    element: (
      <RequireSession>
        <HomeLayout />
      </RequireSession>
    ),
    children: [
      { index: true, element: <HomeIndex /> },
      { path: 'networks/:networkKey', element: <Navigate to="/" replace /> },
      { path: 'nodes/:nodeId', element: <NodePage /> },
    ],
  },
  {
    path: '/admin',
    errorElement: <RouteErrorFallback />,
    element: (
      <RequireOwner>
        {withSuspense(<AdminLayout />)}
      </RequireOwner>
    ),
    children: [
      { index: true, element: <AdminHome /> },
      { path: 'agents', element: <AdminAgentsList /> },
      // Static segment ranks above `agents/:agentId`, so this is the
      // enrollment workflow and never an Agent Detail lookup.
      { path: 'agents/enroll', element: <AdminAgentEnroll /> },
      { path: 'agents/:agentId', element: <AdminAgentDetail /> },
      { path: 'nodes', element: <AdminNodesList /> },
      { path: 'nodes/:nodeId', element: <AdminNodeDetail /> },
      // Investigation surface (issue #220): one adjustable UTC window over every
      // family of evidence the Server holds for a Node, reached from the Node
      // page or from an Incident occurrence. The window lives in the URL, so a
      // shared or reloaded link reads the same evidence.
      { path: 'nodes/:nodeId/investigation', element: <AdminNodeInvestigation /> },
      { path: 'networks', element: <AdminNetworksList /> },
      { path: 'networks/:networkKey', element: <AdminNetworkDetailPage /> },
      // Validator surface (issue #218): the automatic Network-scoped identity
      // the Server resolved from observed P2P evidence, with the staking
      // status and last-good evidence behind every verdict. The static
      // `validators` segment ranks above `validators/:validatorId`.
      { path: 'validators', element: <AdminValidatorsList /> },
      { path: 'validators/:validatorId', element: <AdminValidatorDetail /> },
      // Operations surface (issue #208): the durable task ledger and one
      // task's recorded outcome, so queued, running, and terminal work is
      // linkable and survives a reload. The static `operations` segment
      // ranks above `operations/:operationId`, so the ledger is never a
      // detail lookup.
      { path: 'operations', element: <AdminOperationsList /> },
      { path: 'operations/:operationId', element: <AdminOperationDetailPage /> },
      { path: 'doctor', element: <AdminDoctor /> },
      // Retention surface (issue #210): the policy table, one bounded edit at a
      // time, and the expiring preview a run is bound to. The run executes the
      // preview's frozen plan, so this page composes one before offering it.
      { path: 'retention', element: <AdminRetention /> },
      // Backup artifact surface (issue #209): read the recorded artifacts and
      // request verification. Backups are created offline and restoring is a
      // separate operation, so neither is offered here.
      { path: 'backups', element: <AdminBackupsList /> },
      { path: 'backups/:artifactId', element: <AdminBackupDetailPage /> },
      { path: 'settings', element: <AdminSettings /> },
      { path: 'alerts/incidents', element: <AdminIncidentsList /> },
      { path: 'alerts/incidents/:incidentId', element: <AdminIncidentDetailPage /> },
      { path: 'alerts/rules', element: <AdminRulesList /> },
      { path: 'alerts/rules/:ruleKey', element: <AdminRuleDetailPage /> },
      { path: 'alerts/silences', element: <AdminSilencesList /> },
      { path: 'alerts/maintenance', element: <AdminMaintenanceList /> },
      // Notification surface (issue #206): one Admin nav entry, with the
      // event/delivery history and the controlled channel test as their own
      // routes so each is linkable and survives a reload.
      { path: 'notifications', element: <AdminNotifications /> },
      { path: 'notifications/events', element: <AdminNotificationEvents /> },
      { path: 'notifications/events/:eventId', element: <AdminNotificationEventDetailPage /> },
      { path: 'notifications/deliveries', element: <AdminNotificationDeliveries /> },
      { path: 'notifications/deliveries/:deliveryId', element: <AdminNotificationDeliveryDetailPage /> },
      { path: 'notifications/channels', element: <AdminNotificationChannels /> },
      { path: 'access/sessions', element: <AdminSessions /> },
      { path: 'access/audit', element: <AdminAudit /> },
      // Safe Admin fallback (issue #92): removed legacy/deferred routes
      // and unknown paths resolve here, never to a legacy page; links from
      // retained security mutations must not land on a blank outlet.
      { path: '*', element: <AdminSectionFallback /> },
    ],
  },
  {
    path: '*',
    element: <Navigate to="/" replace />,
  },
])

export default function App() {
  return (
    <ThemeProvider>
      <AuthProvider>
        <RouterProvider router={router} />
        <Toaster richColors closeButton position="top-center" />
        <BackTop />
      </AuthProvider>
    </ThemeProvider>
  )
}

function HomeIndex() {
  const { networks, realtime, resetting, generation } = useHomeRealtimeContext()
  // Its own query (#246): the Network projection's loading or failure never
  // decides what the Balance tile reads.
  const rewardWallet = usePublicRewardWallet(generation, !resetting)
  // The projection on screen may be the one Home already held while the
  // authoritative refetch is still in flight, so the arrival's landing is
  // provisional until that reading settles (webui.md §15.25).
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
      refreshing={networks.isFetching}
      rewardWallet={rewardWallet.data
        ? { status: 'ready', wallet: rewardWallet.data }
        : rewardWallet.isError ? { status: 'error' } : { status: 'loading' }}
    />
  )
}

function AdminSectionFallback() {
  return (
    <section className="mx-auto max-w-[1280px] p-4">
      <CardX bordered={false} className={SURFACE_CARD_STATIC}>
        <h1 className="text-lg font-semibold">Section not found</h1>
        <p className="mt-2 text-sm text-muted-foreground">
          This Admin section is not part of the current MVP surface.
        </p>
      </CardX>
    </section>
  )
}

export { HomeLayout, RequireOwner, OwnerRequiredPanel }
export { useHomeRealtimeContext }
