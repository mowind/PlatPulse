import { useAuth } from '../auth/AuthContext'

/**
 * The Server could not be asked about this document's Session (a restart, a
 * `502`/`503`, a database failure, a dropped connection), so the reading on
 * screen is the last one the Server gave, not a live one. The reader keeps the
 * access they already had: the notice states the outage instead of letting a
 * stale shell pass for a current one (webui.md §3.3, issue #267).
 */
export function ServerOutageNotice() {
  const { outage } = useAuth()
  if (!outage) return null
  return (
    <p
      role="status"
      className="mx-4 mb-4 rounded-md border border-warning/40 bg-warning/10 px-4 py-3 text-sm text-warning-foreground dark:text-warning"
    >
      The Server cannot confirm your Session right now. Retrying automatically.
    </p>
  )
}
