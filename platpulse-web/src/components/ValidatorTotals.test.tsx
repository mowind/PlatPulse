import { cleanup, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import type { PublicNetwork } from '../api/generated'
import { ValidatorTotalsSection } from './ValidatorTotals'

type Summary = PublicNetwork['validatorSummary']

function network(overrides: {
  networkKey: string
  displayName: string
  nodeCount?: number
  summary: Summary
}): PublicNetwork {
  const nodeCount = overrides.nodeCount ?? 1
  return {
    networkKey: overrides.networkKey,
    displayName: overrides.displayName,
    nodes: Array.from({ length: nodeCount }, (_, index) => ({ nodeId: `${overrides.networkKey}-${index}` })),
    validatorSummary: overrides.summary,
  } as unknown as PublicNetwork
}

function metricValue(article: HTMLElement, label: string): string {
  const row = within(article).getByText(label, { exact: true }).closest('[data-slot="metric-row"]')
  if (!row) throw new Error(`No metric row for ${label}`)
  const value = row.querySelector('[data-slot="metric-row-value"]')
  if (!value) throw new Error(`No metric value for ${label}`)
  return value.textContent ?? ''
}

afterEach(cleanup)

describe('Validator totals (issue #159)', () => {
  it('groups the current selection by Network with exact deduplicated totals and independent coverage', () => {
    const mainnet = network({
      networkKey: 'mainnet',
      displayName: 'Mainnet',
      nodeCount: 7,
      summary: {
        blocks: { knownSum: 1108, expectedCount: 5, valuedCount: 4, staleCount: 1, state: 'partial' },
        rewards: { knownSum: '11.750000000001', expectedCount: 5, valuedCount: 3, staleCount: 1, state: 'partial' },
        eligibleValidatorCount: 5,
        linkedNodeCount: 6,
        unlinkedNodeCount: 1,
      },
    })
    const testnet = network({
      networkKey: 'testnet',
      displayName: 'Testnet',
      nodeCount: 1,
      summary: {
        blocks: { knownSum: 7, expectedCount: 1, valuedCount: 1, staleCount: 0, state: 'complete' },
        rewards: { knownSum: '2', expectedCount: 1, valuedCount: 1, staleCount: 0, state: 'complete' },
        eligibleValidatorCount: 1,
        linkedNodeCount: 1,
        unlinkedNodeCount: 0,
      },
    })

    render(<ValidatorTotalsSection networks={[mainnet, testnet]} />)

    const mainnetArticle = screen.getByRole('article', { name: 'Validator totals for Mainnet' })
    const testnetArticle = screen.getByRole('article', { name: 'Validator totals for Testnet' })
    // Networks never combine: the mainnet total is the deduplicated set, not
    // a Node sum, and testnet keeps its own value.
    expect(metricValue(mainnetArticle, 'Cumulative blocks')).toBe('1,108')
    expect(metricValue(testnetArticle, 'Cumulative blocks')).toBe('7')
    // Exact decimal string sums keep every source digit on Home.
    expect(metricValue(mainnetArticle, 'Cumulative rewards')).toBe('11.750000000001')
    expect(metricValue(testnetArticle, 'Cumulative rewards')).toBe('2')
    // Per-metric coverage is independent: blocks cover one more Validator
    // than rewards, and both mark the stale contributor.
    expect(within(mainnetArticle).getByText('4/5 Validators with values · 1 stale')).toBeTruthy()
    expect(within(mainnetArticle).getByText('3/5 Validators with values · 1 stale')).toBeTruthy()
    expect(within(mainnetArticle).getAllByText(/Known values subtotal/)).toHaveLength(2)
    // Unlinked Nodes are reported separately, never as zero-value Validators.
    expect(within(mainnetArticle).getByText('1 unlinked Node')).toBeTruthy()
    expect(within(testnetArticle).getByText('0 unlinked Nodes')).toBeTruthy()
  })

  it('shows Unknown when no value exists and a real zero when the source reported zero', () => {
    const unknown = network({
      networkKey: 'unknown-net',
      displayName: 'Unknown Network',
      summary: {
        blocks: { knownSum: null, expectedCount: 2, valuedCount: 0, staleCount: 0, state: 'unknown' },
        rewards: { knownSum: null, expectedCount: 2, valuedCount: 0, staleCount: 0, state: 'unknown' },
        eligibleValidatorCount: 2,
        linkedNodeCount: 2,
        unlinkedNodeCount: 0,
      },
    })
    const zero = network({
      networkKey: 'zero-net',
      displayName: 'Zero Network',
      summary: {
        blocks: { knownSum: 0, expectedCount: 1, valuedCount: 1, staleCount: 0, state: 'complete' },
        rewards: { knownSum: '0.000', expectedCount: 1, valuedCount: 1, staleCount: 0, state: 'complete' },
        eligibleValidatorCount: 1,
        linkedNodeCount: 1,
        unlinkedNodeCount: 0,
      },
    })

    render(<ValidatorTotalsSection networks={[unknown, zero]} />)
    const unknownArticle = screen.getByRole('article', { name: 'Validator totals for Unknown Network' })
    expect(metricValue(unknownArticle, 'Cumulative blocks')).toBe('Unknown')
    expect(metricValue(unknownArticle, 'Cumulative rewards')).toBe('Unknown')
    expect(within(unknownArticle).getAllByText(/showing Unknown rather than zero/)).toHaveLength(2)
    // A legitimate source zero stays a value, never Unknown.
    const zeroArticle = screen.getByRole('article', { name: 'Validator totals for Zero Network' })
    expect(metricValue(zeroArticle, 'Cumulative blocks')).toBe('0')
    expect(metricValue(zeroArticle, 'Cumulative rewards')).toBe('0')
  })

  it('renders nothing without a summary so a pre-feature projection never fabricates totals', () => {
    const legacy = { networkKey: 'mainnet', displayName: 'Mainnet', nodes: [{}] } as unknown as PublicNetwork
    const { container } = render(<ValidatorTotalsSection networks={[legacy]} />)
    expect(container.querySelector('[data-slot="validator-summary"]')).toBeNull()
  })
})
