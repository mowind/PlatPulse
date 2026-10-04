/**
 * Owner Validator registry presentation (issue #218, design §15.4).
 *
 * Presentation only: it turns the Server's evidence into the entries, the
 * statements, and the ages the Owner surface owes the Operator. Four rules it
 * never breaks. Automatic identity is a chain identity, so it is never an
 * ownership claim, a manual role, or consensus membership. An unresolved
 * identification state (a key still to be verified, a missing or contradicted
 * key) is Unknown, never absence, so a Node the Server could not resolve is
 * never shown as "no Validator". A Provider failure keeps the last-good verdict
 * and is marked retained/stale; it never becomes Not a Validator. And a verdict
 * without a last-good instant shows no age instead of a zero.
 */
import type { AdminNodeValidatorIdentity, AdminValidatorInsight, Validator } from './api/generated'
import {
  currentValidatorStatusLabel,
  currentValidatorStatusQualifierLabel,
} from './components/LinkedValidator'
import { validatorActivityLabel } from './components/ValidatorActivityBadge'
import { formatIdentifier } from './formatBytes'
import { formatDuration } from './formatDuration'

export type ValidatorTone = 'ok' | 'warning' | 'error' | 'neutral'

/**
 * Discovery states the Server records for a Node's automatic Validator
 * identity. `identified` is the only state that establishes an identity; every
 * other state says what the evaluation could not establish, and none of them is
 * evidence that the Node does not stake.
 */
const IDENTITY_PRESENTATION: Record<string, { label: string; tone: ValidatorTone }> = {
  identified: { label: 'Identified', tone: 'ok' },
  not_evaluated: { label: 'Not evaluated', tone: 'neutral' },
  missing_public_key: { label: 'P2P public key missing', tone: 'warning' },
  invalid_public_key: { label: 'P2P public key invalid', tone: 'error' },
  network_identity_missing: { label: 'Network identity missing', tone: 'warning' },
  network_identity_mismatch: { label: 'Network identity mismatch', tone: 'error' },
}

const UNKNOWN_IDENTITY = { label: 'Unknown', tone: 'neutral' as ValidatorTone }

/** Short label and tone for one discovery state. An unlisted state stays
 *  Unknown rather than being forced into `identified` or into an absence. */
export function validatorIdentityPresentation(state: string | null | undefined): {
  label: string
  tone: ValidatorTone
} {
  const key = typeof state === 'string' ? state.trim().toLowerCase() : ''
  return IDENTITY_PRESENTATION[key] ?? UNKNOWN_IDENTITY
}

/** Whether the Server established an automatic identity for this Node. */
export function validatorIdentityIsResolved(
  identity: AdminNodeValidatorIdentity | null | undefined,
): boolean {
  return validatorIdentityPresentation(identity?.state).label === 'Identified'
}

export type PublicAssociation = {
  label: string
  tone: ValidatorTone
  description: string
}

/**
 * What the Public projection does with the automatic Link this Node holds. The
 * Server answers that in `associationEffective`: Public shows the association
 * only for an Active Node with an open interval. The discovery state never
 * overrides the Server's own answer, so an interval that is still open keeps
 * being reported as open while the current evidence no longer identifies a key,
 * and a Node with no open interval has nothing to project.
 */
export function validatorPublicAssociation(
  identity: AdminNodeValidatorIdentity | null | undefined,
): PublicAssociation | null {
  if (!identity?.validatorId) return null
  return identity.associationEffective
    ? { label: 'Shown', tone: 'ok', description: 'Active Node with an open Link' }
    : { label: 'Not shown', tone: 'warning', description: 'Open Link, Node not Active' }
}

export type IdentityCoverage = {
  total: number
  evaluated: number
  identified: number
  unresolved: number
}

/** Coverage counts for the Owner summary. Only a Node the Server actually
 *  evaluated carries a status; a Node whose state is still the DTO default
 *  `not_evaluated` was never examined, so it is counted as a known Node rather
 *  than as evaluated work, and an evaluation-free Server never reads `Resolved`.
 *  A search never changes these counts. */
export function validatorIdentityCoverage(
  identities: readonly AdminNodeValidatorIdentity[],
): IdentityCoverage {
  const evaluated = identities.filter((identity) => identity.state !== 'not_evaluated')
  const identified = evaluated.filter((identity) => validatorIdentityIsResolved(identity)).length
  return {
    total: identities.length,
    evaluated: evaluated.length,
    identified,
    unresolved: evaluated.length - identified,
  }
}

/**
 * The statement one coverage row owes the Operator: the Server's own sanitized
 * reason where it sent one, and a fixed sentence per state otherwise. The
 * resolved state explains what the automatic Link means for the Public
 * projection instead, because an open Link and a projected association are not
 * the same thing (Public also requires an Active Node).
 */
export function validatorIdentityNotice(identity: AdminNodeValidatorIdentity): {
  label: string
  tone: ValidatorTone
  description: string
} {
  const { label, tone } = validatorIdentityPresentation(identity.state)
  const reason = identity.reason?.trim()
  if (reason) return { label, tone, description: reason }
  if (validatorIdentityIsResolved(identity)) {
    return {
      label,
      tone,
      description: identity.associationEffective
        ? 'An automatic Link is open and the Public projection shows this association.'
        : 'An automatic Link is open; the Public projection shows no association because the Node is not Active.',
    }
  }
  if (identity.validatorId) {
    return {
      label,
      tone,
      description: identity.associationEffective
        ? 'No automatic Validator identity is established from the current evidence; the association identified earlier is still open and the Public projection still shows it.'
        : 'No automatic Validator identity is established from the current evidence; the association identified earlier is still open, but Public shows no association because the Node is not Active.',
    }
  }
  return {
    label,
    tone,
    description: 'No automatic Validator identity has been established for this Node.',
  }
}

/** Only an evaluated Node whose identity was not established needs attention. */
export function validatorIdentityNeedsAttention(
  identity: AdminNodeValidatorIdentity | null | undefined,
): boolean {
  if (!identity) return false
  return !validatorIdentityIsResolved(identity)
}

/** How one coverage row names its Node: its own display name where it has one,
 *  otherwise the identifier the Operator can search for. */
export function validatorIdentityNodeLabel(identity: AdminNodeValidatorIdentity): string {
  const name = identity.nodeDisplayName?.trim()
  return name && name.length > 0 ? name : identity.nodeId
}

/**
 * The Current Validator Status sentence (#173): the verdict plus the explicit
 * special state of a confirmed-valid identity. `unknown` keeps its own wording
 * and never reads as a negative conclusion.
 */
export function validatorStatusLabel(insight: AdminValidatorInsight | null | undefined): string {
  const verdict = currentValidatorStatusLabel(insight?.currentValidatorStatus)
  const qualifier = currentValidatorStatusQualifierLabel(
    insight?.currentValidatorStatusQualifier ?? null,
  )
  return qualifier ? `${verdict} · ${qualifier}` : verdict
}

/** Currency of the verdict. A retained verdict is a warning (the last-good
 *  answer stands while the source cannot be read), an unestablished one is
 *  neutral because it is not a negative state. */
export function validatorStatusTone(
  insight: AdminValidatorInsight | null | undefined,
): ValidatorTone {
  const state = (insight?.currentValidatorStatusState ?? '').trim().toLowerCase()
  if (state === 'stale') return 'warning'
  if (state === 'current') {
    return (insight?.currentValidatorStatus ?? '').trim().toLowerCase() === 'validator'
      ? 'ok'
      : 'neutral'
  }
  return 'neutral'
}

/** How to read the verdict's currency, in the Operator's words. */
export function validatorStatusEvidence(insight: AdminValidatorInsight | null | undefined): string {
  switch ((insight?.currentValidatorStatusState ?? '').trim().toLowerCase()) {
    case 'current':
      return 'Current'
    case 'stale':
      return 'Retained (stale)'
    default:
      return 'Not established'
  }
}

/**
 * The sanitized note behind the verdict where it needs one: a retained verdict
 * must say it is retained, and an unestablished one must say it is not a
 * negative conclusion (ADR 0005).
 */
export function validatorStatusNote(
  insight: AdminValidatorInsight | null | undefined,
): string | null {
  switch ((insight?.currentValidatorStatusState ?? '').trim().toLowerCase()) {
    case 'stale':
      // Two different reasons for the same currency: a refresh that failed, and
      // a successful observation that has simply aged past the window.
      return (insight?.outcome ?? '').trim().toLowerCase() === 'error'
        ? 'The last successful verdict is retained; the latest refresh failed.'
        : 'The last successful verdict is retained; the observation behind it is older than the freshness window.'
    case 'current':
      return null
    default:
      return 'Current staking validity is not established; this is not a negative conclusion.'
  }
}

/** Canonical Activity label, shared with the Public badge so both surfaces
 *  name the same Provider value the same way (#173). */
export function validatorActivity(insight: AdminValidatorInsight | null | undefined): string {
  return validatorActivityLabel(insight?.activity)
}

/** Currency of the Activity value, beside its name. */
export function validatorActivityEvidence(
  insight: AdminValidatorInsight | null | undefined,
): string {
  switch ((insight?.activityState ?? '').trim().toLowerCase()) {
    case 'current':
      return 'Current'
    case 'stale':
      return 'Retained (stale)'
    default:
      return 'Not established'
  }
}

/**
 * Age of the last-good observation. A Validator that never refreshed
 * successfully has no age at all, so it reads Unknown instead of zero.
 */
export function validatorLastGoodAge(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return 'Unknown'
  return formatDuration(seconds * 1000)
}

/**
 * The Provider that produced the evidence. `disabled` is the Server's
 * canonical marker for a deployment without a configured Validator Provider,
 * so it reads as Not configured instead of as a Provider named "disabled".
 */
export function validatorSourceLabel(source: string | null | undefined): string {
  const value = source?.trim()
  if (!value || value === 'disabled') return 'Not configured'
  return value
}

/**
 * Counter state of a provider answer. Only a read that succeeded observed a
 * counter: the Server synthesizes its `normal` default both for a deployment it
 * never read and on a first failure with no good answer, so that value is never
 * printed as an observation. When the latest refresh failed but an earlier good
 * answer exists, the value the Server retains is printed as retained evidence
 * rather than as a current one.
 */
export function validatorCounterStateLabel(
  insight: AdminValidatorInsight | null | undefined,
): string {
  if (!insight) return 'Not observed'
  const state = (insight.counterState ?? '').trim()
  if (!state) return 'Not observed'
  const outcome = (insight.outcome ?? '').trim().toLowerCase()
  if (outcome === 'success' || outcome === 'empty') return state
  if (
    insight.lastGoodReceivedAt &&
    outcome !== 'not_configured' &&
    outcome !== 'unsupported' &&
    outcome !== 'not_found'
  ) {
    return state + ' (retained)'
  }
  return 'Not observed'
}

/** Live-staking rank: a rank is never rendered as zero, and no rank is
 *  Unknown rather than a fabricated position. */
export function validatorRank(insight: AdminValidatorInsight | null | undefined): string {
  return insight?.rank == null ? 'Unknown' : `#${insight.rank}`
}

/** Short form of a chain identity key for tables; the full value stays in the
 *  DOM and in the title attribute of the rendered element. */
export function validatorKeyLabel(key: string | null | undefined): string {
  const value = key?.trim()
  return value && value.length > 0 ? formatIdentifier(value) : 'Unknown'
}

/** How one Validator names itself: its Owner-set display name, otherwise its
 *  chain identity key, which is the identity the discovery dimension resolves. */
export function validatorDisplayName(validator: Validator): string {
  const name = validator.displayName?.trim()
  if (name && name.length > 0) return name
  return validatorKeyLabel(validator.validatorNodeId)
}

/** Stable Owner ordering: per Network, by display name, then by identity. */
export function sortValidators(validators: readonly Validator[]): Validator[] {
  return [...validators].sort((left, right) => {
    if (left.networkKey !== right.networkKey) return left.networkKey < right.networkKey ? -1 : 1
    const leftName = validatorDisplayName(left)
    const rightName = validatorDisplayName(right)
    if (leftName !== rightName) return leftName < rightName ? -1 : 1
    return left.validatorId < right.validatorId ? -1 : 1
  })
}

/** Free-text match over the fields an Operator can search by: the display name,
 *  the identity key, and the identifier. */
export function validatorMatchesQuery(validator: Validator, query: string): boolean {
  const needle = query.trim().toLowerCase()
  if (needle.length === 0) return true
  return [
    validatorDisplayName(validator),
    validator.validatorNodeId,
    validator.validatorId,
    validator.displayName ?? '',
  ]
    .join(' ')
    .toLowerCase()
    .includes(needle)
}

/** Free-text match over a coverage row: Node name, Node id, and the identity
 *  key the evaluation observed. */
export function validatorIdentityMatchesQuery(
  identity: AdminNodeValidatorIdentity,
  query: string,
): boolean {
  const needle = query.trim().toLowerCase()
  if (needle.length === 0) return true
  return [
    validatorIdentityNodeLabel(identity),
    identity.nodeId,
    identity.observedValidatorNodeKey ?? '',
    identity.validatorNodeKey ?? '',
    identity.networkKey,
  ]
    .join(' ')
    .toLowerCase()
    .includes(needle)
}
