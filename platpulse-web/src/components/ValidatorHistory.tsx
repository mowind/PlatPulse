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

function Fact({ label, value }: { label: string; value: string | number }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs font-medium tracking-wider text-muted-foreground">{label}</dt>
      <dd className="m-0 break-words text-sm">{value}</dd>
    </div>
  )
}

export function ValidatorHistory({ entries, compact = false }: { entries: Entry[]; compact?: boolean }) {
  if (entries.length === 0) return <p className="m-0 text-sm text-muted-foreground">No confirmed Validator changes.</p>
  return (
    <section className="mt-3 min-w-0" aria-label="Validator history">
      <h3 className="m-0 mb-2 text-sm font-medium">Confirmed history</h3>
      <div className="grid gap-2">
        {entries.map((entry, index) => (
          <details
            className={cnDetails(compact)}
            key={'historyId' in entry ? entry.historyId : `${entry.kind}-${entry.observedAt}-${index}`}
          >
            <summary className="flex min-h-11 cursor-pointer flex-wrap items-baseline gap-x-2 gap-y-1 px-3 py-2 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring [&::-webkit-details-marker]:hidden">
              <span aria-hidden="true" className="text-muted-foreground transition-transform group-open:rotate-90">▸</span>
              <span className="min-w-0 break-words font-medium">{entryLabel(entry)}</span>
              <span className="text-[11px] text-muted-foreground">{formatDate(entry.observedAt)}</span>
            </summary>
            <dl className="m-0 grid grid-cols-[repeat(auto-fit,minmax(9rem,1fr))] gap-x-4 gap-y-2 px-3 pb-3">
              {entry.kind === 'ranking_changed' && <>
                <Fact label="Previous rank" value={entry.previousRank ?? 'Unknown'} />
                <Fact label="Confirmed rank" value={entry.currentRank ?? 'Unknown'} />
              </>}
              {entry.kind !== 'ranking_changed' && <>
                <Fact label="Counter" value={entry.counterName ?? 'Unknown'} />
                <Fact label="Previous value" value={entry.previousValue ?? 'Unknown'} />
                <Fact label="Corrected value" value={entry.currentValue ?? 'Unknown'} />
              </>}
              <Fact label="Provider time" value={entry.providerTimestamp ? formatDate(entry.providerTimestamp) : 'Unknown'} />
              {'linkRoles' in entry && <Fact label="Link roles" value={entry.linkRoles.join(', ') || 'Unknown'} />}
              {isAdminEntry(entry) && <AdminEvidence links={entry.links} candidateObservedAt={entry.candidateObservedAt} />}
            </dl>
          </details>
        ))}
      </div>
    </section>
  )
}

function cnDetails(compact: boolean): string {
  return [
    'group min-w-0 overflow-hidden rounded-md border border-border/60 bg-background/40',
    compact ? 'text-sm' : '',
  ].join(' ').trim()
}

function AdminEvidence({ links, candidateObservedAt }: { links: LinkContext[]; candidateObservedAt?: string | null }) {
  return <>
    <Fact label="Candidate observed" value={candidateObservedAt ? formatDate(candidateObservedAt) : 'Not applicable'} />
    <Fact label="Node Validator Links" value={links.length === 0 ? 'None recorded' : links.map((link) => `${link.role} (${link.nodeId})`).join(', ')} />
  </>
}
