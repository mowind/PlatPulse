import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { PublicNode, PublicValidatorInsight } from '../api/generated'
import { LinkedValidatorSection, currentValidatorStatusLabel, validatorDataStatus, validatorStateLabel } from './LinkedValidator'

const node: PublicNode = {
  nodeId: 'node-1',
  displayName: 'Node One',
  networkKey: 'mainnet',
  health: 'healthy',
  healthReason: 'RPC reachable',
  rpcState: 'connected',
  syncState: 'synced',
  consensusState: 'ready',
  processState: 'running',
  resyncState: 'idle',
  networkReferenceConfidence: 'high',
  peers: { state: 'ok', freshness: 'current', peerCount: 1 },
  consensus: { state: 'ok', freshness: 'current', validator: true },
  validator: null,
}

const insight: PublicValidatorInsight = {
  validatorId: 'validator-1',
  validatorNodeId: '0xvalidator',
  displayName: 'Validator One',
  nodeId: 'node-1',
  state: 'fresh',
  freshness: 'fresh',
  source: 'platscan',
  providerTimestamp: '2026-08-25T00:00:00Z',
  receivedAt: '2026-08-25T00:00:05Z',
  blockCount: 4321,
  expectedBlockCount: 110,
  blockRate: '90.909091',
  blockRateState: 'ok',
  genBlocksRate: '75.5',
  delegationRewardPercentage: '20',
  rewardAmount: '1234.123456789012',
  rank: 7,
  rankState: 'ranked',
  rankFreshness: 'fresh',
  rankReceivedAt: '2026-08-25T00:00:05Z',
  rankCohortSize: 300,
  counterState: 'normal',
  activity: 'producing',
  activityState: 'current',
  currentValidatorStatus: 'validator',
  currentValidatorStatusState: 'current',
  currentValidatorStatusQualifier: null,
}

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

const renderCard = (value: Partial<PublicNode> = {}) => render(<LinkedValidatorSection node={{ ...node, validator: insight, ...value }} />)
const renderDetail = (value: Partial<PublicNode> = {}) => render(<LinkedValidatorSection node={{ ...node, validator: insight, ...value }} variant="detail" />)
const detailRegion = () => screen.getByRole('region', { name: 'Linked Validator' })

/** The rendered value of a Linked Validator metric cell, in either the
 *  emphasized label-over-value form or the compact key-value form. */
const metricValue = (label: string) => {
  const labelNode = screen.getByText(label, { exact: true })
  const cell = labelNode.closest('[data-slot="validator-metric"], [data-slot="metric-row"]')
  return cell?.querySelector('strong')?.textContent ?? undefined
}

describe('LinkedValidatorSection Home card', () => {
  it('keeps the six metrics as two emphasized cells and four compact parameter rows', () => {
    renderCard()
    const group = screen.getByRole('group', { name: 'Linked Validator metrics' })
    expect(group.getAttribute('data-slot')).toBe('linked-validator-metrics')
    const emphasized = group.querySelectorAll('[data-slot="validator-metric"]')
    expect(emphasized).toHaveLength(2)
    for (const metric of emphasized) {
      expect(metric.querySelector('strong')?.className).toContain('tabular-nums')
      expect(metric.querySelector('strong')?.className).toContain('font-semibold')
      expect(metric.className).not.toMatch(/bg-|border-|break-all|overflow-wrap/)
    }
    const compact = group.querySelectorAll('[data-slot="metric-row"][data-layout="compact"]')
    expect(compact).toHaveLength(4)
    for (const row of compact) {
      expect(row.querySelector('[data-slot="metric-row-value"]')?.className).toContain('tabular-nums')
      expect(row.querySelector('[data-full-label]')?.textContent).toBeTruthy()
      expect(row.querySelector('[data-short-label]')?.textContent).toBeTruthy()
      expect(row.className).not.toMatch(/bg-|border-|break-all|overflow-wrap/)
    }
  })

  it('moves identity, the heading, the identifier, the copy control and Details off the Home card', () => {
    renderCard()
    expect(screen.queryByText('Linked Validator')).toBeNull()
    expect(screen.queryByText('Validator One')).toBeNull()
    expect(screen.queryByText('Data: Current')).toBeNull()
    expect(screen.queryByText('0xvalidator')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Copy full Validator identifier' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Open Validator details' })).toBeNull()
    expect(screen.queryByRole('dialog')).toBeNull()
    // The six metrics stay directly visible.
    expect(metricValue('Cumulative blocks')).toBe('4,321')
    expect(metricValue('Cumulative rewards')).toBe('1.23K')
    expect(metricValue('Network rank')).toBe('#7')
    expect(metricValue('Production rate')).toBe('90.91%')
    expect(metricValue('PlatScan 24h rate')).toBe('75.50%')
    expect(metricValue('Delegation reward share')).toBe('20.00%')
  })

  it('keeps a source-reported zero distinct from an unknown value', () => {
    renderCard({ validator: { ...insight, blockCount: 0 } })
    expect(metricValue('Cumulative blocks')).toBe('0')
    cleanup()
    renderCard({ validator: { ...insight, blockCount: null } })
    expect(metricValue('Cumulative blocks')).toBe('—')
  })

  it('shows gross cumulative rewards on the card and abbreviates them exactly', () => {
    renderCard({ validator: { ...insight, rewardAmount: '1234567.89' } })
    expect(metricValue('Cumulative rewards')).toBe('1.23M')
    cleanup()
    renderCard({ validator: { ...insight, rewardAmount: '0' } })
    expect(metricValue('Cumulative rewards')).toBe('0')
    cleanup()
    renderCard({ validator: { ...insight, rewardAmount: null } })
    expect(metricValue('Cumulative rewards')).toBe('—')
  })

  it('shows both production rates with their distinct source meanings', () => {
    renderCard()
    expect(metricValue('Production rate')).toBe('90.91%')
    expect(metricValue('PlatScan 24h rate')).toBe('75.50%')
  })

  it('distinguishes a zero scheduled denominator from an incomplete pair', () => {
    renderCard({ validator: { ...insight, blockRate: null, blockRateState: 'not_applicable' } })
    expect(metricValue('Production rate')).toBe('Not applicable')
    cleanup()
    renderCard({ validator: { ...insight, blockRate: null, blockRateState: 'unknown' } })
    expect(metricValue('Production rate')).toMatch(/^Unknown:/)
  })

  it('keeps a source-reported zero 24h rate distinct from a missing one', () => {
    renderCard({ validator: { ...insight, genBlocksRate: '0' } })
    expect(metricValue('PlatScan 24h rate')).toBe('0.00%')
    cleanup()
    renderCard({ validator: { ...insight, genBlocksRate: null } })
    expect(metricValue('PlatScan 24h rate')).toMatch(/^Unknown:/)
  })

  it('keeps the delegation reward share in percentage points with legitimate 0 and 100', () => {
    renderCard({ validator: { ...insight, delegationRewardPercentage: '20' } })
    expect(metricValue('Delegation reward share')).toBe('20.00%')
    cleanup()
    renderCard({ validator: { ...insight, delegationRewardPercentage: '0' } })
    expect(metricValue('Delegation reward share')).toBe('0.00%')
    cleanup()
    renderCard({ validator: { ...insight, delegationRewardPercentage: '100' } })
    expect(metricValue('Delegation reward share')).toBe('100.00%')
    cleanup()
    renderCard({ validator: { ...insight, delegationRewardPercentage: null } })
    expect(metricValue('Delegation reward share')).toMatch(/^Unknown:/)
  })

  it('shows the Network rank and keeps unranked distinct from failure or unknown', () => {
    renderCard()
    expect(metricValue('Network rank')).toBe('#7')
    cleanup()
    // Unranked is a ranking conclusion, not a value; a Validator that retains
    // other metrics keeps its six-metric structure and prints Unranked.
    renderCard({ validator: { ...insight, rank: null, rankState: 'unranked' } })
    expect(metricValue('Network rank')).toBe('Unranked')
    expect(screen.getByRole('group', { name: 'Linked Validator metrics' })).toBeTruthy()
    cleanup()
    // With no other metric and no live identity, Unranked alone must not reopen
    // six Unknown slots.
    renderCard({ validator: { ...insight, blockCount: null, rewardAmount: null, blockRate: null, blockRateState: 'unknown', genBlocksRate: null, delegationRewardPercentage: null, rank: null, rankState: 'unranked' } })
    expect(screen.queryByRole('group', { name: 'Linked Validator metrics' })).toBeNull()
    expect(screen.getByText(/Not in the Network’s complete live-staking cohort/)).toBeTruthy()
    cleanup()
    renderCard({ validator: { ...insight, rank: null, rankState: 'error' } })
    expect(metricValue('Network rank')).toMatch(/^Unknown:/)
  })

  it('retains last-good values after a source failure', () => {
    renderCard({ validator: { ...insight, state: 'error', freshness: 'stale', genBlocksRate: '0' } })
    expect(metricValue('PlatScan 24h rate')).toBe('0.00%')
    expect(metricValue('Production rate')).toBe('90.91%')
  })

  it('retains historical metrics when current staking identity is absent', () => {
    renderCard({ validator: { ...insight, currentValidatorStatus: 'not_validator', activity: 'exited' } })
    expect(metricValue('Cumulative blocks')).toBe('4,321')
    expect(metricValue('Cumulative rewards')).toBe('1.23K')
    expect(screen.getByRole('group', { name: 'Linked Validator metrics' })).toBeTruthy()
  })
})

describe('LinkedValidatorSection Home empty state', () => {
  const missing: PublicValidatorInsight = {
    ...insight, blockCount: null, rewardAmount: null, rank: null, rankState: 'unknown',
    blockRate: null, blockRateState: 'unknown', expectedBlockCount: null,
    genBlocksRate: null, delegationRewardPercentage: null,
  }
  const notValidator: PublicValidatorInsight = {
    ...missing, currentValidatorStatus: 'not_validator', currentValidatorStatusState: 'current',
  }

  it('never renders six Unknown slots when no metric is displayable', () => {
    renderCard({ validator: { ...notValidator, state: 'empty', freshness: 'fresh' } })
    expect(screen.queryByRole('group', { name: 'Linked Validator metrics' })).toBeNull()
    expect(screen.queryByText('Not a Validator')).toBeNull()
    expect(screen.queryAllByText('—')).toHaveLength(0)
  })

  it.each([
    ['loading', { state: 'loading', freshness: 'unknown' }, 'Loading Validator metrics…'],
    ['error', { state: 'error', freshness: 'stale' }, 'The Validator source could not be read; no metrics are available.'],
    ['not_configured', { state: 'not_configured', freshness: 'unknown' }, 'No Validator source is configured for this Network.'],
    ['unsupported', { state: 'unsupported', freshness: 'unknown' }, 'The Validator source does not support this Validator identifier.'],
    ['not_found', { state: 'not_found', freshness: 'unknown' }, 'The source has no current record for this Validator.'],
    ['empty', { state: 'empty', freshness: 'fresh' }, 'No validator metrics'],
  ] as const)('shows the %s state as one accurate short line', (state, override, text) => {
    renderCard({ validator: { ...notValidator, ...override } })
    const status = screen.getByRole('status')
    expect(status.textContent).toBe(text)
    // The region keeps its reserved height and stamps the real Server state.
    expect(status.parentElement?.getAttribute('data-state')).toBe(state)
    expect(screen.queryByRole('group', { name: 'Linked Validator metrics' })).toBeNull()
    expect(screen.queryAllByText('Unknown')).toHaveLength(0)
  })

  it('keeps partial metrics visible instead of folding to the empty state', () => {
    renderCard({ validator: { ...missing, rewardAmount: '42.5' } })
    expect(screen.getByRole('group', { name: 'Linked Validator metrics' })).toBeTruthy()
    expect(metricValue('Cumulative rewards')).toBe('42.5')
  })

  it('keeps the known not-applicable production rate visible', () => {
    renderCard({ validator: { ...missing, blockRateState: 'not_applicable' } })
    expect(screen.getByRole('group', { name: 'Linked Validator metrics' })).toBeTruthy()
    expect(metricValue('Production rate')).toBe('Not applicable')
  })

  it('explains a missing Link without claiming absent staking', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: null }} />)
    expect(screen.getByRole('status', { name: 'Validator identity state: not observed' }).textContent).toBe('Validator identity has not been observed yet.')
    expect(screen.queryByText('Linked Validator')).toBeNull()
    expect(screen.queryByRole('group', { name: 'Linked Validator metrics' })).toBeNull()
  })

  it('keeps a long Server identity explanation on Node detail, not on the Home card', () => {
    const longReason = 'The observed Network Identity does not match the registered Network tuple, so no automatic correspondence was established.'
    render(<LinkedValidatorSection node={{ ...node, validator: null, validatorIdentityState: 'network_identity_mismatch', validatorIdentityReason: longReason }} />)
    expect(screen.queryByText(longReason)).toBeNull()
    expect(screen.getByText('No Validator identity has been established for this Node.')).toBeTruthy()
    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: null, validatorIdentityState: 'network_identity_mismatch', validatorIdentityReason: longReason }} variant="detail" />)
    expect(screen.getByText(longReason)).toBeTruthy()
  })

  it('distinguishes identified data unavailability from unobserved discovery', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: null, validatorIdentityState: 'identified' }} />)
    expect(screen.getByRole('status', { name: 'Validator identity state: identified' }).textContent).toBe('Validator data is unavailable for the identified identity.')
  })

  it.each(['missing_public_key', 'invalid_public_key', 'network_identity_missing', 'network_identity_mismatch'])('preserves canonical identity state access: %s', validatorIdentityState => {
    render(<LinkedValidatorSection node={{ ...node, validator: null, validatorIdentityState }} />)
    expect(screen.getByRole('status', { name: 'Validator identity state: ' + validatorIdentityState }).textContent).toBe('No Validator identity has been established for this Node.')
  })
})

describe('LinkedValidatorSection Node detail', () => {
  it('keeps the identity, full identifier, copy control and all six metrics', async () => {
    const identifier = '0X' + 'AbCdEf0123456789'.repeat(8)
    const writeText = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('navigator', { clipboard: { writeText } })
    renderDetail({ validator: { ...insight, validatorNodeId: identifier } })
    const region = detailRegion()
    expect(within(region).getByText('Linked Validator')).toBeTruthy()
    expect(within(region).getByText('Validator One')).toBeTruthy()
    expect(within(region).getByText(identifier, { exact: true })).toBeTruthy()
    expect(region.querySelectorAll('[data-slot="metric-row"]')).toHaveLength(6)
    fireEvent.click(within(region).getByRole('button', { name: 'Copy full Validator identifier' }))
    await waitFor(() => expect(within(region).getByRole('status', { name: 'Identifier copy status' }).textContent).toBe('Validator identifier copied.'))
    expect(writeText).toHaveBeenCalledExactlyOnceWith(identifier)
  })

  it('announces clipboard failure and keeps the full identifier selectable', async () => {
    const identifier = '0x' + 'aBcD'.repeat(32)
    vi.stubGlobal('navigator', {})
    renderDetail({ validator: { ...insight, validatorNodeId: identifier } })
    fireEvent.click(screen.getByRole('button', { name: 'Copy full Validator identifier' }))
    await waitFor(() => expect(screen.getByRole('status', { name: 'Identifier copy status' }).textContent).toMatch(/Copy failed.*manually/))
    expect(screen.getByText(identifier, { exact: true })).toBeTruthy()
  })

  it('shows the full available precision only in Node detail', () => {
    renderDetail({ validator: { ...insight, rewardAmount: '1234567.890123456789', blockRate: '125.123456', genBlocksRate: '135.987654', delegationRewardPercentage: '20.123456' } })
    expect(metricValue('Cumulative rewards')).toBe('1,234,567.890123456789')
    expect(metricValue('Production rate')).toBe('125.123456%')
    expect(metricValue('PlatScan 24h rate')).toBe('135.987654%')
    expect(metricValue('Delegation reward share')).toBe('20.123456%')
    expect(screen.getByText(/Network native unit/)).toBeTruthy()
  })

  it('shows the source, independent success times and cutoff', () => {
    renderDetail()
    const region = detailRegion()
    expect(within(region).getByText('Last success')).toBeTruthy()
    expect(within(region).getByText('Rank last success')).toBeTruthy()
    expect(within(region).getByText('Rank cohort')).toBeTruthy()
    expect(within(region).getByText('platscan')).toBeTruthy()
    expect(within(region).getByText('Source cutoff')).toBeTruthy()
    // The raw provider timestamp is never rendered as text.
    expect(screen.queryByText('2026-08-25T00:00:00Z')).toBeNull()
  })

  it('never presents an absent source cutoff as a fabricated time', () => {
    renderDetail({ validator: { ...insight, providerTimestamp: null } })
    expect(screen.getByText('Not provided by the source')).toBeTruthy()
  })

  it('prints every public state vocabulary entry', () => {
    renderDetail()
    const states = screen.getByLabelText('Public Validator states')
    expect(within(states).getByText('Provider state').nextElementSibling?.textContent).toBe('fresh')
    expect(within(states).getByText('Staking status state').nextElementSibling?.textContent).toBe('current')
    expect(within(states).getByText('Production rate state').nextElementSibling?.textContent).toBe('ok')
  })

  it('keeps retained and not-applicable explanations alongside the metrics', () => {
    renderDetail({ validator: { ...insight, state: 'error', freshness: 'stale', blockRate: null, blockRateState: 'not_applicable', counterState: 'counter_reset' } })
    expect(screen.getByText(/zero scheduled-block denominator/)).toBeTruthy()
    expect(screen.getByText(/prior value was not treated as normal growth/)).toBeTruthy()
    expect(screen.getByText(/Showing the last successful value/)).toBeTruthy()
  })

  it('keeps the locked qualifier and the unknown-staking explanation', () => {
    renderDetail({ validator: { ...insight, currentValidatorStatus: 'validator', currentValidatorStatusQualifier: 'locked' } })
    expect(screen.getByText('Locked')).toBeTruthy()
    expect(screen.getByText(/confirmed-valid staking identity but is locked/)).toBeTruthy()
    cleanup()
    renderDetail({ validator: { ...insight, currentValidatorStatus: 'unknown', currentValidatorStatusState: 'unknown', currentValidatorStatusQualifier: null } })
    expect(screen.getByText(/not a negative conclusion/)).toBeTruthy()
  })

  it('keeps the sanitized identity explanation when no Link is established', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: null, validatorIdentityState: 'network_identity_mismatch', validatorIdentityReason: 'The observed Network Identity does not match.' }} variant="detail" />)
    expect(screen.getByText('Validator identity')).toBeTruthy()
    expect(screen.getByText('The observed Network Identity does not match.')).toBeTruthy()
  })
})

describe('validatorDataStatus', () => {
  it('stays silent for routine fresh data and for the authoritative empty verdict', () => {
    expect(validatorDataStatus(insight)).toBeNull()
    expect(validatorDataStatus({ ...insight, state: 'empty', freshness: 'fresh', currentValidatorStatus: 'not_validator' })).toBeNull()
  })

  it.each([
    ['error', 'Failed', 'destructive'],
    ['stale', 'Stale', 'warning'],
    ['not_configured', 'Not configured', 'warning'],
    ['unsupported', 'Unsupported', 'warning'],
    ['not_found', 'Not found', 'warning'],
  ] as const)('surfaces %s as a short %s cue', (state, label, tone) => {
    const status = validatorDataStatus({ ...insight, state, freshness: state === 'error' ? 'stale' : 'unknown' })
    expect(status).not.toBeNull()
    expect(status?.label).toBe(label)
    expect(status?.tone).toBe(tone)
    expect(status?.description).toBeTruthy()
  })

  it('surfaces aged data even when the Provider state itself is fresh', () => {
    expect(validatorDataStatus({ ...insight, freshness: 'stale' })?.label).toBe('Stale')
  })

  it('surfaces an independently failed or aged ranking list', () => {
    expect(validatorDataStatus({ ...insight, rankState: 'error', rankFreshness: 'stale' })?.label).toBe('Ranking failed')
    expect(validatorDataStatus({ ...insight, rankState: 'ranked', rankFreshness: 'stale' })?.label).toBe('Rank stale')
    expect(validatorDataStatus({ ...insight, rankState: 'ranked', rankFreshness: 'fresh' })).toBeNull()
  })
})

describe('Validator label mapping', () => {
  it('maps Current Validator Status and Provider states to fixed labels', () => {
    expect(currentValidatorStatusLabel('validator')).toBe('Validator')
    expect(currentValidatorStatusLabel('not_validator')).toBe('Not a Validator')
    expect(currentValidatorStatusLabel('unknown')).toBe('Validator status unknown')
    expect(validatorStateLabel('fresh', 'fresh')).toBe('Current')
    expect(validatorStateLabel('stale', 'stale')).toBe('Stale')
    expect(validatorStateLabel('not_configured', 'unknown')).toBe('Not configured')
    expect(validatorStateLabel('unsupported', 'unknown')).toBe('Unsupported')
    expect(validatorStateLabel('error', 'stale')).toBe('Collection failed')
    expect(validatorStateLabel('unknown', 'unknown')).toBe('Never observed')
  })
})
