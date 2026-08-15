import { Link } from 'react-router'
import {
  useAdminBackups,
  verificationLabel,
  verificationTone,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'

/**
 * PAGE-ADMIN-BACKUPS (webui.md §4.5/§8.4): artifact list and integrity
 * state. Sanitized metadata only — file base name, size, checksum, schema,
 * Server version, timestamps, and verification. Database contents and
 * secrets are never displayed, and no download endpoint exists.
 */

export default function AdminBackupsList() {
  const { generation } = useAuth()
  const query = useAdminBackups(generation)

  return (
    <section className="page">
      <h1>Backups</h1>
      <p className="muted">
        Backups are consistent snapshots stored in the configured backup directory. Only
        sanitized metadata is shown: database contents are never displayed and there is no
        download endpoint.
      </p>
      <div className="page-actions">
        <Link className="primary-action" to="/admin/data/backups/create">
          Create a backup
        </Link>
      </div>
      {query.isError && (
        <p className="form-error" role="alert">
          Unable to load backup artifacts.{' '}
          {query.error instanceof Error ? query.error.message : ''}
        </p>
      )}
      {query.isSuccess && query.data.length === 0 && (
        <p className="muted">No backup artifacts yet.</p>
      )}
      <div className="table-wrap">
        <table className="node-table">
          <caption className="visually-hidden">Backup artifacts</caption>
          <thead>
            <tr>
              <th scope="col">Artifact</th>
              <th scope="col">Size</th>
              <th scope="col">Checksum</th>
              <th scope="col">Schema</th>
              <th scope="col">Verification</th>
              <th scope="col">Created</th>
            </tr>
          </thead>
          <tbody>
            {query.data?.map((artifact) => (
              <tr key={artifact.artifactId}>
                <td data-label="Artifact">
                  <Link to={`/admin/data/backups/${artifact.artifactId}`}>
                    {artifact.filename}
                  </Link>
                  <small>{artifact.serverVersion}</small>
                </td>
                <td data-label="Size">{formatBytes(artifact.bytes)}</td>
                <td data-label="Checksum">
                  <code className="sha256-summary">{artifact.sha256.slice(0, 16)}…</code>
                </td>
                <td data-label="Schema">{artifact.schemaVersion}</td>
                <td data-label="Verification">
                  <StatusBadge
                    status={verificationLabel(artifact.verification)}
                    tone={verificationTone(artifact.verification)}
                  />
                  {artifact.verifiedAt && (
                    <small>{formatObservedAt(artifact.verifiedAt)}</small>
                  )}
                </td>
                <td data-label="Created">
                  <small>{formatObservedAt(artifact.createdAt)}</small>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  )
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`
}
