import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react'
import type { SessionProjection } from '../api/generated'
import { fetchSession, login as apiLogin, logout as apiLogout } from '../api/auth'

/** Auth state machine: loading → guest, or authenticated with a session. */
export type AuthStatus =
  | { state: 'loading' }
  | { state: 'guest' }
  | { state: 'authenticated'; session: SessionProjection; csrfToken: string }

interface AuthContextValue {
  status: AuthStatus
  login: (username: string, password: string) => Promise<void>
  logout: () => Promise<void>
}

const AuthContext = createContext<AuthContextValue | null>(null)

export function AuthProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<AuthStatus>({ state: 'loading' })

  useEffect(() => {
    let cancelled = false
    fetchSession().then((response) => {
      if (cancelled) return
      setStatus(
        response
          ? {
              state: 'authenticated',
              session: response.session,
              csrfToken: response.csrfToken,
            }
          : { state: 'guest' },
      )
    })
    return () => {
      cancelled = true
    }
  }, [])

  const login = useCallback(async (username: string, password: string) => {
    const response = await apiLogin(username, password)
    setStatus({
      state: 'authenticated',
      session: response.session,
      csrfToken: response.csrfToken,
    })
  }, [])

  const logout = useCallback(async () => {
    await apiLogout()
    setStatus({ state: 'guest' })
  }, [])

  const value = useMemo(
    () => ({ status, login, logout }),
    [status, login, logout],
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
