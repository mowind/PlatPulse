import { useState } from 'react'
import { Link } from 'react-router'
import { updateAccessSettings, useAdminAccess } from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge } from '../components/StatusBadge'

/** PAGE-ADMIN-SITE-ACCESS: audited Home access policy transition. */
export default function AdminSiteAccess() {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const query = useAdminAccess(generation)
  const [busy, setBusy] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  async function toggle() {
    if (!query.data || busy) return
    const next = query.data.mode === 'public' ? 'private' : 'public'
    if (!window.confirm(next === 'public'
      ? 'Make Home Public? Unauthenticated visitors will be able to read Home projections.'
      : 'Make Home Private? Unauthenticated visitors will lose access to Home projections.')) return
    setBusy(true)
    setNotice(null)
    setError(null)
    try {
      const result = await updateAccessSettings(next, csrfToken)
      setNotice(`Site Access Mode is now ${result.mode === 'public' ? 'Public' : 'Private'}. Audit was recorded.`)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to update Site Access Mode.')
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="page">
      <p><Link to="/admin">← Admin overview</Link></p>
      <h1>Site Access</h1>
      <p className="muted">This Owner-only setting gates Home only. Admin always requires an Owner Session. Every change closes affected streams, clears protected caches, and starts a new authorization generation.</p>
      {!query.data && query.isPending && <p role="status">Loading Site Access Mode…</p>}
      {!query.data && query.isError && <p className="form-error" role="alert">Unable to load Site Access Mode.</p>}
      {query.data && (
        <article className="panel">
          <div className="panel-heading">
            <h2>Current mode</h2>
            <StatusBadge status={query.data.mode === 'public' ? 'Public' : 'Private'} tone={query.data.mode === 'public' ? 'ok' : 'neutral'} />
          </div>
          <p>{query.data.mode === 'public' ? 'Everyone can read the Home Public Projection without signing in.' : 'Home requires a Human Session.'}</p>
          <button className="primary-action" type="button" disabled={busy} onClick={() => void toggle()}>
            {busy ? 'Updating…' : query.data.mode === 'public' ? 'Make Home Private' : 'Make Home Public'}
          </button>
          {notice && <p className="form-success" role="status">{notice}</p>}
          {error && <p className="form-error" role="alert">{error}</p>}
        </article>
      )}
    </section>
  )
}
