import { useState, type FormEvent } from 'react'
import { Navigate, useLocation, useNavigate } from 'react-router'
import { useAuth } from '../auth/AuthContext'
import { AuthApiError } from '../api/auth'

/**
 * Login page (design §12.2/§12.4): username + password form with labels,
 * keyboard submission, a live error region, and no admin controls. After
 * login the user returns to the route that required authentication.
 */
export default function LoginPage() {
  const { status, login } = useAuth()
  const navigate = useNavigate()
  const location = useLocation()
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [submitting, setSubmitting] = useState(false)

  if (status.state === 'authenticated') {
    return <Navigate to="/" replace />
  }

  const from = (location.state as { from?: string } | null)?.from ?? '/'

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (submitting) return
    setError(null)
    setSubmitting(true)
    try {
      await login(username, password)
      navigate(from, { replace: true })
    } catch (caught) {
      const message =
        caught instanceof AuthApiError
          ? loginErrorMessage(caught.code, caught.message)
          : 'login failed; try again'
      setError(message)
      setSubmitting(false)
    }
  }

  return (
    <main className="login-page">
      <section className="login-card page" aria-labelledby="login-heading">
        <h1 id="login-heading">Sign in to PlatPulse</h1>
        <p className="login-hint">
          The Home dashboard is private by default. Sign in with your Owner
          account.
        </p>
        {error && (
          <p className="form-error" role="alert">
            {error}
          </p>
        )}
        <form onSubmit={handleSubmit} noValidate={false}>
          <div className="field">
            <label htmlFor="login-username">Username</label>
            <input
              id="login-username"
              name="username"
              type="text"
              autoComplete="username"
              autoFocus
              required
              value={username}
              onChange={(event) => setUsername(event.target.value)}
            />
          </div>
          <div className="field">
            <label htmlFor="login-password">Password</label>
            <input
              id="login-password"
              name="password"
              type="password"
              autoComplete="current-password"
              required
              value={password}
              onChange={(event) => setPassword(event.target.value)}
            />
          </div>
          <button type="submit" className="primary-action" disabled={submitting}>
            {submitting ? 'Signing in…' : 'Sign in'}
          </button>
        </form>
      </section>
    </main>
  )
}

function loginErrorMessage(code: string, fallback: string): string {
  switch (code) {
    case 'invalid_credentials':
      return 'Invalid username or password.'
    case 'login_rate_limited':
      return 'Too many failed attempts. Try again later.'
    case 'origin_validation_failed':
      return 'The request origin was rejected by the server.'
    case 'setup_required':
      return 'This server has not been set up yet. An Owner must initialize it first.'
    case 'user_disabled':
      return 'This account is disabled.'
    default:
      return fallback
  }
}
