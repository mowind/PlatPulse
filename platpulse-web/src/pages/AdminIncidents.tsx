import { useState } from 'react'
import { Link, useParams } from 'react-router'
import {
  type IncidentFilters,
  useAdminIncidentDetail,
  useAdminIncidents,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { formatTs, ruleStateBadge } from './AdminAlertRules'
import type { IncidentListItem } from '../api/generated'

/**
 * PAGE-ADMIN-INCIDENTS / PAGE-ADMIN-INCIDENT (design §4.4, webui.md §8.3):
 * durable Incident history. Incidents open after a Rule stays firing and
 * resolve only after sustained fresh Known recovery; they are never
 * manually resolvable, reopened, or deleted. The detail view shows the
 * immutable opening/resolution evidence, the independent current evaluation
 * state, and overlapping Silence/Maintenance suppressions (both reasons
 * remain visible independently).
 */

function severityLabel(severity: string | undefined): string {
  switch (severity) {
    case 'info':
      return 'Info'
    case 'warning':
      return 'Warning'
    case 'critical':
      return 'Critical'
    default:
      return severity ?? 'Unknown'
  }
}

export default function AdminIncidentsList() {
  const { generation } = useAuth()
  const [stateFilter, setStateFilter] = useState('open')
  const filters: IncidentFilters = { state: stateFilter || undefined, limit: 100 }
  const query = useAdminIncidents(generation, filters)

  return (
    <section className="page">
      <h1>Incidents</h1>
      <p className="muted">
        Incidents are durable: they open when a Rule stays firing and resolve only when fresh
        Known recovery holds for the recovery duration. Unknown or Stale input never resolves
        them, and they cannot be edited or deleted.
      </p>
      <div className="filter-bar" role="group" aria-label="Incident filters">
        <label htmlFor="incident-state-filter">State</label>
        <select
          id="incident-state-filter"
          value={stateFilter}
          onChange={(event) => setStateFilter(event.target.value)}
        >
          <option value="open">Open</option>
          <option value="resolved">Resolved</option>
          <option value="">All</option>
        </select>
      </div>
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading Incidents…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load Incidents'}
          <button type="button" className="text-action" onClick={() => void query.refetch()}>
            Try again
          </button>
        </p>
      )}
      {query.data && query.data.incidents.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" />{' '}
          {stateFilter ? `No ${stateFilter} Incidents.` : 'No Incidents.'}
        </p>
      )}
      {query.data && query.data.incidents.length > 0 && (
        <div className="table-wrap">
          <table className="node-table">
            <caption className="sr-only">Incidents with rule, subject, and state</caption>
            <thead>
              <tr>
                <th scope="col">Incident</th>
                <th scope="col">Rule</th>
                <th scope="col">Subject</th>
                <th scope="col">Severity</th>
                <th scope="col">State</th>
                <th scope="col">Opened</th>
              </tr>
            </thead>
            <tbody>
              {query.data.incidents.map((incident) => (
                <IncidentRow key={incident.incidentId} incident={incident} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  )
}

function IncidentRow({ incident }: { incident: IncidentListItem }) {
  return (
    <tr>
      <td data-label="Incident">
        <Link to={`/admin/alerts/incidents/${encodeURIComponent(incident.incidentId)}`}>
          {incident.incidentId.slice(0, 8)}
        </Link>
      </td>
      <td data-label="Rule">{incident.ruleKey}</td>
      <td data-label="Subject">
        {incident.subjectKind} · {incident.subjectKey}
      </td>
      <td data-label="Severity">{severityLabel(incident.severity)}</td>
      <td data-label="State">
        {incident.state === 'open' ? (
          <StatusBadge status="Open" tone="error" />
        ) : (
          <StatusBadge status="Resolved" tone="ok" />
        )}
      </td>
      <td data-label="Opened">{formatObservedAt(incident.openedAt)}</td>
    </tr>
  )
}

/** PAGE-ADMIN-INCIDENT: immutable evidence, independent evaluation, and
 * overlapping suppressions. */
export function AdminIncidentDetail() {
  const { incidentId = '' } = useParams()
  const { generation } = useAuth()
  const query = useAdminIncidentDetail(generation, incidentId)

  return (
    <section className="page">
      <p>
        <Link to="/admin/alerts/incidents" className="text-action">
          ← Incidents
        </Link>
      </p>
      <h1>Incident {incidentId.slice(0, 8)}</h1>
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Incident…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load the Incident'}
        </p>
      )}
      {query.data && (
        <>
          <dl className="detail-list">
            <dt>Rule</dt>
            <dd>
              <Link to={`/admin/alerts/rules/${encodeURIComponent(query.data.ruleKey)}`}>
                {query.data.ruleKey}
              </Link>{' '}
              (version {query.data.ruleVersion})
            </dd>
            <dt>Subject</dt>
            <dd>
              {query.data.subjectKind} · {query.data.subjectKey}
            </dd>
            <dt>Severity</dt>
            <dd>{severityLabel(query.data.severity)}</dd>
            <dt>State</dt>
            <dd>
              {query.data.state === 'open' ? (
                <StatusBadge status="Open" tone="error" />
              ) : (
                <StatusBadge status="Resolved" tone="ok" />
              )}
            </dd>
            <dt>Sequence</dt>
            <dd>{query.data.sequence}</dd>
            <dt>Opened</dt>
            <dd>{formatObservedAt(query.data.openedAt)}</dd>
            <dt>Resolved</dt>
            <dd>{formatObservedAt(query.data.resolvedAt)}</dd>
          </dl>

          <h2>Current evaluation</h2>
          {query.data.evaluation ? (
            <dl className="detail-list">
              <dt>State</dt>
              <dd>
                {ruleStateBadge(query.data.evaluation.state)}
                {query.data.evaluation.evaluationUnavailable && (
                  <>
                    {' '}
                    <StatusBadge status="Evaluation unavailable" tone="warning" />
                  </>
                )}
              </dd>
              <dt>Input</dt>
              <dd>
                {query.data.evaluation.inputKind} —{' '}
                {query.data.evaluation.inputDetail ?? 'no detail'}
              </dd>
              <dt>Last evaluated</dt>
              <dd>{formatTs(query.data.evaluation.lastEvaluatedAt)}</dd>
            </dl>
          ) : (
            <p className="panel-state">
              <StatusBadge status="Unknown" tone="neutral" /> No evaluation state recorded.
            </p>
          )}

          <h2>Opening evidence</h2>
          <EvidenceTable evidence={query.data.openedEvidence} />

          {query.data.resolvedEvidence && (
            <>
              <h2>Resolution evidence</h2>
              <EvidenceTable evidence={query.data.resolvedEvidence} />
            </>
          )}

          <h2>Suppression</h2>
          {query.data.suppressions.length === 0 && (
            <p className="panel-state">
              <StatusBadge status="Empty" tone="ok" /> No active Silence or Maintenance Window
              matches this Incident.
            </p>
          )}
          {query.data.suppressions.length > 0 && (
            <ul className="node-list">
              {query.data.suppressions.map((suppression) => (
                <li key={`${suppression.kind}:${suppression.id}`}>
                  <span>
                    {suppression.kind === 'silence' ? (
                      <StatusBadge status="Silence" tone="neutral" />
                    ) : (
                      <StatusBadge status="Maintenance" tone="warning" />
                    )}{' '}
                    {suppression.reason}
                  </span>
                  <small className="muted">
                    until {formatTs(suppression.endsAt)}
                    {suppression.marksIncident && ' · marks Incident suppressed'}
                  </small>
                </li>
              ))}
            </ul>
          )}
          <p className="muted">
            Silence suppresses delivery only; Maintenance suppresses expected delivery and
            marks the Incident suppressed. Both reasons stay visible independently when they
            overlap — neither changes the facts or the Incident history.
          </p>
        </>
      )}
    </section>
  )
}

function EvidenceTable({ evidence }: { evidence: unknown }) {
  if (evidence == null || typeof evidence !== 'object') {
    return (
      <p className="panel-state">
        <StatusBadge status="Empty" tone="ok" /> No evidence recorded.
      </p>
    )
  }
  const rows = Object.entries(evidence as Record<string, unknown>)
  if (rows.length === 0) {
    return (
      <p className="panel-state">
        <StatusBadge status="Empty" tone="ok" /> No evidence recorded.
      </p>
    )
  }
  return (
    <dl className="detail-list">
      {rows.map(([key, value]) => (
        <div key={key}>
          <dt>{key.replaceAll('_', ' ')}</dt>
          <dd>{typeof value === 'object' ? JSON.stringify(value) : String(value)}</dd>
        </div>
      ))}
    </dl>
  )
}
