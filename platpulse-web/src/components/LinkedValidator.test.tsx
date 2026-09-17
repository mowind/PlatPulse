import { cleanup, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import type { PublicNode, PublicValidatorInsight } from '../api/generated'
import { LinkedValidatorSection, validatorRoleLabel, validatorStateLabel } from './LinkedValidator'

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
  linkRole: 'standby',
  state: 'fresh',
  freshness: 'fresh',
  source: 'platscan',
  providerTimestamp: '2026-08-25T00:00:00Z',
  receivedAt: '2026-08-25T00:00:05Z',
  blockCount: 4321,
  counterState: 'normal',
  activity: 'producing',
  activityState: 'current',
}

afterEach(cleanup)

describe('LinkedValidatorSection', () => {
  it('shows the unlinked state instead of a fabricated zero', () => {
    render(<LinkedValidatorSection node={node} />)
    expect(screen.getByText('Linked Validator')).toBeTruthy()
    expect(screen.getByText(/Unlinked/)).toBeTruthy()
    expect(screen.queryByText('Cumulative blocks')).toBeNull()
    expect(screen.queryByText('0')).toBeNull()
  })

  it('shows identity, link role, and the cumulative Validator block count', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: insight }} />)
    const section = screen.getByLabelText('Linked Validator')
    expect(within(section).getByText('Validator One')).toBeTruthy()
    expect(within(section).getByText('Standby')).toBeTruthy()
    expect(within(section).getByText('Cumulative blocks')).toBeTruthy()
    expect(within(section).getByText('4,321')).toBeTruthy()
    expect(within(section).getByText('Current')).toBeTruthy()
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
    expect(screen.getByText(/last successful cumulative value/)).toBeTruthy()
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

  it('maps roles and states to fixed, sanitized public labels', () => {
    expect(validatorRoleLabel('primary')).toBe('Primary')
    expect(validatorRoleLabel(null)).toBe('Role unknown')
    expect(validatorStateLabel('fresh', 'fresh')).toBe('Current')
    expect(validatorStateLabel('stale', 'stale')).toBe('Stale')
    expect(validatorStateLabel('not_configured', 'unknown')).toBe('Not configured')
    expect(validatorStateLabel('unsupported', 'unknown')).toBe('Unsupported')
    expect(validatorStateLabel('error', 'stale')).toBe('Collection failed')
    expect(validatorStateLabel('unknown', 'unknown')).toBe('Never observed')
  })
})
