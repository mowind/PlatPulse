import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router'
import {
  AdminApiError,
  useAdminBackup,
  verifyBackupEntry,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { formatBytes } from './AdminBackups'

/**
 * PAGE-ADMIN-BACKUP (webui.md §4.5/§8.4): artifact detail and verify
 * action. Verification recomputes the checksum and runs a read-only
 * integrity/schema check on the snapshot; a failed verification never
 * deletes the artifact or any previous one.
 */

function verificationTone(verification: string | undefined): 'ok' | 'warning' | 'error' {
  switch (verification) {
    case 'ok':
      return 'ok'
    case 'failed':
      return 'error'
    default:
      return 'warning'
  }
}

function verificationLabel(verification: string | undefined): string {
  switch (verification) {
    case 'ok':
      return 'Verified'
    case 'failed':
      return 'Verification failed'
    case 'pending':
      return 'Not verified'
    default:
      return 'Unknown'
  }
}

export default function AdminBackupDetail() {
  const { artifactId = '' } = useParams()
  const navigate = useNavigate()
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const query = useAdminBackup(generation, artifactId)
  const [verifying, setVerifying] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const artifact = query.data?.artifact

  async function onVerify() {
    if (!artifact || verifying) return
    setVerifying(true)
    setError(null)
    try {
      const response = await verifyBackupEntry(artifact.artifactId, csrfToken)
      navigate(`/admin/operations/${response.operation.operation.operationId}`)
    } catch (caught) {
      setError(
        caught instanceof AdminApiError
          ? caught.message
          : 'Unable to start the verification.',
      )
      setVerifying(false)
    }
  }

  return (
    <section className="page">
      <p>
        <Link to="/admin/data/backups">← Backups</Link>
      </p>
      <h1>{artifact?.filename ?? 'Backup artifact'}</h1>
      {query.isError && (
        <p className="form-error" role="alert">
          Unable to load the backup artifact.{' '}
          {query.error instanceof Error ? query.error.message : ''}
        </p>
      )}
      {query.isSuccess && artifact && (
        <>
          <div className="page-actions">
            <StatusBadge
              status={verificationLabel(artifact.verification)}
              tone={verificationTone(artifact.verification)}
            />
            <button
              type="button"
              className="primary-action"
              onClick={onVerify}
              disabled={verifying}
            >
              {verifying ? 'Queuing…' : 'Verify artifact'}
            </button>
          </div>
          {error && (
            <p className="form-error" role="alert">
              {error}
            </p>
          )}
          <h2>Artifact metadata</h2>
          <dl className="detail-list">
            <div>
              <dt>Filename</dt>
              <dd>{artifact.filename}</dd>
            </div>
            <div>
              <dt>Size</dt>
              <dd>{formatBytes(artifact.bytes)}</dd>
            </div>
            <div>
              <dt>SHA-256</dt>
              <dd>
                <code>{artifact.sha256}</code>
              </dd>
            </div>
            <div>
              <dt>Schema version</dt>
              <dd>{artifact.schemaVersion}</dd>
            </div>
            <div>
              <dt>Server version</dt>
              <dd>{artifact.serverVersion}</dd>
            </div>
            <div>
              <dt>Created</dt>
              <dd>{formatObservedAt(artifact.createdAt)}</dd>
            </div>
            <div>
              <dt>Data range</dt>
              <dd>
                {artifact.dataRangeMin && artifact.dataRangeMax
                  ? `${artifact.dataRangeMin.slice(0, 19)} → ${artifact.dataRangeMax.slice(0, 19)}`
                  : 'No block data yet'}
              </dd>
            </div>
            <div>
              <dt>Verified at</dt>
              <dd>
                {artifact.verifiedAt ? formatObservedAt(artifact.verifiedAt) : 'Never verified'}
              </dd>
            </div>
            <div>
              <dt>Create Operation</dt>
              <dd>
                {artifact.createOperationId ? (
                  <Link to={`/admin/operations/${artifact.createOperationId}`}>
                    {artifact.createOperationId.slice(0, 8)}
                  </Link>
                ) : (
                  '—'
                )}
              </dd>
            </div>
          </dl>
          {query.data.verificationError && (
            <>
              <h2>Verification error</h2>
              <p className="form-error" role="alert">
                {query.data.verificationError}
              </p>
            </>
          )}
        </>
      )}
    </section>
  )
}
