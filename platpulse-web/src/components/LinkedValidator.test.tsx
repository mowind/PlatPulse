import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
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

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

function openDetails() {
  fireEvent.click(screen.getByRole('button', { name: 'Open Validator details' }))
  return screen.getByRole('dialog', { name: 'Validator details' })
}

describe('LinkedValidatorSection', () => {
  it.each([undefined, null])('explains a missing discovery pass without claiming absent staking: %s', validatorIdentityState => {
    render(<LinkedValidatorSection node={{ ...node, validatorIdentityState }} />)
    const identity = screen.getByRole('region', { name: 'Linked Validator identity' })
    expect(within(identity).getByRole('status', { name: 'Validator identity state: not observed' }).textContent).toBe('Validator identity has not been observed yet.')
    expect(screen.queryByText('Linked Validator')).toBeNull()
    expect(screen.queryByText('Cumulative blocks')).toBeNull()
    expect(screen.queryByText('Not a Validator')).toBeNull()
    expect(screen.queryByRole('group', { name: 'Linked Validator metrics' })).toBeNull()
  })

  it.each(['card', 'detail'] as const)('distinguishes identified data unavailability from unobserved discovery in %s', variant => {
    render(<LinkedValidatorSection node={{ ...node, validatorIdentityState: 'identified' }} variant={variant} />)
    expect(screen.getByRole('status', { name: 'Validator identity state: identified' }).textContent).toBe('Validator data is unavailable for the identified identity.')
    expect(screen.queryByText('Not a Validator')).toBeNull()
    expect(screen.queryByText('Validator identity has not been observed yet.')).toBeNull()
    expect(screen.queryByRole('group', { name: 'Linked Validator metrics' })).toBeNull()
  })

  it.each(['missing_public_key', 'invalid_public_key', 'network_identity_missing', 'network_identity_mismatch'])('preserves canonical identity state access when no sanitized reason is supplied: %s', validatorIdentityState => {
    render(<LinkedValidatorSection node={{ ...node, validatorIdentityState }} />)
    expect(screen.getByRole('status', { name: 'Validator identity state: ' + validatorIdentityState }).textContent).toBe('No Validator identity has been established for this Node.')
    expect(screen.queryByText('Not a Validator')).toBeNull()
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
    expect(within(openDetails()).getByText(/confirmed-valid staking identity but is locked/)).toBeTruthy()

    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, currentValidatorStatus: 'unknown', currentValidatorStatusState: 'unknown', currentValidatorStatusQualifier: null } }} />)
    expect(screen.getByText('Validator status unknown')).toBeTruthy()
    expect(within(openDetails()).getByText(/not a negative conclusion/)).toBeTruthy()
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
    expect(screen.getByText('Cumulative blocks').nextElementSibling?.textContent).toBe('—')
  })

  it('labels an unconfigured source explicitly and retains the last-good value', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, state: 'not_configured', freshness: 'unknown', providerTimestamp: null } }} />)
    expect(screen.getByText('Not configured')).toBeTruthy()
    expect(screen.getByText('4,321')).toBeTruthy()
    expect(screen.getByText(/last successful value retained/)).toBeTruthy()
    expect(within(openDetails()).getByText(/No Validator source is configured for this Network/)).toBeTruthy()
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
    // The card keeps the abbreviated reward form, never a JavaScript number.
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
    expect(screen.getByText('Cumulative rewards').nextElementSibling?.textContent).toBe('—')
  })

  it('retains a last-good reward after a source failure even without a block count', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, state: 'error', freshness: 'stale', blockCount: null, rewardAmount: '42.5' } }} />)
    expect(screen.getByText('Cumulative rewards').nextElementSibling?.textContent).toBe('42.5')
    expect(screen.getByText(/last successful value retained/)).toBeTruthy()
  })

  it('shows both production rates with their distinct source meanings', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: insight }} />)
    expect(screen.getByText('Production rate').nextElementSibling?.textContent).toBe('90.91%')
    expect(screen.getByText('PlatScan 24h rate').nextElementSibling?.textContent).toBe('75.50%')
  })

  it('distinguishes a zero scheduled denominator from an incomplete pair', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, blockRate: null, blockRateState: 'not_applicable' } }} />)
    expect(screen.getByText('Production rate').nextElementSibling?.textContent).toBe('Not applicable')
    expect(within(openDetails()).getByText(/zero scheduled-block denominator/)).toBeTruthy()

    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, blockRate: null, blockRateState: 'unknown' } }} />)
    expect(screen.getByText('Production rate').nextElementSibling?.textContent).toBe('—')
  })

  it('keeps a source-reported zero 24h rate distinct from a missing one', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, genBlocksRate: '0' } }} />)
    expect(screen.getByText('PlatScan 24h rate').nextElementSibling?.textContent).toBe('0.00%')

    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, genBlocksRate: null } }} />)
    expect(screen.getByText('PlatScan 24h rate').nextElementSibling?.textContent).toBe('—')
  })

  it('retains a last-good rate after a source failure and marks it retained', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, state: 'error', freshness: 'stale', genBlocksRate: '0' } }} />)
    expect(screen.getByText('PlatScan 24h rate').nextElementSibling?.textContent).toBe('0.00%')
    expect(screen.getByText('Production rate').nextElementSibling?.textContent).toBe('90.91%')
    expect(screen.getByText(/last successful value retained/)).toBeTruthy()
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
    expect(screen.getByText('Delegation reward share').nextElementSibling?.textContent).toBe('—')
  })

  it('retains a last-good delegation share after a failure without carrying it from another observation', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, state: 'error', freshness: 'stale', delegationRewardPercentage: '20' } }} />)
    expect(screen.getByText('Delegation reward share').nextElementSibling?.textContent).toBe('20.00%')
    expect(screen.getByText(/last successful value retained/)).toBeTruthy()
  })

  it('shows the Network rank and keeps unranked distinct from failure or unknown', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: insight }} />)
    expect(screen.getByText('Network rank')).toBeTruthy()
    expect(screen.getByText('Network rank').nextElementSibling?.textContent).toBe('#7')

    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, rank: null, rankState: 'unranked' } }} />)
    expect(screen.getByText('Network rank').nextElementSibling?.textContent).toBe('Unranked')
    expect(within(openDetails()).getByText(/complete live-staking ALL cohort/)).toBeTruthy()

    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, rank: null, rankState: 'error' } }} />)
    expect(screen.getByText('Network rank').nextElementSibling?.textContent).toBe('—')
    expect(screen.queryByText('Unranked')).toBeNull()
  })

  it('retains a last-good rank when the list cannot be refreshed and marks the stale ranking', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, state: 'error', freshness: 'stale', rankState: 'error', rankFreshness: 'stale' } }} />)
    expect(screen.getByText('Network rank').nextElementSibling?.textContent).toBe('#7')
    expect(within(openDetails()).getByText(/last successful rank is retained/)).toBeTruthy()

    cleanup()
    // Detail stays Current while the ranking list itself aged out.
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, state: 'fresh', freshness: 'fresh', rankFreshness: 'stale' } }} />)
    expect(screen.getByText('Current')).toBeTruthy()
    expect(within(openDetails()).getByText(/ranking list has not refreshed recently/)).toBeTruthy()
  })

  it('never presents a failed or absent ranking as unranked or zero', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, rank: null, rankState: 'unknown', rankFreshness: 'unknown' } }} />)
    expect(screen.getByText('Network rank').nextElementSibling?.textContent).toBe('—')
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


  it('keeps six metrics above the fold with stacked two-line labels and no metric decoration', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: insight }} />)
    const group = screen.getByRole('group', { name: 'Linked Validator metrics' })
    expect(group.className).toContain('grid-cols-2')
    const metrics = group.querySelectorAll('[data-slot="validator-metric"]')
    expect(metrics).toHaveLength(6)
    for (const metric of metrics) {
      expect(metric.firstElementChild?.className).toContain('min-h-8')
      expect(metric.firstElementChild?.className).toContain('text-xs')
      expect(metric.querySelector('strong')?.className).toContain('tabular-nums')
      expect(metric.className).not.toMatch(/bg-|border-|break-all|overflow-wrap/)
    }
  })

  it('distinguishes Provider Data: Current from unknown current staking status', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, currentValidatorStatus: 'unknown', currentValidatorStatusState: 'unknown' } }} />)
    expect(screen.getByRole('status', { name: 'Validator Provider data state' }).textContent).toBe('Data: Current')
    expect(screen.getByText('Validator status unknown')).toBeTruthy()
    expect(screen.queryByText('Healthy')).toBeNull()
    expect(screen.getByRole('group', { name: 'Linked Validator metrics' })).toBeTruthy()
  })

  it('copies the full original mixed-case prefixed identifier, not its display fragments', async () => {
    const identifier = '0X' + 'AbCdEf0123456789'.repeat(8)
    const writeText = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('navigator', { clipboard: { writeText } })
    const navigate = vi.fn()
    render(<div onClick={navigate}><LinkedValidatorSection node={{ ...node, validator: { ...insight, validatorNodeId: identifier } }} /></div>)
    expect(screen.getByText(identifier.slice(0, 10) + '…' + identifier.slice(-8))).toBeTruthy()
    expect(screen.queryByText(identifier)).toBeNull()
    expect(screen.getByText('Validator One')).toBeTruthy()
    const copy = screen.getByRole('button', { name: 'Copy full Validator identifier' })
    const details = screen.getByRole('button', { name: 'Open Validator details' })
    for (const control of [copy, details]) {
      expect(control.className).toContain('relative z-10')
      expect(control.closest('a')).toBeNull()
    }
    fireEvent.click(copy)
    await waitFor(() => expect(screen.getByRole('status', { name: 'Identifier copy status' }).textContent).toBe('Validator identifier copied.'))
    expect(writeText).toHaveBeenCalledExactlyOnceWith(identifier)
    expect(navigate).not.toHaveBeenCalled()
    const dialog = openDetails()
    expect(navigate).not.toHaveBeenCalled()
    const field = within(dialog).getByRole('textbox', { name: 'Full Validator identifier' })
    expect(field).toBeInstanceOf(HTMLTextAreaElement)
    if (!(field instanceof HTMLTextAreaElement)) throw new Error('Expected selectable identifier')
    expect(field.value).toBe(identifier)
    expect(field.readOnly).toBe(true)
    fireEvent.focus(field)
    expect(field.selectionStart).toBe(0)
    expect(field.selectionEnd).toBe(identifier.length)
    fireEvent.click(within(dialog).getByRole('button', { name: 'Close' }))
    await waitFor(() => expect(document.activeElement).toBe(details))
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it.each(['rejected', 'unavailable'])('announces %s clipboard access and keeps a manual full identifier fallback', async mode => {
    const identifier = '0x' + 'aBcD'.repeat(32)
    vi.stubGlobal('navigator', mode === 'rejected'
      ? { clipboard: { writeText: vi.fn().mockRejectedValue(new Error('Denied')) } }
      : {})
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, validatorNodeId: identifier, displayName: identifier } }} />)
    fireEvent.click(screen.getByRole('button', { name: 'Copy full Validator identifier' }))
    await waitFor(() => expect(screen.getByRole('status', { name: 'Identifier copy status' }).textContent).toMatch(/Copy failed.*manually/))
    expect(screen.queryByText(identifier)).toBeNull()
    const field = within(openDetails()).getByRole('textbox', { name: 'Full Validator identifier' })
    expect(field.getAttribute('readonly')).not.toBeNull()
    expect(field.textContent).toBe(identifier)
  })

  it('keeps exact six metrics and provenance in the dialog without capping rates above 100', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, rewardAmount: '1234567.890123456789', blockRate: '125.123456', genBlocksRate: '135.987654', delegationRewardPercentage: '20.123456' } }} />)
    expect(screen.getByText('Production rate').nextElementSibling?.textContent).toBe('125.12%')
    expect(screen.getByText('PlatScan 24h rate').nextElementSibling?.textContent).toBe('135.99%')
    const dialog = within(openDetails())
    for (const [label, value] of [
      ['Cumulative blocks', '4,321'], ['Cumulative rewards', '1,234,567.890123456789'],
      ['Network rank', '#7'], ['Production rate', '125.123456%'],
      ['PlatScan 24h rate', '135.987654%'], ['Delegation reward share', '20.123456%'],
    ]) expect(dialog.getByText(label).nextElementSibling?.textContent).toBe(value)
    expect(dialog.getByText('Source').nextElementSibling?.textContent).toBe('platscan')
    expect(dialog.getByText('Last success')).toBeTruthy()
    expect(dialog.getByText('Rank last success')).toBeTruthy()
    expect(dialog.getByText('Source cutoff')).toBeTruthy()
    expect(dialog.getByText('Staking status state').nextElementSibling?.textContent).toBe('current')
    expect(dialog.getByText('Provider state').nextElementSibling?.textContent).toBe('fresh')
    expect(dialog.getByText('Production rate state').nextElementSibling?.textContent).toBe('ok')
  })

  const missing: PublicValidatorInsight = {
    ...insight, blockCount: null, rewardAmount: null, rank: null, rankState: 'unknown',
    blockRate: null, blockRateState: 'unknown', expectedBlockCount: null,
    genBlocksRate: null, delegationRewardPercentage: null,
  }
  const notValidator: PublicValidatorInsight = {
    ...missing, currentValidatorStatus: 'not_validator', currentValidatorStatusState: 'current',
  }

  it.each([
    { state: 'fresh', freshness: 'fresh' },
    { state: 'empty', freshness: 'fresh' },
    { state: 'empty', freshness: 'unknown' },
  ])('folds only empty authoritative current non-validator metrics: $state/$freshness', provider => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...notValidator, ...provider } }} />)
    expect(screen.getByText('Not a Validator')).toBeTruthy()
    expect(screen.getByRole('status', { name: 'Validator Provider data state' }).textContent).toContain(validatorStateLabel(provider.state, provider.freshness))
    expect(screen.queryByRole('group', { name: 'Linked Validator metrics' })).toBeNull()
    expect(screen.getByText('No current staking identity; no Validator metrics available.')).toBeTruthy()
    const dialog = within(openDetails())
    expect(dialog.getByRole('group', { name: 'Linked Validator metrics' }).querySelectorAll('[data-slot="metric-row"]')).toHaveLength(6)
    expect(dialog.getByText(/Authoritative evidence reports no current staking identity/)).toBeTruthy()
    expect(dialog.getByText('Provider state').nextElementSibling?.textContent).toBe(provider.state)
  })

  it.each<Partial<PublicValidatorInsight>>([
    { currentValidatorStatus: 'unknown' }, { currentValidatorStatus: 'validator' },
    { currentValidatorStatusState: 'stale' }, { currentValidatorStatusState: 'unknown' },
    { currentValidatorStatusState: undefined },
    { state: 'error' }, { state: 'loading' }, { state: 'unknown' }, { state: 'stale' },
    { state: 'not_configured' }, { state: 'unsupported' }, { state: 'not_found' },
    { freshness: 'stale' }, { freshness: 'unknown' }, { state: 'empty', freshness: 'stale' },
    { rankState: 'unranked' }, { rankState: 'ranked' }, { blockRateState: 'not_applicable' },
    { blockCount: 0 }, { rewardAmount: '0' }, { rank: 7 }, { blockRate: '125' },
    { genBlocksRate: '0' }, { delegationRewardPercentage: '0' },
  ])('does not fold missing slots when evidence or meaningful values remain: %j', override => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...notValidator, ...override } }} />)
    expect(screen.getByRole('group', { name: 'Linked Validator metrics' }).querySelectorAll('[data-slot="validator-metric"]')).toHaveLength(6)
  })

  it('never infers absent staking or folds unknown metrics from consensus Non-validator alone', () => {
    render(<LinkedValidatorSection node={{ ...node, consensus: { state: 'ok', freshness: 'current', validator: false }, validator: { ...missing, currentValidatorStatus: 'unknown', currentValidatorStatusState: 'unknown' } }} />)
    const metrics = screen.getByRole('group', { name: 'Linked Validator metrics' })
    expect(within(metrics).getAllByText('—')).toHaveLength(6)
    expect(within(metrics).getAllByText(/^Unknown/)).toHaveLength(6)
    for (const value of metrics.querySelectorAll('strong')) {
      const description = document.getElementById(value.getAttribute('aria-describedby') ?? '')
      expect(description?.textContent).toMatch(/^Unknown: .+/)
    }
    expect(screen.getByText('Validator status unknown')).toBeTruthy()
    expect(screen.queryByText('Not a Validator')).toBeNull()
    expect(screen.queryByText('0')).toBeNull()
  })

  it('retains historical metrics when current staking identity is absent', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: { ...insight, currentValidatorStatus: 'not_validator', activity: 'exited' } }} />)
    expect(screen.getByText('Not a Validator')).toBeTruthy()
    expect(screen.getByText('Cumulative blocks').nextElementSibling?.textContent).toBe('4,321')
    expect(screen.getByText('Cumulative rewards').nextElementSibling?.textContent).toBe('1.23K')
    expect(screen.getByRole('group', { name: 'Linked Validator metrics' })).toBeTruthy()
  })

  it('retains important independent stale, identity, qualifier and counter-reset cues outside details', () => {
    render(<LinkedValidatorSection node={{ ...node, validatorIdentityReason: 'Identity verification is unavailable.', validator: { ...insight, currentValidatorStatusState: 'stale', currentValidatorStatusQualifier: 'exiting', state: 'error', freshness: 'stale', rankFreshness: 'stale', rankState: 'error', counterState: 'counter_reset' } }} />)
    expect(screen.getByText('Exiting')).toBeTruthy()
    expect(screen.getByText(/Staking status stale/)).toBeTruthy()
    expect(screen.getByText(/last established association retained/)).toBeTruthy()
    expect(screen.getByText(/Network rank stale/)).toBeTruthy()
    expect(screen.getByText(/Network ranking collection failed/)).toBeTruthy()
    expect(screen.getByText(/Counter reset or correction observed/)).toBeTruthy()
    expect(screen.getByText('Collection failed')).toBeTruthy()
    expect(screen.getByText(/last successful value retained/)).toBeTruthy()
    const dialog = within(openDetails())
    expect(dialog.getByText(/prior value was not treated as normal growth/)).toBeTruthy()
    expect(dialog.getByText(/Identity verification is unavailable/)).toBeTruthy()
    expect(dialog.getByText(/confirmed-valid staking identity but is exiting/)).toBeTruthy()
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
