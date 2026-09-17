import { useState, type FormEvent } from 'react'
import { Link, Navigate, useLocation, useNavigate } from 'react-router'
import { useAuth } from '../auth/AuthContext'
import { AuthApiError } from '../api/auth'
import BackgroundDecoration from '../components/BackgroundDecoration'
import ThemeToggle from '../components/ThemeToggle'
import { Alert, AlertDescription } from '../components/ui/alert'
import { Button } from '../components/ui/button'
import { Input } from '../components/ui/input'
import { useIsScrolled } from '../hooks/use-is-scrolled'
import { cn } from '../lib/utils'
import platpulseMark from '../../../assets/platpulse-mark.png'

/**
 * Login page (design §12.2/§12.4): username + password form with labels,
 * keyboard submission, a live error region, and no admin controls. After
 * login the user returns to the route that required authentication.
 *
 * The chrome follows Emerald's Login foundation: a centred, cardless form
 * column below the Emerald brand header in the shared max-w-[1280px]
 * content column. The header exposes only the Home link and theme control.
 */
export default function LoginPage() {
  const { status, login } = useAuth()
  const navigate = useNavigate()
  const location = useLocation()
  const scrolled = useIsScrolled()
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [submitting, setSubmitting] = useState(false)

  if (status.state === 'authenticated') {
    return <Navigate to="/" replace />
  }

  const from = (location.state as { from?: string; sessionExpired?: boolean } | null)?.from ?? '/'
  const sessionExpired =
    (location.state as { sessionExpired?: boolean } | null)?.sessionExpired ?? false

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
    <div className="relative flex min-h-dvh flex-col" data-slot="login-page">
      <BackgroundDecoration />
      <header
        data-slot="app-header"
        className={cn(
          'sticky top-0 z-10 border-b border-transparent transition-all duration-200',
          scrolled ? 'backdrop-blur-xl' : 'bg-transparent',
        )}
      >
        <div className="mx-auto flex h-14 max-w-[1280px] items-center justify-between px-4">
          <Link to="/" data-slot="app-brand" className="flex min-h-11 items-center gap-3" aria-label="PlatPulse">
            <img className="size-8 shrink-0 rounded-full" src={platpulseMark} alt="" />
            <span className="text-lg font-semibold">PlatPulse</span>
          </Link>
          <ThemeToggle />
        </div>
      </header>
      <main className="mx-auto flex w-full max-w-[1280px] flex-1 items-center justify-center px-4 py-12">
        <section className="w-full max-w-sm" aria-labelledby="login-heading">
          <h1 id="login-heading" className="text-lg font-semibold">
            Sign in to PlatPulse
          </h1>
          <p data-slot="login-hint" className="mt-2 text-sm text-muted-foreground">
            The Home dashboard is private by default. Sign in with your Owner
            or Viewer account.
          </p>
          {sessionExpired && (
            <Alert role="status" className="mt-4">
              <AlertDescription>
                Your session expired or was revoked. Sign in again to continue.
              </AlertDescription>
            </Alert>
          )}
          {error && (
            <Alert variant="destructive" data-slot="form-error" className="mt-4">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          <form onSubmit={handleSubmit} noValidate={false} className="mt-6 flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <label
                htmlFor="login-username"
                className="text-xs font-medium tracking-wider text-muted-foreground"
              >
                Username
              </label>
              <Input
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
            <div className="flex flex-col gap-1.5">
              <label
                htmlFor="login-password"
                className="text-xs font-medium tracking-wider text-muted-foreground"
              >
                Password
              </label>
              <Input
                id="login-password"
                name="password"
                type="password"
                autoComplete="current-password"
                required
                value={password}
                onChange={(event) => setPassword(event.target.value)}
              />
            </div>
            <Button type="submit" className="w-full" disabled={submitting}>
              {submitting ? 'Signing in…' : 'Sign in'}
            </Button>
          </form>
        </section>
      </main>
    </div>
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
