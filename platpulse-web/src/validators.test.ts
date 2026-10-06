import { describe, expect, it } from 'vitest'
import type { AdminNodeValidatorIdentity, AdminValidatorInsight, Validator } from './api/generated'
import {
  sortValidators,
  validatorActivity,
  validatorDisplayName,
  validatorIdentityCoverage,
  validatorIdentityMatchesQuery,
  validatorIdentityNeedsAttention,
  validatorIdentityNodeLabel,
  validatorIdentityNotice,
  validatorCounterStateLabel,
  validatorIdentityPresentation,
  validatorIdentityIsResolved,
  validatorKeyLabel,
  validatorLastGoodAge,
  validatorMatchesQuery,
  validatorPublicAssociation,
  validatorRank,
  validatorRankEvidence,
  validatorRankOutcome,
  validatorSourceLabel,
  validatorStatusEvidence,
  validatorStatusLabel,
  validatorStatusNote,
  validatorStatusTone,
} from './validators'

function insight(overrides: Partial<AdminValidatorInsight> = {}): AdminValidatorInsight {
  return {
    activityState: 'current',
    counterState: 'ok',
    currentValidatorStatus: 'validator',
    currentValidatorStatusState: 'current',
    freshness: 'fresh',
    outcome: 'success',
    rankState: 'unknown',
    rankFreshness: 'unknown',
    rankOutcome: 'unknown',
    state: 'fresh',
    validatorNodeId: '0xkey',
    ...overrides,
  }
}

function identity(
  overrides: Partial<AdminNodeValidatorIdentity> = {},
): AdminNodeValidatorIdentity {
  return {
    associationEffective: true,
    lifecycle: 'active',
    networkKey: 'platon-mainnet',
    nodeId: 'node-1',
    state: 'identified',
    ...overrides,
  }
}

function validator(overrides: Partial<Validator> = {}): Validator {
  return {
    createdAt: '2026-08-12T09:00:00Z',
    linkCount: 1,
    networkKey: 'platon-mainnet',
    updatedAt: '2026-08-12T09:00:00Z',
    validatorId: 'v-1',
    validatorNodeId: '0x000000000000000000000000000000000000000000000000000000000000000a',
    ...overrides,
  }
}

describe('automatic identity coverage', () => {
  it('names every canonical discovery state and keeps unlisted states unknown', () => {
    expect(validatorIdentityPresentation('identified')).toEqual({ label: 'Identified', tone: 'ok' })
    expect(validatorIdentityPresentation('not_evaluated').label).toBe('Not evaluated')
    expect(validatorIdentityPresentation('missing_public_key')).toEqual({
      label: 'P2P public key missing',
      tone: 'warning',
    })
    expect(validatorIdentityPresentation('invalid_public_key').tone).toBe('error')
    expect(validatorIdentityPresentation('network_identity_missing').tone).toBe('warning')
    expect(validatorIdentityPresentation('network_identity_mismatch').tone).toBe('error')
    // Case and padding come from the wire, not from the Server's vocabulary.
    expect(validatorIdentityPresentation(' Identified ').label).toBe('Identified')
    expect(validatorIdentityPresentation('something-new')).toEqual({
      label: 'Unknown',
      tone: 'neutral',
    })
    expect(validatorIdentityPresentation(null)).toEqual({ label: 'Unknown', tone: 'neutral' })
  })

  it('resolves an identity only for the identified state', () => {
    expect(validatorIdentityIsResolved(identity())).toBe(true)
    for (const state of [
      'not_evaluated',
      'missing_public_key',
      'invalid_public_key',
      'network_identity_missing',
      'network_identity_mismatch',
      'unknown',
    ]) {
      expect(validatorIdentityIsResolved(identity({ state }))).toBe(false)
    }
    expect(validatorIdentityIsResolved(null)).toBe(false)
  })

  it('counts every evaluated Node so an unresolved identity is never absent', () => {
    expect(
      validatorIdentityCoverage([
        identity(),
        identity({ nodeId: 'node-2', state: 'missing_public_key' }),
        identity({ nodeId: 'node-3', state: 'network_identity_mismatch' }),
      ]),
    ).toEqual({ total: 3, evaluated: 3, identified: 1, unresolved: 2 })
    // A Node the Server never evaluated is a known Node, not evaluated work, so
    // a Server with nothing evaluated is never summarized as Resolved.
    expect(
      validatorIdentityCoverage([
        identity({ state: 'not_evaluated' }),
        identity({ nodeId: 'node-2' }),
      ]),
    ).toEqual({ total: 2, evaluated: 1, identified: 1, unresolved: 0 })
    expect(validatorIdentityCoverage([])).toEqual({
      total: 0,
      evaluated: 0,
      identified: 0,
      unresolved: 0,
    })
  })

  it('prefers the Server reason and explains a resolved state by its Link', () => {
    expect(
      validatorIdentityNotice(
        identity({ state: 'missing_public_key', reason: 'No P2P public key was observed.' }),
      ),
    ).toEqual({ label: 'P2P public key missing', tone: 'warning', description: 'No P2P public key was observed.' })
    expect(
      validatorIdentityNotice(identity({ associationEffective: true })).description,
    ).toContain('the Public projection shows this association')
    expect(
      validatorIdentityNotice(identity({ associationEffective: false })).description,
    ).toContain('the Node is not Active')
    expect(validatorIdentityNotice(identity({ state: 'not_evaluated' })).description).toBe(
      'No automatic Validator identity has been established for this Node.',
    )
  })

  it('keeps a retained interval visible while the current evidence identifies no key', () => {
    expect(validatorPublicAssociation(identity())).toBeNull()
    expect(validatorPublicAssociation(null)).toBeNull()
    const retained = identity({
      state: 'missing_public_key',
      validatorId: 'v-1',
      validatorNodeKey: '0xkey',
    })
    expect(validatorPublicAssociation(retained)).toEqual({
      label: 'Shown',
      tone: 'ok',
      description: 'Active Node with an open Link',
    })
    // The Server keeps the interval open for an evidence gap, so the surfaces
    // report the retained association instead of claiming there is no Link.
    expect(validatorIdentityNotice(retained).description).toContain(
      'the association identified earlier is still open',
    )
    expect(
      validatorIdentityNotice(
        identity({ state: 'unknown', validatorId: 'v-1', associationEffective: false }),
      ).description,
    ).toContain('Public shows no association because the Node is not Active')
    expect(
      validatorPublicAssociation(
        identity({ lifecycle: 'retired', validatorId: 'v-1', associationEffective: false }),
      ),
    ).toEqual({ label: 'Not shown', tone: 'warning', description: 'Open Link, Node not Active' })
  })

  it('asks for attention only where an evaluation did not establish an identity', () => {
    expect(validatorIdentityNeedsAttention(identity())).toBe(false)
    expect(validatorIdentityNeedsAttention(identity({ state: 'not_evaluated' }))).toBe(true)
    expect(validatorIdentityNeedsAttention(null)).toBe(false)
  })

  it('names a coverage row by its display name or its identifier', () => {
    expect(validatorIdentityNodeLabel(identity({ nodeDisplayName: 'ap-1' }))).toBe('ap-1')
    expect(validatorIdentityNodeLabel(identity({ nodeDisplayName: '  ' }))).toBe('node-1')
    expect(validatorIdentityNodeLabel(identity())).toBe('node-1')
  })

  it('searches a coverage row by Node and identity key', () => {
    const row = identity({
      observedValidatorNodeKey: '0xffff',
      validatorNodeKey: '0xaaaa',
      nodeDisplayName: 'ap-1',
    })
    expect(validatorIdentityMatchesQuery(row, '')).toBe(true)
    expect(validatorIdentityMatchesQuery(row, 'AP-1')).toBe(true)
    expect(validatorIdentityMatchesQuery(row, '0xaaaa')).toBe(true)
    expect(validatorIdentityMatchesQuery(row, 'node-1')).toBe(true)
    expect(validatorIdentityMatchesQuery(row, 'platon')).toBe(true)
    expect(validatorIdentityMatchesQuery(row, 'absent')).toBe(false)
  })
})

describe('current validator status', () => {
  it('states the verdict with its explicit qualifier', () => {
    expect(validatorStatusLabel(insight())).toBe('Validator')
    expect(validatorStatusLabel(insight({ currentValidatorStatusQualifier: 'locked' }))).toBe(
      'Validator · Locked',
    )
    expect(validatorStatusLabel(insight({ currentValidatorStatusQualifier: 'exiting' }))).toBe(
      'Validator · Exiting',
    )
    expect(validatorStatusLabel(insight({ currentValidatorStatus: 'not_validator' }))).toBe(
      'Not a Validator',
    )
    expect(validatorStatusLabel(insight({ currentValidatorStatus: 'unknown' }))).toBe(
      'Validator status unknown',
    )
    expect(validatorStatusLabel(null)).toBe('Validator status unknown')
  })

  it('never gives an unestablished verdict the weight of a decided one', () => {
    expect(validatorStatusTone(insight())).toBe('ok')
    expect(validatorStatusTone(insight({ currentValidatorStatus: 'not_validator' }))).toBe('neutral')
    expect(
      validatorStatusTone(
        insight({ currentValidatorStatus: 'unknown', currentValidatorStatusState: 'unknown' }),
      ),
    ).toBe('neutral')
    expect(validatorStatusTone(insight({ currentValidatorStatusState: 'stale' }))).toBe('warning')
  })

  it('keeps a retained verdict and an unestablished one clearly labelled', () => {
    expect(validatorStatusEvidence(insight())).toBe('Current')
    expect(validatorStatusEvidence(insight({ currentValidatorStatusState: 'stale' }))).toBe(
      'Retained (stale)',
    )
    expect(validatorStatusEvidence(insight({ currentValidatorStatusState: 'unknown' }))).toBe(
      'Not established',
    )
    expect(validatorStatusNote(insight())).toBeNull()
    expect(validatorStatusNote(insight({ currentValidatorStatusState: 'stale' }))).toContain(
      'retained',
    )
    expect(validatorStatusNote(insight({ currentValidatorStatusState: 'unknown' }))).toContain(
      'not a negative conclusion',
    )
  })

  it('separates a failed refresh from an observation that only aged', () => {
    expect(
      validatorStatusNote(insight({ currentValidatorStatusState: 'stale', outcome: 'error' })),
    ).toBe('The last successful verdict is retained; the latest refresh failed.')
    expect(
      validatorStatusNote(insight({ currentValidatorStatusState: 'stale', outcome: 'success' })),
    ).toBe(
      'The last successful verdict is retained; the observation behind it is older than the freshness window.',
    )
  })
})

describe('provider evidence', () => {
  it('names the canonical Activity with the shared Public vocabulary', () => {
    expect(validatorActivity(insight({ activity: 'producing' }))).toBe('Producing')
    expect(validatorActivity(insight({ activity: 'exited' }))).toBe('Exited')
    // Missing and unavailable evidence stay Observing instead of inventing a
    // status; the surrounding note carries the reason.
    expect(validatorActivity(insight({ activity: null }))).toBe('Observing')
    expect(validatorActivity(insight({ activity: 'unknown' }))).toBe('Observing')
    expect(validatorActivity(insight({ activity: 'brand-new' }))).toBe('Brand-new')
  })

  it('shows no age where no last-good instant exists', () => {
    expect(validatorLastGoodAge(null)).toBe('Unknown')
    expect(validatorLastGoodAge(undefined)).toBe('Unknown')
    expect(validatorLastGoodAge(-1)).toBe('Unknown')
    expect(validatorLastGoodAge(0)).toBe('0s')
    expect(validatorLastGoodAge(90)).toBe('1m')
    expect(validatorLastGoodAge(3 * 3600 + 120)).toBe('3h 2m')
  })

  it('never renders a missing rank as zero', () => {
    expect(validatorRank(insight({ rank: 7 }))).toBe('#7')
    expect(validatorRank(insight({ rank: null, rankState: 'unranked', rankOutcome: 'success' }))).toBe('Unranked')
    expect(validatorRank(insight({ rank: null, rankState: 'error', rankOutcome: 'error' }))).toBe('Unknown')
    expect(validatorRank(insight({ rank: null, rankState: 'unranked', rankOutcome: 'error' }))).toBe('Unknown')
    expect(validatorRankEvidence(insight({ freshness: 'fresh', rankFreshness: 'stale' }))).toBe('Retained rank (stale)')
    expect(validatorRankEvidence(insight({ freshness: 'stale', rankFreshness: 'fresh' }))).toBe('Fresh rank')
    expect(validatorRankEvidence(null)).toBe('Rank not established')
    expect(validatorRankOutcome(insight({ rankOutcome: 'error' }))).toBe('Rank refresh error')
    expect(validatorRankOutcome(insight({ rankOutcome: 'not_configured' }))).toBe('Rank Provider not configured')
    expect(validatorRankOutcome(insight({ rankOutcome: 'unsupported' }))).toBe('Rank unsupported')
    expect(validatorRankOutcome(null)).toBe('Rank not observed')
    expect(validatorRank(insight({ rank: null }))).toBe('Unknown')
    expect(validatorRank(null)).toBe('Unknown')
  })

  it('never prints a counter state for a Provider the Server never read', () => {
    expect(validatorCounterStateLabel(insight({ counterState: 'normal' }))).toBe('normal')
    expect(validatorCounterStateLabel(insight({ counterState: 'counter_reset' }))).toBe(
      'counter_reset',
    )
    expect(
      validatorCounterStateLabel(insight({ outcome: 'not_configured', counterState: 'normal' })),
    ).toBe('Not observed')
    expect(
      validatorCounterStateLabel(insight({ outcome: 'unsupported', counterState: 'normal' })),
    ).toBe('Not observed')
    // A first failure stores the same synthesized default with no good answer
    // behind it, so it is still not an observation.
    expect(
      validatorCounterStateLabel(insight({ outcome: 'error', counterState: 'normal' })),
    ).toBe('Not observed')
    expect(
      validatorCounterStateLabel(
        insight({
          outcome: 'error',
          counterState: 'normal',
          lastGoodReceivedAt: '2026-03-01T00:00:00Z',
        }),
      ),
    ).toBe('normal (retained)')
    expect(
      validatorCounterStateLabel(
        insight({ outcome: 'error', counterState: 'counter_reset', lastGoodReceivedAt: null }),
      ),
    ).toBe('Not observed')
    expect(validatorCounterStateLabel(null)).toBe('Not observed')
  })

  it('names a deployment with no Provider instead of showing its canonical marker', () => {
    expect(validatorSourceLabel('disabled')).toBe('Not configured')
    expect(validatorSourceLabel(null)).toBe('Not configured')
    expect(validatorSourceLabel(undefined)).toBe('Not configured')
    expect(validatorSourceLabel('   ')).toBe('Not configured')
    expect(validatorSourceLabel('platsScan')).toBe('platsScan')
  })

  it('shortens an identity key and says Unknown when there is none', () => {
    expect(validatorKeyLabel('0x000000000000000000000000000000000000000000000000000000000000000a')).toBe(
      '0x000000…000a',
    )
    expect(validatorKeyLabel('0xabc')).toBe('0xabc')
    expect(validatorKeyLabel(null)).toBe('Unknown')
    expect(validatorKeyLabel('   ')).toBe('Unknown')
  })
})

describe('validator list', () => {
  it('names a Validator by its display name or its chain identity', () => {
    expect(validatorDisplayName(validator({ displayName: 'Atlas' }))).toBe('Atlas')
    expect(validatorDisplayName(validator())).toBe('0x000000…000a')
  })

  it('orders per Network, by name, then by identifier', () => {
    // Networks keep the Server's own key order, so a Validator never leaves the
    // Network group it belongs to; inside a group the Owner's name decides.
    const ordered = sortValidators([
      validator({ validatorId: 'v-3', networkKey: 'platon-mainnet', displayName: 'Zeta' }),
      validator({ validatorId: 'v-2', networkKey: 'platon-devnet', displayName: 'Beta' }),
      validator({ validatorId: 'v-1', networkKey: 'platon-mainnet', displayName: 'Alpha' }),
      validator({
        validatorId: 'v-4',
        networkKey: 'platon-mainnet',
        displayName: 'Alpha',
        validatorNodeId: '0x000000000000000000000000000000000000000000000000000000000000000b',
      }),
    ])
    expect(ordered.map((entry) => entry.validatorId)).toEqual(['v-2', 'v-1', 'v-4', 'v-3'])
  })

  it('searches by display name, identity key, and identifier', () => {
    const entry = validator({ displayName: 'Atlas' })
    expect(validatorMatchesQuery(entry, '')).toBe(true)
    expect(validatorMatchesQuery(entry, 'atlas')).toBe(true)
    expect(validatorMatchesQuery(entry, 'v-1')).toBe(true)
    expect(validatorMatchesQuery(entry, '000000000a')).toBe(true)
    expect(validatorMatchesQuery(entry, 'missing')).toBe(false)
  })
})
