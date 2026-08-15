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
  recheckSession: () => Promise<void>
}

const AuthContext = createContext<AuthContextValue | null>(null)

export function AuthProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<AuthStatus>({ state: 'loading' })
  const [generation, setGeneration] = useState(1)
  const [accessLost, setAccessLost] = useState(false)
  const [hadSession, setHadSession] = useState(false)
  const statusRef = useRef(status)
  statusRef.current = status

  useEffect(() => {
    let cancelled = false
    fetchSession().then((response) => {
      if (cancelled) return
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
    return () => {
      cancelled = true
    }
  }, [])

  const login = useCallback(async (username: string, password: string) => {
    const response = await apiLogin(username, password)
    // Synchronously retire any cached Admin data from an earlier session
    // before the new session can render (design §3.3).
    setGeneration((value) => {
      resetAdminCache(value + 1)
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
    await apiLogout()
    setGeneration((value) => {
      resetAdminCache(value + 1)
      return value + 1
    })
    setStatus({ state: 'guest' })
  }, [])

  const recheckSession = useCallback(async () => {
    const current = statusRef.current
    if (current.state !== 'authenticated') return
    const response = await fetchSession()
    if (!response) {
      // Expired or revoked while the surface was open: the old session's
      // data must never flash again.
      setAccessLost(true)
      setGeneration((value) => {
        resetAdminCache(value + 1)
        return value + 1
      })
      setStatus({ state: 'guest' })
      return
    }
    if (
      response.session.role !== current.session.role ||
      response.session.userId !== current.session.userId
    ) {
      // Role/user changed: new access generation, old data discarded.
      setAccessLost(false)
      setGeneration((value) => {
        resetAdminCache(value + 1)
        return value + 1
      })
      setStatus({
        state: 'authenticated',
        session: response.session,
        csrfToken: response.csrfToken,
      })
    }
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
