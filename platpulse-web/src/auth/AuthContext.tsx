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
import { fetchSession, login as apiLogin, logout as apiLogout } from '../api/auth'
import { resetAdminCache } from '../api/admin'
import { resetPublicCache } from '../api/public'

/** Auth state machine: loading → guest, or authenticated with a session. */
export type AuthStatus =
  | { state: 'loading' }
  | { state: 'guest' }
  | { state: 'authenticated'; session: SessionProjection; csrfToken: string }

interface AuthContextValue {
  status: AuthStatus
  /**
   * Access generation (design §3.3). Every authorization transition
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
   * gets the stable non-leaking Owner-required panel (design §12.1).
   */
  hadSession: boolean
  login: (username: string, password: string) => Promise<void>
  logout: () => Promise<void>
  /** Re-check the session after an access reset signal (SSE or 401). */
  recheckSession: () => Promise<boolean>
}

const AuthContext = createContext<AuthContextValue | null>(null)

export function AuthProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<AuthStatus>({ state: 'loading' })
  const [generation, setGeneration] = useState(1)
  const [accessLost, setAccessLost] = useState(false)
  const [hadSession, setHadSession] = useState(false)
  const statusRef = useRef(status)
  const sessionProbeRef = useRef<AbortController | null>(null)
  const sessionEpochRef = useRef(0)
  statusRef.current = status

  useEffect(() => {
    const controller = new AbortController()
    const epoch = sessionEpochRef.current + 1
    sessionEpochRef.current = epoch
    sessionProbeRef.current = controller
    fetchSession(controller.signal)
      .then((response) => {
        if (controller.signal.aborted || sessionEpochRef.current !== epoch) return
        // The initial check establishes the first generation; there is no
        // previous session to discard, so the generation is not bumped.
        setStatus(
          response
            ? {
                state: 'authenticated',
                session: response.session,
                csrfToken: response.csrfToken,
              }
            : { state: 'guest' },
        )
        if (response) setHadSession(true)
      })
      .catch(() => {
        // Aborted probes are expected during login/logout or unmount.
      })
    return () => {
      controller.abort()
      if (sessionProbeRef.current === controller) sessionProbeRef.current = null
      sessionEpochRef.current += 1
    }
  }, [])

  const login = useCallback(async (username: string, password: string) => {
    sessionProbeRef.current?.abort()
    sessionProbeRef.current = null
    sessionEpochRef.current += 1
    const response = await apiLogin(username, password)
    // Synchronously retire any cached Admin data from an earlier session
    // before the new session can render (design §3.3).
    setGeneration((value) => {
      resetAdminCache(value + 1)
      resetPublicCache(value + 1)
      return value + 1
    })
    setAccessLost(false)
    setHadSession(true)
    setStatus({
      state: 'authenticated',
      session: response.session,
      csrfToken: response.csrfToken,
    })
  }, [])

  const logout = useCallback(async () => {
    const current = statusRef.current
    const csrfToken = current.state === 'authenticated' ? current.csrfToken : ''
    await apiLogout(csrfToken)
    sessionProbeRef.current?.abort()
    sessionProbeRef.current = null
    sessionEpochRef.current += 1
    setGeneration((value) => {
      resetAdminCache(value + 1)
      resetPublicCache(value + 1)
      return value + 1
    })
    setStatus({ state: 'guest' })
  }, [])

  const recheckSession = useCallback(async () => {
    const current = statusRef.current
    if (current.state !== 'authenticated') return true
    sessionProbeRef.current?.abort()
    const controller = new AbortController()
    const epoch = sessionEpochRef.current + 1
    sessionEpochRef.current = epoch
    sessionProbeRef.current = controller
    let response: Awaited<ReturnType<typeof fetchSession>>
    try {
      response = await fetchSession(controller.signal)
    } catch {
      return false
    }
    if (sessionProbeRef.current === controller) sessionProbeRef.current = null
    if (controller.signal.aborted || sessionEpochRef.current !== epoch) return false
    if (!response) {
      // Expired or revoked while the surface was open: the old session's
      // data must never flash again.
      setAccessLost(true)
      setGeneration((value) => {
        resetAdminCache(value + 1)
        resetPublicCache(value + 1)
        return value + 1
      })
      setStatus({ state: 'guest' })
      return true
    }
    const identityChanged =
      response.session.role !== current.session.role ||
      response.session.userId !== current.session.userId
    // Any successful access re-check establishes a new authorization
    // generation. The reset signal already retired the old stream/cache;
    // bumping even for the same session prevents a restarted stream or an
    // in-flight response from being associated with the old generation.
    setAccessLost(false)
    setGeneration((value) => {
      resetAdminCache(value + 1)
      resetPublicCache(value + 1)
      return value + 1
    })
    if (identityChanged) {
      setStatus({
        state: 'authenticated',
        session: response.session,
        csrfToken: response.csrfToken,
      })
    }
    return true
  }, [])

  const value = useMemo(
    () => ({ status, generation, accessLost, hadSession, login, logout, recheckSession }),
    [status, generation, accessLost, hadSession, login, logout, recheckSession],
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
