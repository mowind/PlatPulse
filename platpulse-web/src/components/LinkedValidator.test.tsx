import { cleanup, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import type { PublicNode, PublicValidatorInsight } from '../api/generated'
import { LinkedValidatorSection, currentValidatorStatusLabel, validatorStateLabel } from './LinkedValidator'

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

afterEach(cleanup)

describe('LinkedValidatorSection', () => {
  it('renders nothing for a Node with no effective Validator Link', () => {
    const { container } = render(<LinkedValidatorSection node={node} />)
    expect(container.querySelector('[data-slot="linked-validator"]')).toBeNull()
    expect(screen.queryByText('Linked Validator')).toBeNull()
    expect(screen.queryByText('Cumulative blocks')).toBeNull()
  })

  it('shows identity, Current Validator Status, and the cumulative block count', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: insight }} />)
    const section = screen.getByLabelText('Linked Validator')
    expect(within(section).getByText('Validator One')).toBeTruthy()
    expect(within(section).getByText('Validator')).toBeTruthy()
    expect(within(section).getByText('Cumulative blocks')).toBeTruthy()
    expect(within(section).getByText('4,321')).toBeTruthy()
    expect(within(section).getByText('Current')).toBeTruthy()
  })

  it('shows the explicit locked qualifier and an unknown status without a manual role', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, currentValidatorStatus: 'validator', currentValidatorStatusState: 'current', currentValidatorStatusQualifier: 'locked' } }} />)
    expect(screen.getByText('Locked')).toBeTruthy()
    expect(screen.getByText(/confirmed-valid staking identity but is locked/)).toBeTruthy()

    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, currentValidatorStatus: 'unknown', currentValidatorStatusState: 'unknown', currentValidatorStatusQualifier: null } }} />)
    expect(screen.getByText('Validator status unknown')).toBeTruthy()
    expect(screen.getByText(/not a negative conclusion/)).toBeTruthy()
  })

  it('explains an unestablished automatic correspondence instead of guessing', () => {
    const { container } = render(
      <LinkedValidatorSection
        node={{ ...node, validatorIdentityState: 'network_identity_mismatch', validatorIdentityReason: 'The observed Network Identity does not match.' }}
      />,
    )
    expect(container.querySelector('[data-slot="linked-validator"]')).not.toBeNull()
    expect(screen.getByText('Validator identity')).toBeTruthy()
    expect(screen.getByText('The observed Network Identity does not match.')).toBeTruthy()
    expect(screen.queryByText(/manual/i)).toBeNull()
  })

  it('keeps a source-reported zero distinct from an unknown value', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, blockCount: 0 } }} />)
    expect(screen.getByText('Cumulative blocks').nextElementSibling?.textContent).toBe('0')

    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, blockCount: null } }} />)
    expect(screen.getByText('Cumulative blocks').nextElementSibling?.textContent).toBe('Unknown')
  })

  it('labels an unconfigured source explicitly and retains the last-good value', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, state: 'not_configured', freshness: 'unknown', providerTimestamp: null } }} />)
    expect(screen.getByText('Not configured')).toBeTruthy()
    expect(screen.getByText(/No Validator source is configured for this Network/)).toBeTruthy()
    expect(screen.getByText('4,321')).toBeTruthy()
    expect(screen.getByText(/Showing the last successful value/)).toBeTruthy()
  })

  it('marks a stale last-good value and a counter reset without splicing history', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, state: 'stale', freshness: 'stale', counterState: 'counter_reset' } }} />)
    expect(screen.getByText(/last successful value retained/)).toBeTruthy()
    expect(screen.getByText(/Counter reset or correction observed/)).toBeTruthy()
  })

  it('shows the last successful fetch time, source, and source cutoff only in detail', () => {
    const { rerender } = render(<LinkedValidatorSection node={{ ...node, validator: insight }} />)
    expect(screen.queryByText('Last success')).toBeNull()

    rerender(<LinkedValidatorSection node={{ ...node, validator: insight }} variant="detail" />)
    expect(screen.getByText('Last success')).toBeTruthy()
    expect(screen.getAllByText(/2026/).length).toBeGreaterThanOrEqual(2)
    expect(screen.getByText('platscan')).toBeTruthy()
    expect(screen.getByText('Source cutoff')).toBeTruthy()
    // The raw provider timestamp is never rendered as text.
    expect(screen.queryByText('2026-08-25T00:00:00Z')).toBeNull()
  })

  it('never presents an absent source cutoff as a fabricated time', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, providerTimestamp: null } }} variant="detail" />)
    expect(screen.getByText('Not provided by the source')).toBeTruthy()
  })

  it('shows gross cumulative rewards on the card and abbreviates them exactly', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, rewardAmount: '1234567.89' } }} />)
    expect(screen.getByText('Cumulative rewards')).toBeTruthy()
    expect(screen.getByText('Cumulative rewards').nextElementSibling?.textContent).toBe('1.23M')
  })

  it('shows the source full precision and native unit in Node detail', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, rewardAmount: '1234567.890123456789' } }} variant="detail" />)
    expect(screen.getByText('Cumulative rewards').nextElementSibling?.textContent).toBe('1,234,567.890123456789')
    expect(screen.getByText(/Network native unit/)).toBeTruthy()
  })

  it('keeps a source-reported zero reward distinct from unknown', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, rewardAmount: '0' } }} />)
    expect(screen.getByText('Cumulative rewards').nextElementSibling?.textContent).toBe('0')

    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, rewardAmount: null } }} />)
    expect(screen.getByText('Cumulative rewards').nextElementSibling?.textContent).toBe('Unknown')
  })

  it('retains a last-good reward after a source failure even without a block count', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, state: 'error', freshness: 'stale', blockCount: null, rewardAmount: '42.5' } }} />)
    expect(screen.getByText('Cumulative rewards').nextElementSibling?.textContent).toBe('42.5')
    expect(screen.getByText(/Showing the last successful value/)).toBeTruthy()
  })

  it('shows both production rates with their distinct source meanings', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: insight }} />)
    expect(screen.getByText('Production rate').nextElementSibling?.textContent).toBe('90.91%')
    expect(screen.getByText('PlatScan 24h rate').nextElementSibling?.textContent).toBe('75.50%')
  })

  it('distinguishes a zero scheduled denominator from an incomplete pair', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, blockRate: null, blockRateState: 'not_applicable' } }} />)
    expect(screen.getByText('Production rate').nextElementSibling?.textContent).toBe('Not applicable')
    expect(screen.getByText(/zero scheduled-block denominator/)).toBeTruthy()

    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, blockRate: null, blockRateState: 'unknown' } }} />)
    expect(screen.getByText('Production rate').nextElementSibling?.textContent).toBe('Unknown')
  })

  it('keeps a source-reported zero 24h rate distinct from a missing one', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, genBlocksRate: '0' } }} />)
    expect(screen.getByText('PlatScan 24h rate').nextElementSibling?.textContent).toBe('0.00%')

    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, genBlocksRate: null } }} />)
    expect(screen.getByText('PlatScan 24h rate').nextElementSibling?.textContent).toBe('Unknown')
  })

  it('retains a last-good rate after a source failure and marks it retained', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, state: 'error', freshness: 'stale', genBlocksRate: '0' } }} />)
    expect(screen.getByText('PlatScan 24h rate').nextElementSibling?.textContent).toBe('0.00%')
    expect(screen.getByText('Production rate').nextElementSibling?.textContent).toBe('90.91%')
    expect(screen.getByText(/Showing the last successful value/)).toBeTruthy()
  })

  it('shows the full available rate precision only in Node detail', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: insight }} variant="detail" />)
    expect(screen.getByText('Production rate').nextElementSibling?.textContent).toBe('90.909091%')
    expect(screen.getByText('PlatScan 24h rate').nextElementSibling?.textContent).toBe('75.5%')
  })

  it('shows the delegation reward share in percentage points, not a fraction or basis points', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, delegationRewardPercentage: '20' } }} />)
    expect(screen.getByText('Delegation reward share')).toBeTruthy()
    expect(screen.getByText('Delegation reward share').nextElementSibling?.textContent).toBe('20.00%')

    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, delegationRewardPercentage: '20' } }} variant="detail" />)
    expect(screen.getByText('Delegation reward share').nextElementSibling?.textContent).toBe('20%')
  })

  it('keeps legitimate 0 and 100 boundaries and a missing delegation share distinct', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, delegationRewardPercentage: '0' } }} />)
    expect(screen.getByText('Delegation reward share').nextElementSibling?.textContent).toBe('0.00%')

    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, delegationRewardPercentage: '100' } }} />)
    expect(screen.getByText('Delegation reward share').nextElementSibling?.textContent).toBe('100.00%')

    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, delegationRewardPercentage: null } }} />)
    expect(screen.getByText('Delegation reward share').nextElementSibling?.textContent).toBe('Unknown')
  })

  it('retains a last-good delegation share after a failure without carrying it from another observation', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, state: 'error', freshness: 'stale', delegationRewardPercentage: '20' } }} />)
    expect(screen.getByText('Delegation reward share').nextElementSibling?.textContent).toBe('20.00%')
    expect(screen.getByText(/Showing the last successful value/)).toBeTruthy()
  })

  it('shows the Network rank and keeps unranked distinct from failure or unknown', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: insight }} />)
    expect(screen.getByText('Network rank')).toBeTruthy()
    expect(screen.getByText('Network rank').nextElementSibling?.textContent).toBe('#7')

    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, rank: null, rankState: 'unranked' } }} />)
    expect(screen.getByText('Network rank').nextElementSibling?.textContent).toBe('Unranked')
    expect(screen.getAllByText(/complete live-staking ALL cohort/).length).toBeGreaterThan(0)

    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, rank: null, rankState: 'error' } }} />)
    expect(screen.getByText('Network rank').nextElementSibling?.textContent).toBe('Unknown')
    expect(screen.queryByText('Unranked')).toBeNull()
  })

  it('retains a last-good rank when the list cannot be refreshed and marks the stale ranking', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, state: 'error', freshness: 'stale', rankState: 'error', rankFreshness: 'stale' } }} />)
    expect(screen.getByText('Network rank').nextElementSibling?.textContent).toBe('#7')
    expect(screen.getByText(/last successful rank is retained/)).toBeTruthy()

    cleanup()
    // Detail stays Current while the ranking list itself aged out.
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, state: 'fresh', freshness: 'fresh', rankFreshness: 'stale' } }} />)
    expect(screen.getByText('Current')).toBeTruthy()
    expect(screen.getByText(/ranking list has not refreshed recently/)).toBeTruthy()
  })

  it('never presents a failed or absent ranking as unranked or zero', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, rank: null, rankState: 'unknown', rankFreshness: 'unknown' } }} />)
    expect(screen.getByText('Network rank').nextElementSibling?.textContent).toBe('Unknown')
    expect(screen.queryByText('Unranked')).toBeNull()
    expect(screen.queryByText('0')).toBeNull()
  })

  it('shows the independent ranking success time and cohort in Node detail', () => {
    render(
      <LinkedValidatorSection
        node={{ ...node, validator: { ...insight, rankReceivedAt: '2026-08-20T01:02:03Z', rankCohortSize: 123 } }}
        variant="detail"
      />,
    )
    expect(screen.getByText('Rank last success')).toBeTruthy()
    expect(screen.getByText('Rank cohort')).toBeTruthy()
    expect(screen.getByText('123')).toBeTruthy()
  })

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
