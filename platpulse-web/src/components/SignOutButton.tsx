import { useState } from 'react'
import { useAuth } from '../auth/AuthContext'

/** Sign out of the current human session (design §12.3). A failed
 * revocation keeps the session and shows the failure instead of silently
 * pretending the user signed out. */
export default function SignOutButton() {
  const { logout } = useAuth()
  const [error, setError] = useState<string | null>(null)
  return (
    <>
      <button
        type="button"
        className="sign-out"
        onClick={() => {
          setError(null)
          void logout().catch(() => {
            setError('Could not sign out. Try again.')
          })
        }}
      >
        Sign out
      </button>
      {error && (
        <span className="sign-out-error" role="alert">
          {error}
        </span>
      )}
    </>
  )
}
