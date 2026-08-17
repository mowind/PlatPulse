import type { AdminValidatorHistoryEntry, PublicValidatorHistoryEntry } from '../api/generated'

type Entry = AdminValidatorHistoryEntry | PublicValidatorHistoryEntry

type LinkContext = AdminValidatorHistoryEntry['links'][number]

function formatDate(value: string): string {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString()
}

function isAdminEntry(entry: Entry): entry is AdminValidatorHistoryEntry {
  return 'links' in entry
}

function entryLabel(entry: Entry): string {
  if (entry.kind === 'ranking_changed') {
    return `Ranking changed: ${entry.previousRank ?? 'Unknown'} → ${entry.currentRank ?? 'Unknown'}`
  }
  return `Counter reset or correction: ${entry.counterName ?? 'counter'}`
}

export function ValidatorHistory({ entries, compact = false }: { entries: Entry[]; compact?: boolean }) {
  if (entries.length === 0) return <p className="muted">No confirmed Validator changes.</p>
  return (
    <section className={`validator-history${compact ? ' validator-history-compact' : ''}`} aria-label="Validator history">
      <h3>Confirmed history</h3>
      <div className="validator-history-list">
        {entries.map((entry, index) => (
          <details className="validator-history-entry" key={'historyId' in entry ? entry.historyId : `${entry.kind}-${entry.observedAt}-${index}`}>
            <summary>{entryLabel(entry)} <span className="table-secondary">{formatDate(entry.observedAt)}</span></summary>
            <dl className="detail-list">
              {entry.kind === 'ranking_changed' && <>
                <div><dt>Previous rank</dt><dd>{entry.previousRank ?? 'Unknown'}</dd></div>
                <div><dt>Confirmed rank</dt><dd>{entry.currentRank ?? 'Unknown'}</dd></div>
              </>}
              {entry.kind !== 'ranking_changed' && <>
                <div><dt>Counter</dt><dd>{entry.counterName ?? 'Unknown'}</dd></div>
                <div><dt>Previous value</dt><dd>{entry.previousValue ?? 'Unknown'}</dd></div>
                <div><dt>Corrected value</dt><dd>{entry.currentValue ?? 'Unknown'}</dd></div>
              </>}
              <div><dt>Provider time</dt><dd>{entry.providerTimestamp ? formatDate(entry.providerTimestamp) : 'Unknown'}</dd></div>
              {'linkRoles' in entry && <div><dt>Link roles</dt><dd>{entry.linkRoles.join(', ') || 'Unknown'}</dd></div>}
              {isAdminEntry(entry) && <AdminEvidence links={entry.links} candidateObservedAt={entry.candidateObservedAt} />}
            </dl>
          </details>
        ))}
      </div>
    </section>
  )
}

function AdminEvidence({ links, candidateObservedAt }: { links: LinkContext[]; candidateObservedAt?: string | null }) {
  return <>
    <div><dt>Candidate observed</dt><dd>{candidateObservedAt ? formatDate(candidateObservedAt) : 'Not applicable'}</dd></div>
    <div><dt>Node Validator Links</dt><dd>{links.length === 0 ? 'None recorded' : links.map((link) => `${link.role} (${link.nodeId})`).join(', ')}</dd></div>
  </>
}
