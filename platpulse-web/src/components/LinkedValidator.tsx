import type { PublicNode, PublicValidatorInsight } from '../api/generated'
import { MetricRow } from './MetricRow'
import { CardX } from './ui/card-x'
import { formatUtcDateTime } from './StatusBadge'
import { SURFACE_CARD } from '../lib/surface'
import { cn } from '../lib/utils'

/** Human label for the effective Node Validator Link role. The role never
 *  changes which Validator identity the metrics describe; it only describes
 *  how this Node participates in the shared Validator (#154). */
export function validatorRoleLabel(role: string | null | undefined): string {
  switch ((role ?? '').toLowerCase()) {
    case 'primary': return 'Primary'
    case 'standby': return 'Standby'
    case 'observer': return 'Observer'
    default: return 'Role unknown'
  }
}

/**
 * Safe public label for a Provider state. The Server never sends raw provider
 * diagnostics to the Public API, so the UI must not invent or echo any: it
 * renders a fixed, sanitized explanation per canonical state (#154).
 */
export function validatorStateLabel(state: string, freshness: string): string {
  switch (state) {
    case 'fresh': return 'Current'
    case 'stale': return 'Stale'
    case 'error': return 'Collection failed'
    case 'not_configured': return 'Not configured'
    case 'unsupported': return 'Unsupported'
    case 'not_found': return 'Validator not found'
    case 'empty': return 'No live Validator'
    default: return freshness === 'unknown' ? 'Never observed' : 'Unknown'
  }
}

/** Text carries the meaning; this dot only supplements it (design §2.1). */
function stateDotClass(state: string): string {
  switch (state) {
    case 'fresh': return 'bg-emerald-500'
    case 'stale':
    case 'not_configured':
    case 'unsupported': return 'bg-amber-500'
    case 'error': return 'bg-destructive'
    default: return 'bg-muted-foreground'
  }
}

function blockCountLabel(blockCount: number | null | undefined): string {
  // A source-reported zero is a real cumulative count, never rendered as
  // Unknown; only a missing value is Unknown.
  return blockCount == null ? 'Unknown' : blockCount.toLocaleString()
}

function stateNote(validator: PublicValidatorInsight): string | null {
  switch (validator.state) {
    case 'not_configured': return 'No Validator source is configured for this Network; no value was queried.'
    case 'error': return 'The Validator source could not be read; the last successful cumulative value is retained.'
    case 'unsupported': return 'The Validator source does not support this Validator identifier.'
    case 'not_found': return 'The source has no current record for this Validator.'
    case 'empty': return 'The source reported no live Validator for this identifier.'
    default: return null
  }
}

/**
 * The extensible linked-Validator area shared by the Home Node card and Node
 * detail. It owns the cumulative Validator block count and the explicit
 * unlinked / not-configured / never-observed / stale / retained states; later
 * Validator metrics extend the same metric group rather than adding a second
 * region (#154).
 */
export function LinkedValidatorSection({ node, variant = 'card' }: { node: PublicNode; variant?: 'card' | 'detail' }) {
  const validator = node.validator
  if (!validator) {
    return <section data-slot="linked-validator" className="min-w-0 border-t border-border pt-3" aria-label="Linked Validator">
      <h3 className="m-0 text-xs font-medium tracking-wider text-muted-foreground">Linked Validator</h3>
      <p className="m-0 mt-1 text-[11px] text-muted-foreground" role="status">Unlinked — no effective Validator Link is configured for this Node.</p>
    </section>
  }

  const state = validatorStateLabel(validator.state, validator.freshness)
  const note = stateNote(validator)
  const retained = validator.state !== 'fresh' && validator.state !== 'stale' && validator.blockCount != null
  const body = <>
    <header className="flex min-w-0 flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
      <h3 className="m-0 text-xs font-medium tracking-wider text-muted-foreground">Linked Validator</h3>
      <span className="shrink-0 rounded-full border border-border px-2 py-0.5 text-[10px] font-semibold text-muted-foreground">{validatorRoleLabel(validator.linkRole)}</span>
    </header>
    <p className="m-0 mt-1 min-w-0 break-words text-sm font-semibold">{validator.displayName || validator.validatorNodeId}</p>
    <p className="m-0 mt-1 flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground" role="status">
      <span className={cn('inline-block size-1.5 shrink-0 rounded-full', stateDotClass(validator.state))} aria-hidden="true" />
      <span className="font-medium text-foreground">{state}</span>
      {validator.state === 'stale' && validator.freshness === 'stale' && <span>· last successful value retained</span>}
    </p>
    {note && <p className="m-0 mt-1 text-[11px] text-muted-foreground">{note}</p>}
    <div className="mt-2 grid grid-cols-[repeat(auto-fit,minmax(9rem,1fr))] gap-x-4" role="group" aria-label="Linked Validator metrics">
      <MetricRow label="Cumulative blocks" value={blockCountLabel(validator.blockCount)} />
    </div>
    {validator.counterState === 'counter_reset' && <p className="m-0 mt-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive" role="status">Counter reset or correction observed; the prior value was not treated as normal growth.</p>}
    {retained && <p className="m-0 mt-1 text-[11px] text-muted-foreground">Showing the last successful cumulative value; the current source state is unavailable.</p>}
    {variant === 'detail' && <Provenance validator={validator} />}
  </>

  if (variant === 'detail') {
    return <CardX
      bordered={false}
      role="region"
      aria-label="Linked Validator"
      data-slot="linked-validator"
      className={cn('min-w-0 rounded-md border-none', SURFACE_CARD)}
      contentClassName="flex min-w-0 flex-col gap-2"
    >{body}</CardX>
  }
  return <section data-slot="linked-validator" className="min-w-0 border-t border-border pt-3" aria-label="Linked Validator">{body}</section>
}

function Provenance({ validator }: { validator: PublicValidatorInsight }) {
  const fact = (label: string, value: string) => <div key={label} className="min-w-0">
    <dt className="text-[11px] font-medium tracking-wider text-muted-foreground">{label}</dt>
    <dd className="m-0 break-words text-xs">{value}</dd>
  </div>
  return <dl className="m-0 grid grid-cols-[repeat(auto-fit,minmax(9rem,1fr))] gap-x-4 gap-y-2 border-t border-dashed border-border/60 pt-2">
    {fact('Last success', validator.receivedAt ? formatUtcDateTime(validator.receivedAt) : 'Never observed')}
    {fact('Source', validator.source ?? 'Unknown')}
    {fact('Source cutoff', validator.providerTimestamp ? formatUtcDateTime(validator.providerTimestamp) : 'Not provided by the source')}
  </dl>
}
