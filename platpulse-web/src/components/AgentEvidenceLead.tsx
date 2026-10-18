import type { EvidenceSummary, EvidenceTone } from '../agentEvidence'
import { cn } from '../lib/utils'

/** Text treatment per evidence tone; colour is never the only signal (a
 *  Critical lead and every group carry words). */
export const EVIDENCE_TONE: Record<EvidenceTone, string> = {
  error: 'font-medium text-destructive',
  warning: 'text-foreground',
  neutral: 'text-muted-foreground',
}

/** The headline of an evidence summary (summarizeEvidence): the most severe
 *  Current item — a Critical one says so in words, not only in colour — or,
 *  without any Current item, the first Not observed one, labelled as such and
 *  never as Current. Shared by the Agents row and the Overview preview so both
 *  read the same. */
export function EvidenceLead({ summary, className }: { summary: EvidenceSummary; className?: string }) {
  const lead = summary.lead
  if (!lead) return null
  return (
    <span className={cn('[overflow-wrap:anywhere]', EVIDENCE_TONE[lead.item.tone], className)} data-group={lead.group}>
      {lead.group === 'unknown' ? (
        <span className="font-normal text-muted-foreground">Not observed · </span>
      ) : lead.item.severity === 'critical' ? (
        <span>Critical · </span>
      ) : null}
      {lead.item.text}
    </span>
  )
}
