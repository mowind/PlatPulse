import { useState } from 'react'
import { useAuth } from '../auth/AuthContext'
import { Button } from './ui/button'

/** Sign out of the current human session (design §12.3). A failed
 * revocation keeps the session and shows the failure instead of silently
 * pretending the user signed out. */
export default function SignOutButton() {
  const { logout } = useAuth()
  const [error, setError] = useState<string | null>(null)
  return (
    <>
      <Button
        variant="ghost"
        size="sm"
        className="text-muted-foreground hover:text-foreground"
        onClick={() => {
          setError(null)
          void logout().catch(() => {
            setError('Could not sign out. Try again.')
          })
        }}
      >
        Sign out
      </Button>
      {error && (
        <span className="text-xs text-destructive" role="alert">
          {error}
        </span>
      )}
    </>
  )
}
