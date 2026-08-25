import { render, screen, cleanup } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import type { PublicValidatorInsight } from '../api/generated'
import { ValidatorInsight } from './ValidatorInsight'

const fresh: PublicValidatorInsight = {
  validatorId: 'validator-1',
  validatorNodeId: '0xvalidator',
  displayName: 'Primary validator',
  nodeId: 'node-1',
  linkRole: 'primary',
  state: 'fresh',
  freshness: 'fresh',
  source: 'explorer',
  providerTimestamp: '2026-08-17T00:00:00Z',
  receivedAt: '2026-08-17T00:00:01Z',
  rank: 7,
  stakeAmount: '12345678901234567890.123456789',
  rewardAmount: '0.25',
  rewardRate: '0.125000000000000001',
  delegatorCount: 12,
  epoch: 4,
  blockCount: 99,
  counterState: 'normal',
  activity: 'active',
  activityState: 'current',
}

afterEach(cleanup)

describe('ValidatorInsight', () => {
  it('renders exact values and explicit freshness state', () => {
    render(<ValidatorInsight insight={fresh} />)
    expect(screen.getByText('Fresh')).toBeTruthy()
    expect(screen.getByText('Primary validator')).toBeTruthy()
    expect(screen.getByText('12345678901234567890.123456789')).toBeTruthy()
    expect(screen.getByText('0.125000000000000001')).toBeTruthy()
  })

  it('keeps last-good metrics visible while showing provider error and reset context', () => {
    render(<ValidatorInsight insight={{ ...fresh, state: 'error', freshness: 'stale', counterState: 'counter_reset' }} />)
    expect(screen.getByText('Error')).toBeTruthy()
    expect(screen.getByText(/stale/)).toBeTruthy()
    expect(screen.getByText(/Counter reset or correction observed/i)).toBeTruthy()
    expect(screen.getByText('7')).toBeTruthy()
  })

  it('does not expose provider diagnostics in the public insight component', () => {
    render(<ValidatorInsight insight={{ ...fresh, state: 'unsupported', freshness: 'unknown' }} />)
    expect(screen.getByText('Unsupported')).toBeTruthy()
    expect(screen.getByText(/Never observed/)).toBeTruthy()
    expect(screen.queryByText(/https?:\/\//)).toBeNull()
  })
})
