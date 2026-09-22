import { Activity, Box, Eye, ShieldCheck, type LucideIcon } from 'lucide-react'

import type { PublicValidatorInsight } from '../api/generated'
import { cn } from '../lib/utils'
import { formatObservedAt } from './StatusBadge'
import { DataTooltip } from './ui/data-tooltip'

/**
 * The PlatScan Validator Activity Home presents in a Node card's top-right
 * corner. `activity` is the Server-owned canonical projection of PlatScan's
 * numeric `data.status`: 1|2 active, 3 producing, 4 exiting, 5 exited,
 * 6 verifying, 7 locked, plus the Public-only `observing` label for an
 * authoritative empty identity and `unknown` for unavailable evidence.
 * Home only renders this value: it never infers a status from Node Health,
 * consensus membership, rank, rewards, or the local Node role.
 */
type ActivityTone = 'verifying' | 'producing' | 'active' | 'muted'

type ActivityPresentation = {
  /** The full status name: always the accessible and explained value. */
  label: string
  icon: LucideIcon
  tone: ActivityTone
}

const PRESENTATION: Record<string, ActivityPresentation> = {
  verifying: { label: 'Verifying', icon: ShieldCheck, tone: 'verifying' },
  producing: { label: 'Producing', icon: Box, tone: 'producing' },
  active: { label: 'Active', icon: Activity, tone: 'active' },
  observing: { label: 'Observing', icon: Eye, tone: 'muted' },
  exiting: { label: 'Exiting', icon: Eye, tone: 'muted' },
  exited: { label: 'Exited', icon: Eye, tone: 'muted' },
  locked: { label: 'Locked', icon: Eye, tone: 'muted' },
}

/**
 * Every label Home can render, each measured through an `::after` pseudo-element
 * in the same grid cell. The widest one holds the badge width steady, so
 * switching status never re-flows the Node name. The measurement text lives in
 * generated content rather than a text node: it still sizes the cell, but it
 * cannot be picked up by text queries or exposed to assistive technology.
 */
const SIZERS = [
  { label: 'Verifying', className: "after:content-['Verifying']" },
  { label: 'Producing', className: "after:content-['Producing']" },
  { label: 'Observing', className: "after:content-['Observing']" },
  { label: 'Exiting', className: "after:content-['Exiting']" },
  { label: 'Active', className: "after:content-['Active']" },
  { label: 'Exited', className: "after:content-['Exited']" },
  { label: 'Locked', className: "after:content-['Locked']" },
]

/** Observing and every out-of-scheme value share the neutral treatment: the
 *  status name carries the meaning and colour only supplements it. */
const NEUTRAL: ActivityPresentation = { label: 'Observing', icon: Eye, tone: 'muted' }

const TONE_CLASS: Record<ActivityTone, string> = {
  verifying:
    'border-teal-600/20 bg-teal-600/5 text-teal-600 dark:border-teal-400/25 dark:bg-teal-400/10 dark:text-teal-400',
  producing:
    'border-emerald-600/20 bg-emerald-600/5 text-emerald-600 dark:border-emerald-400/25 dark:bg-emerald-400/10 dark:text-emerald-400',
  active:
    'border-emerald-600/15 bg-emerald-600/5 text-emerald-600/70 dark:border-emerald-400/20 dark:bg-emerald-400/10 dark:text-emerald-400/70',
  muted: 'border-border/60 bg-muted/40 text-muted-foreground',
}

function capitalize(value: string): string {
  return value.charAt(0).toUpperCase() + value.slice(1)
}

/**
 * Resolve the canonical Activity token to its presentation. An unlisted token
 * keeps its own name with the neutral treatment rather than being forced into
 * one of the four named states; the full value stays reachable in the
 * explanation.
 */
function resolveActivity(
  validator: PublicValidatorInsight | null | undefined,
): { presentation: ActivityPresentation; value: string } {
  const raw =
    typeof validator?.activity === 'string' ? validator.activity.trim().toLowerCase() : ''
  if (raw && PRESENTATION[raw]) return { presentation: PRESENTATION[raw], value: raw }
  if (raw === 'unknown') return { presentation: NEUTRAL, value: 'unknown' }
  if (!raw) return { presentation: NEUTRAL, value: 'observing' }
  return { presentation: { label: capitalize(raw), icon: Eye, tone: 'muted' }, value: raw }
}

/** The named Provider behind the value; only a non-PlatScan source is surfaced. */
function activitySource(validator: PublicValidatorInsight | null | undefined): string {
  const source = validator?.source?.trim()
  if (!source || source === 'disabled' || source === 'platscan') return 'PlatScan'
  return source
}

/**
 * Why the badge shows what it shows. Missing and unavailable evidence is never
 * silent: the explanation always names the actual reason while the visible
 * value stays the unified Observing state.
 */
function activityReason(
  validator: PublicValidatorInsight | null | undefined,
  value: string,
  identityReason: string | null | undefined,
): string | null {
  if (!validator) {
    const identity = identityReason?.trim()
    return identity || 'No effective Node Validator Link.'
  }
  if (value === 'observing') return 'PlatScan reports no staking identity for this Validator.'
  if (validator.state === 'error') {
    return validator.receivedAt
      ? 'Showing the last successful value; the latest refresh failed.'
      : 'The latest refresh failed and no successful value is available.'
  }
  if (validator.activityState === 'stale') {
    return 'Showing the last successful value; it is no longer current.'
  }
  switch (validator.state) {
    case 'fresh':
      return null
    case 'not_found':
      return 'PlatScan returned 404; treated as unknown.'
    case 'unsupported':
      return "This Network's PlatScan deployment is unsupported."
    case 'not_configured':
      return 'No PlatScan deployment is bound to this Network.'
    default:
      return 'No successful PlatScan observation is available.'
  }
}

/**
 * A compact PlatScan Validator Activity badge for the Node card header. It
 * replaces the former local Node-role chip: the local role stays in Node
 * detail, and the Node Health marker beside the name stays an independent
 * dimension. The badge is a non-interactive status with a keyboard-focusable
 * explanation; the SVG is decorative so the accessible name is not doubled.
 */
export function ValidatorActivityBadge({
  validator,
  identityReason,
}: {
  validator: PublicValidatorInsight | null | undefined
  identityReason?: string | null
}) {
  const { presentation, value } = resolveActivity(validator)
  const stale = validator?.activityState === 'stale'
  const Icon = presentation.icon
  const updated = validator?.receivedAt
    ? `updated ${formatObservedAt(validator.receivedAt)}`
    : 'never observed'
  const reason = activityReason(validator, value, identityReason)
  const accessibleName = [
    `PlatScan status: ${presentation.label}`,
    updated,
    stale ? 'stale' : null,
    reason,
  ]
    .filter(Boolean)
    .join(', ')

  return (
    <DataTooltip
      placement="bottom"
      width={224}
      contentClassName="leading-snug"
      className="z-10 ml-auto min-w-0 max-w-full"
      contentNode={
        <>
          <span className="block">Source: {activitySource(validator)}</span>
          <span className="block">Status: {presentation.label}</span>
          <span className="block">Updated: {formatObservedAt(validator?.receivedAt)}</span>
          {reason && <span className="block">{reason}</span>}
        </>
      }
    >
      <span
        data-slot="validator-activity"
        data-activity={value}
        data-tone={presentation.tone}
        data-stale={stale ? 'true' : 'false'}
        tabIndex={0}
        aria-label={accessibleName}
        className={cn(
          'inline-flex min-w-0 max-w-full items-center gap-1 rounded border px-1.5 py-0.5 text-[11px] leading-4 outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50',
          TONE_CLASS[presentation.tone],
        )}
      >
        <span className="relative inline-flex shrink-0" aria-hidden="true">
          <Icon size={14} strokeWidth={2} aria-hidden="true" focusable="false" />
          {stale && (
            <span
              data-slot="validator-activity-stale-mark"
              className="absolute -top-1 -right-1 size-1 rounded-full bg-muted-foreground/70"
            />
          )}
        </span>
        <span className="relative grid min-w-0 text-left">
          {SIZERS.map(({ label: candidate, className }) => (
            <span
              key={candidate}
              data-slot="validator-activity-sizer"
              data-sizer={candidate}
              aria-hidden="true"
              className={cn('invisible col-start-1 row-start-1 whitespace-nowrap', className)}
            />
          ))}
          <span
            data-slot="validator-activity-label"
            className="absolute inset-y-0 left-0 right-0 truncate"
          >
            {presentation.label}
          </span>
        </span>
      </span>
    </DataTooltip>
  )
}
