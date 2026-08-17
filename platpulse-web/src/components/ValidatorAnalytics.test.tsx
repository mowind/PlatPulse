import { render, screen, cleanup, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import type { PublicValidatorAnalyticsResponse, AdminValidatorAnalyticsResponse } from '../api/generated'
import { ValidatorAnalytics } from './ValidatorAnalytics'

const publicAnalytics: PublicValidatorAnalyticsResponse = {
  validatorId: 'validator-1',
  state: 'fresh',
  freshness: 'fresh',
  daily: [
    {
      localDate: '2026-02-01',
      monthKey: '2026-02',
      timezone: 'Asia/Tokyo',
      sampleAt: '2026-01-31T15:30:00Z',
      rank: 2,
      stakeAmount: '1000',
      rewardAmount: '10',
      rewardRate: '0.05',
      delegatorCount: 8,
      epoch: 42,
      blockCount: 100,
    },
    {
      localDate: '2026-03-01',
      monthKey: '2026-03',
      timezone: 'Asia/Tokyo',
      sampleAt: '2026-02-28T15:30:00Z',
      rank: 1,
      stakeAmount: '1100',
      rewardAmount: '11',
      rewardRate: '0.06',
      delegatorCount: 9,
      epoch: 43,
      blockCount: 101,
    },
  ],
  monthly: [
    {
      monthKey: '2026-02',
      timezone: 'Asia/Tokyo',
      snapshotCount: 1,
      firstSampleAt: '2026-01-31T15:30:00Z',
      lastSampleAt: '2026-01-31T15:30:00Z',
      rankMin: 2,
      rankMax: 2,
      rankLast: 2,
      stakeLast: '1000',
      rewardLast: '10',
      rewardRateLast: '0.05',
      delegatorCountLast: 8,
      epochLast: 42,
      blockCountLast: 100,
    },
  ],
}

const adminAnalytics: AdminValidatorAnalyticsResponse = {
  validatorId: 'validator-1',
  state: 'error',
  freshness: 'stale',
  daily: [
    {
      localDate: '2026-02-01',
      monthKey: '2026-02',
      timezone: 'Asia/Tokyo',
      sampleAt: '2026-01-31T15:30:00Z',
      receivedAt: '2026-01-31T15:31:00Z',
      providerTimestamp: '2026-01-31T15:30:00Z',
      source: 'explorer',
      rank: 2,
      stakeAmount: '1000',
      rewardAmount: '10',
      rewardRate: '0.05',
      delegatorCount: 8,
      epoch: 42,
      blockCount: 100,
    },
  ],
  monthly: [
    {
      monthKey: '2026-02',
      timezone: 'Asia/Tokyo',
      snapshotCount: 1,
      firstSampleAt: '2026-01-31T15:30:00Z',
      lastSampleAt: '2026-01-31T15:30:00Z',
      rankMin: 2,
      rankMax: 2,
      rankLast: 2,
      stakeLast: '1000',
      rewardLast: '10',
      rewardRateLast: '0.05',
      delegatorCountLast: 8,
      epochLast: 42,
      blockCountLast: 100,
      updatedAt: '2026-01-31T15:31:00Z',
    },
  ],
}

afterEach(cleanup)

describe('ValidatorAnalytics', () => {
  it('renders public daily/monthly tables and a chart alternative', () => {
    render(<ValidatorAnalytics analytics={publicAnalytics} />)
    expect(screen.getByText('Fresh')).toBeTruthy()
    expect(screen.getByText('2026-02-01')).toBeTruthy()
    expect(screen.getByText('2026-02')).toBeTruthy()
    expect(screen.getByText((_content, element) => element?.textContent?.replace(/\s+/g, ' ').trim() === '2 / 2 / 2')).toBeTruthy()
    expect(screen.getByRole('img', { name: /Daily validator rank trend/ })).toBeTruthy()
    const table = screen.getAllByRole('table')[0]
    expect(within(table).getByText('1000')).toBeTruthy()
    expect(screen.queryByText('Received')).toBeNull()
    expect(screen.queryByText('Source')).toBeNull()
  })

  it('renders admin-only source/received/updated fields distinctly', () => {
    render(<ValidatorAnalytics analytics={adminAnalytics} />)
    expect(screen.getByText('Error')).toBeTruthy()
    expect(screen.getAllByText('explorer').length).toBeGreaterThan(0)
    expect(screen.getAllByText('2026-01-31T15:31:00Z').length).toBeGreaterThan(0)
    expect(screen.getByText('Updated')).toBeTruthy()
  })

  it('renders an explicit empty state without fabricating healthy zeros', () => {
    render(
      <ValidatorAnalytics
        analytics={{ validatorId: 'validator-2', state: 'unknown', freshness: 'unknown', daily: [], monthly: [] }}
      />,
    )
    expect(screen.getByText('Unknown')).toBeTruthy()
    expect(screen.getByText(/Never observed/)).toBeTruthy()
    expect(screen.getByText('No Validator analytics yet.')).toBeTruthy()
  })
})