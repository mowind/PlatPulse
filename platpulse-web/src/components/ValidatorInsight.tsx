import type { AdminValidatorInsight, AdminValidatorHistoryEntry, PublicValidatorInsight, PublicValidatorHistoryEntry } from '../api/generated'
import { ValidatorHistory } from './ValidatorHistory'

type Insight = PublicValidatorInsight | AdminValidatorInsight

function stateLabel(state: string): string {
  switch (state) {
    case 'fresh': return 'Fresh'
    case 'stale': return 'Stale'
    case 'error': return 'Error'
    case 'unsupported': return 'Unsupported'
    case 'not_found': return 'Not found'
    case 'empty': return 'Empty'
    default: return 'Unknown'
  }
}

export function ValidatorInsight({ insight, compact = false, history }: { insight: Insight; compact?: boolean; history?: Array<AdminValidatorHistoryEntry | PublicValidatorHistoryEntry> }) {
  return (
    <section className={`validator-insight${compact ? ' validator-insight-compact' : ''}`} aria-label="Validator insight">
      <p><span className={`status status-${insight.state}`}>{stateLabel(insight.state)}</span> · {insight.freshness === 'unknown' ? 'Never observed' : insight.freshness}</p>
      <dl className="detail-list">
        <div><dt>Validator</dt><dd>{insight.displayName || insight.validatorNodeId}</dd></div>
        <div><dt>Rank</dt><dd>{insight.rank ?? 'Unknown'}</dd></div>
        <div><dt>Stake</dt><dd>{insight.stakeAmount ?? 'Unknown'}</dd></div>
        <div><dt>Reward rate</dt><dd>{insight.rewardRate ?? 'Unknown'}</dd></div>
        <div><dt>Blocks</dt><dd>{insight.blockCount ?? 'Unknown'}</dd></div>
      </dl>
      {insight.counterState === 'counter_reset' && <p className="form-error" role="status">Counter reset or correction observed; prior value was not treated as normal growth.</p>}
      {history && <ValidatorHistory entries={history} compact={compact} />}
    </section>
  )
}
