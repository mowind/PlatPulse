import { Link } from 'react-router'
import {
  operationKindLabel,
  operationStatusLabel,
  operationTone,
  useAdminBackups,
  useAdminDoctor,
  useAdminRetention,
  verificationLabel,
  verificationTone,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'

/**
 * PAGE-ADMIN-DATA (webui.md §4.5): DB/worker, retention, backup, and
 * Doctor summary. Panels fail independently; the page never fabricates
 * values for absent state.
 */

export default function AdminData() {
  const { generation } = useAuth()
  const retention = useAdminRetention(generation)
  const backups = useAdminBackups(generation)
  const doctor = useAdminDoctor(generation)

  const lastVerified = backups.data?.find((artifact) => artifact.verification === 'ok')
  const latest = backups.data?.[0]
  const doctorChecks = doctor.data?.checks ?? []

  return (
    <section className="page">
      <h1>Data and maintenance</h1>
      <p className="muted">
        Retention, backup, and Doctor operations are bounded, audited, and recoverable
        through REST. This page summarizes each surface; follow the links for the full
        controls.
      </p>

      <h2>Retention</h2>
      {retention.isError && (
        <p className="form-error" role="alert">
          Unable to load retention state.
        </p>
      )}
      {retention.isSuccess && (
        <dl className="detail-list">
          <div>
            <dt>Policies</dt>
            <dd>
              <Link to="/admin/data/retention">
                {retention.data.policies.length} families configured within safety bounds
              </Link>
            </dd>
          </div>
          <div>
            <dt>Last run</dt>
            <dd>
              {retention.data.lastRun ? (
                <>
                  <StatusBadge
                    status={operationStatusLabel(retention.data.lastRun.status)}
                    tone={operationTone(retention.data.lastRun.status)}
                  />{' '}
                  <Link to={`/admin/operations/${retention.data.lastRun.operationId}`}>
                    {operationKindLabel(retention.data.lastRun.kind)}
                  </Link>{' '}
                  <small>{formatObservedAt(retention.data.lastRun.createdAt)}</small>
                </>
              ) : (
                'Never run'
              )}
            </dd>
          </div>
        </dl>
      )}

      <h2>Backups</h2>
      {backups.isError && (
        <p className="form-error" role="alert">
          Unable to load backup state.
        </p>
      )}
      {backups.isSuccess && (
        <dl className="detail-list">
          <div>
            <dt>Latest artifact</dt>
            <dd>
              {latest ? (
                <>
                  <Link to={`/admin/data/backups/${latest.artifactId}`}>
                    {latest.filename}
                  </Link>{' '}
                  <StatusBadge
                    status={verificationLabel(latest.verification)}
                    tone={verificationTone(latest.verification)}
                  />
                </>
              ) : (
                'No backup artifact yet'
              )}
            </dd>
          </div>
          <div>
            <dt>Last verified</dt>
            <dd>
              {lastVerified ? (
                <>
                  <Link to={`/admin/data/backups/${lastVerified.artifactId}`}>
                    {lastVerified.filename}
                  </Link>{' '}
                  <small>
                    {lastVerified.verifiedAt
                      ? formatObservedAt(lastVerified.verifiedAt)
                      : 'verified'}
                  </small>
                </>
              ) : (
                'None verified'
              )}
            </dd>
          </div>
          <div>
            <dt>Artifacts</dt>
            <dd>
              <Link to="/admin/data/backups">{backups.data.length} stored</Link>
            </dd>
          </div>
        </dl>
      )}

      <h2>Restore</h2>
      <p className="muted">
        Restore requires an exclusive stopped Server, checksum and schema validation, and
        a typed confirmation. It never restores secret files and preserves the current
        database on failure.
      </p>
      <p>
        <Link className="danger-action" to="/admin/data/restore">
          Open the Restore workflow
        </Link>
      </p>

      <h2>Doctor</h2>
      {doctor.isError && (
        <p className="form-error" role="alert">
          Unable to load the Doctor report.
        </p>
      )}
      {doctor.isSuccess && (
        <dl className="detail-list">
          <div>
            <dt>Last report</dt>
            <dd>
              {doctor.data.lastRun ? (
                <>
                  <StatusBadge
                    status={operationStatusLabel(doctor.data.lastRun.status)}
                    tone={operationTone(doctor.data.lastRun.status)}
                  />{' '}
                  <Link to={`/admin/operations/${doctor.data.lastRun.operationId}`}>
                    {operationKindLabel(doctor.data.lastRun.kind)}
                  </Link>{' '}
                  <small>{formatObservedAt(doctor.data.lastRun.createdAt)}</small>
                </>
              ) : (
                'Never run'
              )}
            </dd>
          </div>
          <div>
            <dt>Checks</dt>
            <dd>
              <Link to="/admin/data/doctor">
                {doctorChecks.length > 0
                  ? `${doctorChecks.length} checks — ${doctorChecks
                      .map((check) => check.status)
                      .join(', ')}`
                  : 'No checks yet'}
              </Link>
            </dd>
          </div>
        </dl>
      )}
    </section>
  )
}
