import { act, cleanup, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { ConsensusHeights, LastReportAge, networkHeadComparison } from './NodeDetailObservations'

const head = { currentHead: 100, freshness: '2026-01-01T00:00:00Z', health: 'healthy', rpcState: 'ok', networkReferenceHead: 102, networkReferenceConfidence: 'high' }
const consensus = { state: 'ok', freshness: 'current', highestQcBlock: 100, highestLockBlock: 99, highestCommitBlock: 98 }
afterEach(() => { cleanup(); vi.useRealTimers(); vi.restoreAllMocks() })

describe('Node height comparisons', () => {
  it('subtracts the network reference from Node Head, including zero and positive deltas', () => {
    expect(networkHeadComparison(head)).toEqual({ delta: '−2' })
    expect(networkHeadComparison({ ...head, currentHead: 102 })).toEqual({ delta: '0' })
    expect(networkHeadComparison({ ...head, currentHead: 104 })).toEqual({ delta: '+2' })
  })
  it.each([
    { freshness: null }, { freshness: 'not-a-time' }, { freshness: 'current' },
    { health: 'unknown' }, { health: 'unhealthy' }, { rpcState: 'error' },
    { currentHead: null }, { currentHead: NaN }, { currentHead: -1 },
    { networkReferenceHead: null }, { networkReferenceHead: Infinity },
    { networkReferenceConfidence: 'low' }, { networkReferenceConfidence: 'unknown' },
  ])('explains unavailable or non-current operands: %j', (change) => {
    const result = networkHeadComparison({ ...head, ...change })
    expect(result.delta).toBe('—')
    expect(result.reason).toBeTruthy()
  })
  it('shows consensus absolute heights and deltas against Node Head', () => {
    render(<ConsensusHeights node={{ ...head, consensus }} />)
    expect(within(screen.getByRole('group', { name: 'QC height' })).getByText('0 vs node head')).toBeTruthy()
    expect(within(screen.getByRole('group', { name: 'Locked height' })).getByText('−1 vs node head')).toBeTruthy()
    expect(within(screen.getByRole('group', { name: 'Committed height' })).getByText('98')).toBeTruthy()
  })
  it.each([{ freshness: 'stale' }, { state: 'error' }])('retains marked last-good consensus but never its delta: %j', (change) => {
    render(<ConsensusHeights node={{ ...head, consensus: { ...consensus, ...change } }} />)
    expect(screen.getAllByText(/Last-good/)).toHaveLength(3)
    expect(screen.getAllByText('— vs node head')).toHaveLength(3)
    expect(screen.getByText('98')).toBeTruthy()
  })
  it('does not calculate consensus deltas from a failed Node RPC observation', () => {
    render(<ConsensusHeights node={{ ...head, rpcState: 'error', consensus }} />)
    expect(screen.getAllByText('— vs node head')).toHaveLength(3)
  })
  it('never replaces missing consensus heights with zero', () => {
    render(<ConsensusHeights node={{ ...head, consensus: { state: 'ok', freshness: 'current' } }} />)
    expect(screen.getAllByText('Unknown')).toHaveLength(3)
    expect(screen.queryByText('0')).toBeNull()
  })
  it.each(['starting', 'disabled', 'unsupported'])('suppresses %s consensus evidence even with numeric values', (state) => {
    render(<ConsensusHeights node={{ ...head, consensus: { ...consensus, state } }} />)
    expect(screen.getAllByText('Unknown')).toHaveLength(3)
    expect(screen.getAllByText('— vs node head')).toHaveLength(3)
  })
  it('handles an absent consensus projection without inventing zero heights', () => {
    // Exercise a defensive runtime boundary beyond the generated DTO contract.
    const node = JSON.parse(JSON.stringify(head))
    render(<ConsensusHeights node={node} />)
    expect(screen.getAllByText('Unknown')).toHaveLength(3)
    expect(screen.getAllByText('— vs node head')).toHaveLength(3)
    expect(screen.queryByText('0')).toBeNull()
  })
  it('suppresses unknown consensus evidence even if it contains numbers', () => {
    render(<ConsensusHeights node={{ ...head, consensus: { ...consensus, freshness: 'unknown' } }} />)
    expect(screen.getAllByText('Unknown')).toHaveLength(3)
  })
})

describe('Last report display clock', () => {
  it('resynchronizes a report arriving between ticks rather than warning about the future', () => {
    vi.useFakeTimers(); vi.setSystemTime(new Date('2026-01-01T00:00:10Z'))
    const { rerender } = render(<LastReportAge timestamp="2026-01-01T00:00:00Z" />)
    vi.setSystemTime(new Date('2026-01-01T00:00:10.500Z'))
    rerender(<LastReportAge timestamp="2026-01-01T00:00:10.400Z" />)
    expect(screen.getByText('0s ago')).toBeTruthy()
    expect(screen.queryByText(/Future timestamp/)).toBeNull()
  })
  it('ticks each second without a network update and keeps precise UTC visible', () => {
    vi.useFakeTimers(); vi.setSystemTime(new Date('2026-01-01T00:00:10Z'))
    render(<LastReportAge timestamp="2026-01-01T00:00:00Z" />)
    expect(screen.getByText('10s ago')).toBeTruthy()
    act(() => vi.advanceTimersByTime(1000))
    expect(screen.getByText('11s ago')).toBeTruthy()
    expect(screen.getByText('2026-01-01 00:00:00.000 UTC').tagName).toBe('TIME')
  })
  it('stops while hidden, catches up immediately on visibility restoration, and cleans up', () => {
    vi.useFakeTimers(); vi.setSystemTime(new Date('2026-01-01T00:00:10Z'))
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible')
    const { unmount } = render(<LastReportAge timestamp="2026-01-01T00:00:00Z" />)
    visibility.mockReturnValue('hidden')
    act(() => document.dispatchEvent(new Event('visibilitychange')))
    expect(vi.getTimerCount()).toBe(0)
    act(() => vi.advanceTimersByTime(10000))
    expect(screen.getByText('10s ago')).toBeTruthy()
    visibility.mockReturnValue('visible')
    act(() => document.dispatchEvent(new Event('visibilitychange')))
    expect(screen.getByText('20s ago')).toBeTruthy()
    unmount(); expect(vi.getTimerCount()).toBe(0)
  })
  it.each([null, undefined, '', 'not-a-date'])('shows Unknown for absent or invalid time %s', (timestamp) => {
    render(<LastReportAge timestamp={timestamp} />)
    expect(screen.getByText('Unknown')).toBeTruthy()
  })
  it('explicitly warns about a future timestamp without negative age', () => {
    vi.useFakeTimers(); vi.setSystemTime(new Date('2026-01-01T00:00:00Z'))
    render(<LastReportAge timestamp="2026-01-01T00:00:10Z" />)
    expect(screen.getByText('Future timestamp · check clock')).toBeTruthy()
    expect(screen.queryByText(/ago/)).toBeNull()
  })
})
