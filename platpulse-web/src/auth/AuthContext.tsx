import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import type { SessionProjection } from '../api/generated'
import { probeSession, login as apiLogin, logout as apiLogout } from '../api/auth'
import { resetAdminCache } from '../api/admin'
import { resetPublicCache } from '../api/public'
import { syncHomeReturnReader } from '../homeReturn'

/** Auth state machine: loading → guest, unavailable, or authenticated. */
export type AuthStatus =
  | { state: 'loading' }
  | { state: 'guest' }
  /**
   * The Server could not be asked yet (restarting, 502/503, a database
   * failure, a dropped connection), so access is unknown rather than refused.
   * No prior data is ever shown under it, and the probe retries until the
   * Server answers (issue #267).
   */
  | { state: 'unavailable' }
  | { state: 'authenticated'; session: SessionProjection; csrfToken: string }

/** What an access re-check established (issue #267). */
export type SessionRecheck =
  /** Access is unchanged: the Session is still live, or there was none. */
  | 'confirmed'
  /** The Server refused the cookie (`401` + `auth_required`): not signed in. */
  | 'signed_out'
  /** The Server could not be asked, so nothing about access was established. */
  | 'unavailable'

interface AuthContextValue {
  status: AuthStatus
  /**
   * Access generation (webui.md §3.3). Every authorization transition
   * (login, logout, expiry/revocation, role change) bumps it so Admin
   * surfaces can close streams, abort requests, clear sensitive cache,
   * and discard older-generation responses.
   */
  generation: number
  /**
   * True after a live session was lost (expired or revoked) and until the
   * next successful login; lets the login page explain the transition
   * without leaking any prior session data.
   */
  accessLost: boolean
  /**
   * True once this app instance ever held a session (initial check or
   * login). Lets protected routes distinguish a signed-out user, who is
   * guided back to the login page, from a never-authenticated Guest, who
   * gets the stable non-leaking Owner-required panel (design §10.2).
   */
  hadSession: boolean
  /**
   * True while the last session probe went unanswered: the reader keeps the
   * access they already had, and the shell states that the Server is
   * temporarily unavailable instead of pretending to be up to date
   * (webui.md §3.3, issue #267).
   */
  outage: boolean
  login: (username: string, password: string) => Promise<void>
  logout: () => Promise<void>
  /**
   * Re-check the session after an access reset signal (SSE or 401). Reports
   * what the re-check established instead of a bare "confirmed": an
   * unreachable Server keeps the reader's access and retries, rather than
   * being read as a lost session (issue #267).
   */
  recheckSession: () => Promise<SessionRecheck>
}

const AuthContext = createContext<AuthContextValue | null>(null)

/**
 * Delay before the next session probe after the Server could not be asked:
 * quick enough to recover the moment a restart finishes, slow enough not to
 * hammer a Server that is still starting (issue #267).
 */
const PROBE_RETRY_DELAYS_MS = [1_000, 2_000, 4_000, 8_000, 15_000, 30_000]

/** Which question a probe answers, so an unanswered one retries the same way. */
type ProbeGuard = 'initial' | 'recheck'
type ProbeOutcome = SessionRecheck | 'superseded'

export function AuthProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<AuthStatus>({ state: 'loading' })
  const [generation, setGeneration] = useState(1)
  const [accessLost, setAccessLost] = useState(false)
  const [hadSession, setHadSession] = useState(false)
  const [outage, setOutage] = useState(false)
  const statusRef = useRef(status)
  const sessionProbeRef = useRef<AbortController | null>(null)
  const sessionEpochRef = useRef(0)
  const retryTimerRef = useRef<number | null>(null)
  const retryAttemptRef = useRef(0)
  /** False once the boundary is unmounted: nothing may restart a probe then. */
  const mountedRef = useRef(true)
  /** The sign-in in flight, so a stale rejection cannot recover the probe. */
  const loginAttemptRef = useRef(0)
  statusRef.current = status

  const clearRetryTimer = useCallback(() => {
    if (retryTimerRef.current !== null) {
      window.clearTimeout(retryTimerRef.current)
      retryTimerRef.current = null
    }
    retryAttemptRef.current = 0
  }, [])

  /**
   * Retire every cached value of the previous authorization generation
   * (webui.md §3.3): streams close, requests abort, and older responses can
   * no longer be associated with the new authorization.
   */
  const retireAuthorization = useCallback(() => {
    setGeneration((value) => {
      resetAdminCache(value + 1)
      resetPublicCache(value + 1)
      return value + 1
    })
  }, [])

  /**
   * Retire the probe in flight together with the retry it would have
   * scheduled: whatever it answers belongs to a document state that no longer
   * exists (a login, a logout, or an unmount).
   */
  const invalidateProbe = useCallback(() => {
    sessionProbeRef.current?.abort()
    sessionProbeRef.current = null
    sessionEpochRef.current += 1
    clearRetryTimer()
  }, [clearRetryTimer])

  const probeRef = useRef<(guard: ProbeGuard) => Promise<ProbeOutcome>>(
    async () => 'superseded',
  )

  const scheduleRetry = useCallback((guard: ProbeGuard) => {
    // One pending retry at a time; the delay grows with each unanswered probe
    // and stays at the last step while the Server is still away.
    if (retryTimerRef.current !== null) return
    const attempt = retryAttemptRef.current
    retryAttemptRef.current = attempt + 1
    const delay = PROBE_RETRY_DELAYS_MS[Math.min(attempt, PROBE_RETRY_DELAYS_MS.length - 1)]
    retryTimerRef.current = window.setTimeout(() => {
      retryTimerRef.current = null
      void probeRef.current(guard)
    }, delay)
  }, [])

  /**
   * One session probe, and everything it settles. Only an explicit `401`
   * retires the session; a Server that cannot answer leaves the reader where
   * they are and is asked again (issue #267).
   */
  const probe = useCallback(
    async (guard: ProbeGuard): Promise<ProbeOutcome> => {
      // A retry that outlived its reason never answers for a state that has
      // already moved on (a login, a logout, or a newer probe).
      const before = statusRef.current
      if (guard === 'recheck' && before.state !== 'authenticated') return 'superseded'
      if (guard === 'initial' && before.state !== 'loading' && before.state !== 'unavailable') {
        return 'superseded'
      }
      sessionProbeRef.current?.abort()
      const controller = new AbortController()
      const epoch = sessionEpochRef.current + 1
      sessionEpochRef.current = epoch
      sessionProbeRef.current = controller
      let answer
      try {
        answer = await probeSession(controller.signal)
      } catch {
        // Aborted by login/logout/unmount or by a newer probe: not an answer
        // about access.
        return 'superseded'
      }
      if (sessionProbeRef.current === controller) sessionProbeRef.current = null
      if (controller.signal.aborted || sessionEpochRef.current !== epoch) return 'superseded'

      if (answer.kind === 'unavailable') {
        // The Server is away, which says nothing about this Session: hold the
        // reader where they are (never a Guest) and ask again.
        if (guard === 'initial') setStatus({ state: 'unavailable' })
        setOutage(true)
        scheduleRetry(guard)
        return 'unavailable'
      }

      clearRetryTimer()
      setOutage(false)

      if (answer.kind === 'unauthenticated') {
        if (guard === 'initial') {
          setStatus({ state: 'guest' })
          return 'signed_out'
        }
        // Expired or revoked while a surface was open: the old session's data
        // must never flash again.
        setAccessLost(true)
        retireAuthorization()
        setStatus({ state: 'guest' })
        return 'signed_out'
      }

      const response = answer.response
      if (guard === 'initial') {
        // The initial check establishes the first generation; there is no
        // previous session to discard, so the generation is not bumped.
        setHadSession(true)
        setStatus({
          state: 'authenticated',
          session: response.session,
          csrfToken: response.csrfToken,
        })
        return 'confirmed'
      }

      const current = statusRef.current
      if (current.state !== 'authenticated') return 'superseded'
      const identityChanged =
        response.session.role !== current.session.role ||
        response.session.userId !== current.session.userId
      // Any successful access re-check establishes a new authorization
      // generation. The reset signal already retired the old stream/cache;
      // bumping even for the same session prevents a restarted stream or an
      // in-flight response from being associated with the old generation.
      setAccessLost(false)
      retireAuthorization()
      if (identityChanged) {
        setStatus({
          state: 'authenticated',
          session: response.session,
          csrfToken: response.csrfToken,
        })
      }
      return 'confirmed'
    },
    [clearRetryTimer, retireAuthorization, scheduleRetry],
  )
  probeRef.current = probe

  useEffect(() => {
    mountedRef.current = true
    void probeRef.current('initial')
    return () => {
      mountedRef.current = false
      invalidateProbe()
    }
  }, [invalidateProbe])

  // A departure belongs to the reader who wrote it (webui.md §15.25): the token
  // that carries it is bound here, at the boundary that outlives every shell
  // drawing a reading, because signing out and signing in as somebody else
  // happens while no such shell is mounted — an old history entry would
  // otherwise be read back in front of the reader who replaced them. The
  // reader is the session's identity, not its generation: a successful access
  // re-check republishes the same reader's data and owes them their own return.
  const reader =
    status.state === 'authenticated'
      ? `${status.session.role}:${status.session.userId}`
      : status.state === 'guest'
        ? 'guest'
        : null
  useEffect(() => {
    if (reader !== null) syncHomeReturnReader(reader)
  }, [reader])

  const login = useCallback(
    async (username: string, password: string) => {
      const attempt = loginAttemptRef.current + 1
      loginAttemptRef.current = attempt
      invalidateProbe()
      const response = await apiLogin(username, password).catch((error: unknown) => {
        // A rejected login signed nobody in, so it must not end the access
        // question either: an unanswered first probe is asked again, or a
        // Server that never answered would leave the reader parked on
        // `unavailable` with no way to recover (issue #267). Only the sign-in
        // that is still the current one may do it, and only while the boundary
        // is mounted — an unmounted provider has already canceled its probes,
        // so a late rejection must not start a retry chain nobody owns.
        const before = statusRef.current
        const stillOurs = mountedRef.current && loginAttemptRef.current === attempt
        if (stillOurs && (before.state === 'loading' || before.state === 'unavailable')) {
          void probeRef.current('initial')
        }
        throw error
      })
      // Synchronously retire any cached Admin data from an earlier session
      // before the new session can render (webui.md §3.3).
      retireAuthorization()
      setAccessLost(false)
      setOutage(false)
      setHadSession(true)
      setStatus({
        state: 'authenticated',
        session: response.session,
        csrfToken: response.csrfToken,
      })
    },
    [invalidateProbe, retireAuthorization],
  )

  const logout = useCallback(async () => {
    const current = statusRef.current
    const csrfToken = current.state === 'authenticated' ? current.csrfToken : ''
    // A sign-in still in flight belongs to a reader who is leaving now.
    loginAttemptRef.current += 1
    await apiLogout(csrfToken)
    invalidateProbe()
    setOutage(false)
    retireAuthorization()
    setStatus({ state: 'guest' })
  }, [invalidateProbe, retireAuthorization])

  const recheckSession = useCallback(async (): Promise<SessionRecheck> => {
    // There is no live session to protect: a Guest's access is whatever the
    // Server says it is, and an initial probe that has not been answered yet
    // is already retrying on its own.
    if (statusRef.current.state !== 'authenticated') return 'confirmed'
    const outcome = await probeRef.current('recheck')
    // A superseded probe was replaced by a newer one that owns the state now.
    return outcome === 'superseded' ? 'unavailable' : outcome
  }, [])

  const value = useMemo(
    () => ({
      status,
      generation,
      accessLost,
      hadSession,
      outage,
      login,
      logout,
      recheckSession,
    }),
    [status, generation, accessLost, hadSession, outage, login, logout, recheckSession],
  )

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>
}

export function useAuth(): AuthContextValue {
  const value = useContext(AuthContext)
  if (!value) {
    throw new Error('useAuth must be used inside <AuthProvider>')
  }
  return value
}
