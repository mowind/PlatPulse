import { useState, type FormEvent } from 'react'
import {
  AdminApiError,
  changePersonRole,
  createPersonEntry,
  resetPersonPasswordEntry,
  setPersonDisabled,
  updateAccessSettings,
  useAdminAccess,
  useAdminPeople,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import type { Person } from '../api/generated'

/**
 * PAGE-ACCESS-PEOPLE (design §12.1, issue #47): People and role management
 * plus the anonymous Home (Guest) toggle. Rows show role, disabled state,
 * and active Session count — never passwords or credential material. All
 * mutations are authoritative (no optimistic state); the Server protects
 * the final valid Owner and revokes the affected user's Sessions on role,
 * password, or disabled changes.
 */
export default function AdminPeople() {
  const { status, generation } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const people = useAdminPeople(generation)
  const access = useAdminAccess(generation)
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  return (
    <section className="page">
      <h1>People</h1>
      <p className="muted">
        Owner-only access review. Passwords and credentials are never shown;
        role, password, and disabled changes immediately revoke the affected
        user's Sessions.
      </p>
      {message && (
        <p className="form-success" role="status">
          {message}
        </p>
      )}
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      <GuestAccessPanel query={access} csrfToken={csrfToken} />
      <PeoplePanel query={people} csrfToken={csrfToken} onMessage={setMessage} onError={setError} />
      <CreatePersonForm csrfToken={csrfToken} onMessage={setMessage} onError={setError} />
    </section>
  )
}

function GuestAccessPanel({
  query,
  csrfToken,
}: {
  query: ReturnType<typeof useAdminAccess>
  csrfToken: string
}) {
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const enabled = query.data?.guestEnabled

  async function toggle() {
    if (enabled === undefined || busy) return
    setBusy(true)
    setMessage(null)
    setError(null)
    try {
      const result = await updateAccessSettings(!enabled, csrfToken)
      setMessage(
        result.guestEnabled
          ? 'Anonymous Home access is now enabled. Visitors can view published Nodes without signing in.'
          : 'Anonymous Home access is now disabled. Visitors must sign in.',
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to update access')
    } finally {
      setBusy(false)
    }
  }

  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Anonymous Home access</h2>
        {enabled === true && (
          <span className="account-state account-state-enabled">
            <span aria-hidden="true">✓</span> Enabled
          </span>
        )}
        {enabled === false && <StatusBadge status="Disabled" tone="neutral" />}
      </div>
      <p className="muted">
        When enabled, Guests can read the Home Public Projection without a
        Session. Only explicitly published Nodes appear. Disabling closes all
        open Guest streams immediately.
      </p>
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading access settings…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" /> Unable to load access settings.
        </p>
      )}
      {query.data && (
        <button
          type="button"
          className="primary-action"
          disabled={busy}
          onClick={() => void toggle()}
        >
          {busy ? 'Updating…' : enabled ? 'Disable anonymous Home' : 'Enable anonymous Home'}
        </button>
      )}
      {message && (
        <p className="form-success" role="status">
          {message}
        </p>
      )}
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
    </article>
  )
}

function PeoplePanel({
  query,
  csrfToken,
  onMessage,
  onError,
}: {
  query: ReturnType<typeof useAdminPeople>
  csrfToken: string
  onMessage: (message: string | null) => void
  onError: (error: string | null) => void
}) {
  const { status } = useAuth()
  const me = status.state === 'authenticated' ? status.session.userId : null
  const [roleBusy, setRoleBusy] = useState<string | null>(null)
  const [statusBusy, setStatusBusy] = useState<string | null>(null)
  const [resetOpen, setResetOpen] = useState<string | null>(null)

  async function changeRole(person: Person, role: string) {
    if (role === person.role || roleBusy) return
    onMessage(null)
    onError(null)
    setRoleBusy(person.userId)
    try {
      const result = await changePersonRole(person.userId, role, csrfToken)
      onMessage(
        `${result.username} is now ${result.role === 'owner' ? 'an Owner' : 'a Viewer'}. Their Sessions were revoked; they must sign in again.`,
      )
    } catch (caught) {
      onError(
        caught instanceof AdminApiError && caught.code === 'final_owner_protected'
          ? 'The final valid Owner cannot be demoted.'
          : caught instanceof Error
            ? caught.message
            : 'Unable to change the role',
      )
    } finally {
      setRoleBusy(null)
    }
  }

  async function toggleDisabled(person: Person) {
    onMessage(null)
    onError(null)
    setStatusBusy(person.userId)
    try {
      const result = await setPersonDisabled(person.userId, !person.disabled, csrfToken)
      onMessage(
        result.disabled
          ? `${result.username} is disabled. Their Sessions were revoked.`
          : `${result.username} is enabled again.`,
      )
    } catch (caught) {
      onError(
        caught instanceof AdminApiError && caught.code === 'final_owner_protected'
          ? 'The final valid Owner cannot be disabled.'
          : caught instanceof Error
            ? caught.message
            : 'Unable to update the user',
      )
    } finally {
      setStatusBusy(null)
    }
  }

  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>People and roles</h2>
        {query.data && <span className="panel-count">{query.data.users.length}</span>}
      </div>
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading People…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load People'}
          <button type="button" className="text-action" onClick={() => void query.refetch()}>
            Try again
          </button>
        </p>
      )}
      {query.data && query.data.users.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No People yet.
        </p>
      )}
      {query.data && query.data.users.length > 0 && (
        <div className="table-wrap">
          <table className="people-table">
            <caption className="sr-only">People, roles, and account state</caption>
            <thead>
              <tr>
                <th scope="col">Username</th>
                <th scope="col">Role</th>
                <th scope="col">Status</th>
                <th scope="col">Sessions</th>
                <th scope="col">Created</th>
                <th scope="col">Actions</th>
              </tr>
            </thead>
            <tbody>
              {query.data.users.map((person) => (
                <PersonRows
                  key={person.userId}
                  person={person}
                  isMe={person.userId === me}
                  roleBusy={roleBusy === person.userId}
                  statusBusy={statusBusy === person.userId}
                  resetOpen={resetOpen === person.userId}
                  csrfToken={csrfToken}
                  onOpenReset={() =>
                    setResetOpen((current) => (current === person.userId ? null : person.userId))
                  }
                  onChangeRole={(role) => void changeRole(person, role)}
                  onToggleDisabled={() => void toggleDisabled(person)}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </article>
  )
}

function PersonRows({
  person,
  isMe,
  roleBusy,
  statusBusy,
  resetOpen,
  csrfToken,
  onOpenReset,
  onChangeRole,
  onToggleDisabled,
}: {
  person: Person
  isMe: boolean
  roleBusy: boolean
  statusBusy: boolean
  resetOpen: boolean
  csrfToken: string
  onOpenReset: () => void
  onChangeRole: (role: string) => void
  onToggleDisabled: () => void
}) {
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [passwordError, setPasswordError] = useState<string | null>(null)
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  async function submitReset(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (busy) return
    setBusy(true)
    setPasswordError(null)
    setMessage(null)
    setError(null)
    try {
      await resetPersonPasswordEntry(person.userId, password, csrfToken)
      setPassword('')
      setMessage('Password reset. The user must sign in again.')
    } catch (caught) {
      if (caught instanceof AdminApiError && caught.code === 'invalid_password') {
        setPasswordError(caught.message)
      }
      setError(caught instanceof Error ? caught.message : 'Unable to reset the password')
    } finally {
      setBusy(false)
    }
  }

  return (
    <>
      <tr>
        <th scope="row" data-label="Username">
          {person.username}
          {isMe && <small className="muted"> · you</small>}
        </th>
        <td data-label="Role">
          <select
            aria-label={`Role of ${person.username}`}
            value={person.role}
            disabled={roleBusy || person.disabled}
            onChange={(event) => onChangeRole(event.target.value)}
          >
            <option value="owner">Owner</option>
            <option value="viewer">Viewer</option>
          </select>
          {roleBusy && <small className="muted"> updating…</small>}
        </td>
        <td data-label="Status">
          {person.disabled ? (
            <StatusBadge status="Disabled" tone="error" />
          ) : (
            <span className="account-state account-state-enabled">
              <span aria-hidden="true">✓</span> Enabled
            </span>
          )}
        </td>
        <td data-label="Sessions">{person.sessionCount}</td>
        <td data-label="Created">{formatObservedAt(person.createdAt)}</td>
        <td data-label="Actions">
          <button
            type="button"
            className="text-action"
            disabled={statusBusy}
            onClick={onToggleDisabled}
          >
            {statusBusy
              ? 'Updating…'
              : person.disabled
                ? 'Enable'
                : isMe
                  ? 'Disable me'
                  : 'Disable'}
          </button>
          <button type="button" className="text-action" onClick={onOpenReset}>
            {resetOpen ? 'Close reset' : 'Reset password'}
          </button>
        </td>
      </tr>
      {resetOpen && (
        <tr className="node-detail-row">
          <td colSpan={6}>
            <form onSubmit={submitReset} className="reset-password-form">
              <div className="field">
                <label htmlFor={`reset-password-${person.userId}`}>
                  New password for {person.username}
                </label>
                <input
                  id={`reset-password-${person.userId}`}
                  type="password"
                  autoComplete="new-password"
                  value={password}
                  onChange={(event) => setPassword(event.target.value)}
                  required
                  minLength={12}
                  aria-invalid={passwordError ? true : undefined}
                  aria-describedby={
                    passwordError ? `reset-password-${person.userId}-error` : undefined
                  }
                />
                {passwordError && (
                  <p
                    className="field-error"
                    id={`reset-password-${person.userId}-error`}
                    role="alert"
                  >
                    {passwordError}
                  </p>
                )}
              </div>
              <button type="submit" className="danger-action" disabled={busy}>
                {busy ? 'Resetting…' : 'Reset password'}
              </button>
              <p className="muted">
                All of {person.username}'s Sessions are revoked immediately.
              </p>
              {message && (
                <p className="form-success" role="status">
                  {message}
                </p>
              )}
              {error && (
                <p className="form-error" role="alert">
                  {error}
                </p>
              )}
            </form>
          </td>
        </tr>
      )}
    </>
  )
}

type FieldErrors = { username?: string; password?: string; role?: string }

/** Map a typed mutation error to its offending field (issue #47: forms
 * surface field-level errors plus the page summary). */
function fieldErrorsFrom(error: unknown): FieldErrors {
  if (!(error instanceof AdminApiError)) return {}
  switch (error.code) {
    case 'invalid_username':
    case 'username_taken':
      return { username: error.message }
    case 'invalid_password':
      return { password: error.message }
    case 'invalid_role':
      return { role: error.message }
    default:
      return {}
  }
}

function CreatePersonForm({
  csrfToken,
  onMessage,
  onError,
}: {
  csrfToken: string
  onMessage: (message: string | null) => void
  onError: (error: string | null) => void
}) {
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [role, setRole] = useState<'owner' | 'viewer'>('viewer')
  const [busy, setBusy] = useState(false)
  const [fieldErrors, setFieldErrors] = useState<FieldErrors>({})
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (busy) return
    setBusy(true)
    setFieldErrors({})
    setMessage(null)
    setError(null)
    onMessage(null)
    onError(null)
    try {
      const person = await createPersonEntry({ username, password, role }, csrfToken)
      setUsername('')
      setPassword('')
      setMessage(`${person.username} created as ${person.role === 'owner' ? 'an Owner' : 'a Viewer'}.`)
    } catch (caught) {
      setFieldErrors(fieldErrorsFrom(caught))
      setError(caught instanceof Error ? caught.message : 'Unable to create the user')
    } finally {
      setBusy(false)
    }
  }

  return (
    <article className="panel">
      <h2>Create a user</h2>
      <p className="muted">
        No public registration exists; Owners provision accounts. The password
        is hashed by the Server and never displayed or stored in plaintext.
      </p>
      <form onSubmit={submit} className="create-person-form" noValidate>
        <div className="field">
          <label htmlFor="person-username">Username</label>
          <input
            id="person-username"
            value={username}
            onChange={(event) => setUsername(event.target.value)}
            required
            maxLength={64}
            autoComplete="off"
            aria-invalid={fieldErrors.username ? true : undefined}
            aria-describedby={fieldErrors.username ? 'person-username-error' : undefined}
          />
          {fieldErrors.username && (
            <p className="field-error" id="person-username-error" role="alert">
              {fieldErrors.username}
            </p>
          )}
        </div>
        <div className="field">
          <label htmlFor="person-password">Password</label>
          <input
            id="person-password"
            type="password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            required
            minLength={12}
            autoComplete="new-password"
            aria-invalid={fieldErrors.password ? true : undefined}
            aria-describedby={fieldErrors.password ? 'person-password-error' : undefined}
          />
          {fieldErrors.password && (
            <p className="field-error" id="person-password-error" role="alert">
              {fieldErrors.password}
            </p>
          )}
        </div>
        <div className="field">
          <label htmlFor="person-role">Role</label>
          <select
            id="person-role"
            value={role}
            onChange={(event) => setRole(event.target.value as 'owner' | 'viewer')}
            aria-invalid={fieldErrors.role ? true : undefined}
            aria-describedby={fieldErrors.role ? 'person-role-error' : undefined}
          >
            <option value="viewer">Viewer</option>
            <option value="owner">Owner</option>
          </select>
          {fieldErrors.role && (
            <p className="field-error" id="person-role-error" role="alert">
              {fieldErrors.role}
            </p>
          )}
        </div>
        <button type="submit" className="primary-action" disabled={busy}>
          {busy ? 'Creating…' : 'Create user'}
        </button>
      </form>
      {message && (
        <p className="form-success" role="status">
          {message}
        </p>
      )}
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
    </article>
  )
}
