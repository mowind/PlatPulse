import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import type { PublicNetwork } from '../api/generated'
import { selectionTotal, ValidatorTotalCard } from './ValidatorTotals'
import { sumKnownAmounts } from '../lib/amount'

type Summary = NonNullable<PublicNetwork['validatorSummary']>
function network(networkKey: string, summary?: Summary, nodeCount = 1): PublicNetwork {
  return { networkKey, displayName: networkKey, nodes: Array.from({ length: nodeCount }, (_, index) => ({ nodeId: networkKey + index })), validatorSummary: summary } as unknown as PublicNetwork
}
const main: Summary = {
  blocks: { knownSum: '1108', expectedCount: 5, valuedCount: 4, staleCount: 1, state: 'partial' },
  rewards: { knownSum: '11.750000000001', expectedCount: 5, valuedCount: 3, staleCount: 1, state: 'partial' },
  eligibleValidatorCount: 5, linkedNodeCount: 6, unlinkedNodeCount: 1,
}
const other: Summary = {
  blocks: { knownSum: '7', expectedCount: 1, valuedCount: 1, staleCount: 0, state: 'complete' },
  rewards: { knownSum: '2', expectedCount: 1, valuedCount: 1, staleCount: 0, state: 'complete' },
  eligibleValidatorCount: 1, linkedNodeCount: 1, unlinkedNodeCount: 0,
}
const networks = [network('Mainnet', main, 7), network('Testnet', other)]
afterEach(cleanup)

describe('Home cumulative overview', () => {
  it('adds Server-deduplicated Network subtotals, not Node projections, with independent coverage', () => {
    expect(selectionTotal(networks, 'blocks')).toEqual({ knownSum: '1115', expectedCount: 6, valuedCount: 5, staleCount: 1, state: 'partial', missingNetworks: 0 })
    expect(selectionTotal(networks, 'rewards')).toEqual({ knownSum: '13.750000000001', expectedCount: 6, valuedCount: 4, staleCount: 1, state: 'partial', missingNetworks: 0 })
    render(<><ValidatorTotalCard networks={networks} metric="blocks" /><ValidatorTotalCard networks={networks} metric="rewards" /></>)
    const blocksCard = screen.getByRole('article', { name: 'Cumulative blocks' })
    expect(blocksCard.textContent).toContain('1,115')
    // The tile face carries its own Network scope and its own valued/expected
    // reporting count directly; the partial denominator is readable without
    // opening the Breakdown.
    expect(blocksCard.querySelector('[data-slot="summary-caption"]')?.textContent).toBe('2 Networks · 5/6 reporting · 1 stale')
    expect(screen.getByRole('article', { name: 'Cumulative rewards' }).querySelector('[data-slot="summary-caption"]')?.textContent)
      .toBe('2 Networks · 4/6 reporting · 1 stale')
    fireEvent.click(screen.getByRole('button', { name: 'Cumulative blocks breakdown and exact values' }))
    const dialog = screen.getByRole('dialog')
    expect(within(dialog).getByText('5/6 reporting · 1 stale')).toBeTruthy()
    expect(within(dialog).getByText(/native unit without asset conversion/)).toBeTruthy()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Close' }))
    expect(document.querySelector('[data-slot="validator-summary"]')).toBeNull()
  })

  it('exposes exact values, per-Network coverage, partial explanations and unlinked Nodes by button', () => {
    render(<ValidatorTotalCard networks={networks} metric="rewards" />)
    fireEvent.click(screen.getByRole('button', { name: 'Cumulative rewards breakdown and exact values' }))
    const dialog = screen.getByRole('dialog')
    expect(within(dialog).getByText('13.750000000001')).toBeTruthy()
    expect(within(dialog).getByText('4/6 reporting · 1 stale')).toBeTruthy()
    const mainnet = within(dialog).getByRole('article', { name: 'Validator totals for Mainnet' })
    expect(within(mainnet).getByText('1,108')).toBeTruthy()
    expect(within(mainnet).getByText('11.750000000001')).toBeTruthy()
    expect(within(mainnet).getByText('4/5 reporting · 1 stale')).toBeTruthy()
    expect(within(mainnet).getByText('3/5 reporting · 1 stale')).toBeTruthy()
    expect(within(mainnet).getByText('1 unlinked Node')).toBeTruthy()
    expect(within(mainnet).getAllByText(/Known values subtotal/)).toHaveLength(2)
    expect(within(dialog).getByText(/not a single-asset balance/)).toBeTruthy()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Close' }))
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('keeps unknown distinct from a legitimate zero and partial zero', () => {
    const missing = network('Missing', { ...other, blocks: { knownSum: null, expectedCount: 2, valuedCount: 0, staleCount: 0, state: 'unknown' } })
    const zero = network('Zero', { ...other, blocks: { ...other.blocks, knownSum: '0' } })
    expect(selectionTotal([missing], 'blocks').knownSum).toBeNull()
    expect(selectionTotal([], 'blocks').knownSum).toBeNull()
    expect(selectionTotal([zero], 'blocks').knownSum).toBe('0')
    expect(selectionTotal([missing, zero], 'blocks')).toMatchObject({ knownSum: '0', state: 'partial', expectedCount: 3, valuedCount: 1 })
    render(<ValidatorTotalCard networks={[missing]} metric="blocks" />)
    const card = screen.getByRole('article', { name: 'Cumulative blocks' })
    expect(card.querySelector('[data-slot="summary-value"]')?.textContent).toBe('Unknown')
    // A missing value is Unknown; the visible coverage line never fabricates a zero total.
    expect(card.querySelector('[data-slot="summary-caption"]')?.textContent).toBe('Missing · 0/2 reporting · Unknown')
    // A real zero keeps its own reporting count instead of reading as Unknown.
    render(<ValidatorTotalCard networks={[zero]} metric="blocks" />)
    const zeroCard = screen.getAllByRole('article', { name: 'Cumulative blocks' }).at(-1)!
    expect(zeroCard.querySelector('[data-slot="summary-value"]')?.textContent).toBe('0')
    expect(zeroCard.querySelector('[data-slot="summary-caption"]')?.textContent).toBe('Zero · 1/1 reporting')
  })

  it('does not fabricate coverage for a missing summary or empty Network', () => {
    const missing = network('Unavailable')
    expect(selectionTotal([missing], 'rewards')).toMatchObject({ knownSum: null, missingNetworks: 1, state: 'unknown' })
    expect(selectionTotal([networks[0], missing], 'rewards')).toMatchObject({ knownSum: '11.750000000001', missingNetworks: 1, state: 'partial' })
    expect(selectionTotal([network('Empty', other, 0)], 'blocks').knownSum).toBeNull()
    render(<ValidatorTotalCard networks={[networks[0], missing]} metric="rewards" />)
    // The reported denominator may not claim the whole selection: the missing
    // Network summary is named beside it instead of being counted as covered.
    expect(screen.getByRole('article', { name: 'Cumulative rewards' }).querySelector('[data-slot="summary-caption"]')?.textContent)
      .toBe('2 Networks · 3/5 reporting · 1 Network summary missing · 1 stale')
    fireEvent.click(screen.getByRole('button', { name: /breakdown/ }))
    const dialog = screen.getByRole('dialog')
    expect(within(dialog).getByText('3/5 reporting · 1 Network summary missing · 1 stale')).toBeTruthy()
    expect(within(dialog).getByText('1 Network summary missing from this coverage.')).toBeTruthy()
    // Every summary missing is coverage unknown, never a complete 0/0 reading.
    cleanup()
    render(<ValidatorTotalCard networks={[missing]} metric="rewards" />)
    expect(screen.getByRole('article', { name: 'Cumulative rewards' }).querySelector('[data-slot="summary-caption"]')?.textContent)
      .toBe('Unavailable · Coverage unknown')
  })

  it('states a denominator of zero as no eligible Validators rather than 0/0', () => {
    const none = network('NoValidators', { ...other, blocks: { knownSum: '0', expectedCount: 0, valuedCount: 0, staleCount: 0, state: 'complete' } })
    render(<ValidatorTotalCard networks={[none]} metric="blocks" />)
    const card = screen.getByRole('article', { name: 'Cumulative blocks' })
    expect(card.querySelector('[data-slot="summary-value"]')?.textContent).toBe('0')
    expect(card.querySelector('[data-slot="summary-caption"]')?.textContent).toBe('NoValidators · No eligible Validators')
  })

  it.each(['loading', 'unavailable'] as const)('keeps %s separate from an authoritative empty response', availability => {
    render(<ValidatorTotalCard networks={networks} metric="blocks" availability={availability} />)
    // The tile stays number-only and never fabricates a total; the Breakdown
    // states whether the summaries are still loading or unavailable.
    expect(screen.getByRole('article').querySelector('[data-slot="summary-value"]')?.textContent).toBe('Unknown')
    // The visible stat line states the availability instead of inventing coverage.
    // The scope is still named while coverage is pending or unavailable.
    expect(screen.getByRole('article').querySelector('[data-slot="summary-caption"]')?.textContent)
      .toBe('2 Networks · ' + (availability === 'loading' ? 'Loading coverage…' : 'Coverage unavailable'))
    fireEvent.click(screen.getByRole('button', { name: /breakdown/ }))
    const dialog = screen.getByRole('dialog')
    expect(dialog.textContent).toContain(
      availability === 'loading' ? 'Loading Validator summaries…' : 'Validator summaries unavailable.',
    )
    // The Breakdown states the same availability the face does: a pending or
    // failed response is never read as a zero denominator, so the explanation
    // cannot claim there are no eligible Validators while it says otherwise.
    expect(dialog.querySelector('[data-slot="validator-overview-scope"]')?.textContent)
      .toContain(availability === 'loading' ? 'Loading coverage…' : 'Coverage unavailable')
    expect(dialog.textContent).not.toContain('No eligible Validators')
  })

  it('follows selected Network scope without changing the source totals', () => {
    const { rerender } = render(<ValidatorTotalCard networks={networks} metric="blocks" />)
    rerender(<ValidatorTotalCard networks={[networks[1]]} metric="blocks" />)
    expect(screen.getByRole('article').querySelector('[data-slot="summary-value"]')?.textContent).toBe('7')
    fireEvent.click(screen.getByRole('button', { name: /breakdown/ }))
    const dialog = screen.getByRole('dialog')
    const scope = dialog.querySelector('[data-slot="validator-overview-scope"]')
    expect(scope?.textContent).toContain('Testnet')
    expect(scope?.textContent).toContain('1/1 reporting')
    expect(within(dialog).queryByRole('article', { name: 'Validator totals for Mainnet' })).toBeNull()
  })

  it('names the whole Network scope while a filter narrows the list (#245)', () => {
    render(<ValidatorTotalCard networks={networks} metric="blocks" listNarrowed />)
    const card = screen.getByRole('article', { name: 'Cumulative blocks' })
    // One short scope word on the face; the full explanation lives in the Breakdown.
    expect(card.querySelector('[data-slot="summary-caption"]')?.textContent).toBe('All of 2 Networks · 5/6 reporting · 1 stale')
    fireEvent.click(screen.getByRole('button', { name: /breakdown/ }))
    expect(screen.getByRole('dialog').querySelector('[data-slot="validator-overview-note"]')?.textContent)
      .toBe('This total covers every Active Node in the 2 selected Networks; search and the Health and Validator status filters do not narrow it.')
    cleanup()
    render(<ValidatorTotalCard networks={[networks[1]]} metric="rewards" listNarrowed />)
    expect(screen.getByRole('article').querySelector('[data-slot="summary-caption"]')?.textContent).toBe('All of Testnet · 1/1 reporting')
    fireEvent.click(screen.getByRole('button', { name: /breakdown/ }))
    expect(screen.getByRole('dialog').querySelector('[data-slot="validator-overview-note"]')?.textContent)
      .toBe('This total covers every Active Node in Testnet; search and the Health and Validator status filters do not narrow it.')
    cleanup()
    // Unnarrowed, the face keeps the plain scope and the Breakdown carries no note.
    render(<ValidatorTotalCard networks={[networks[1]]} metric="rewards" />)
    expect(screen.getByRole('article').querySelector('[data-slot="summary-caption"]')?.textContent).toBe('Testnet · 1/1 reporting')
    fireEvent.click(screen.getByRole('button', { name: /breakdown/ }))
    expect(screen.getByRole('dialog').querySelector('[data-slot="validator-overview-note"]')).toBeNull()
  })

  it('keeps the abbreviated reward tile while the Breakdown carries the exact amount', () => {
    const rich = network('Rich', { ...other, rewards: { ...other.rewards, knownSum: '1730000.5' } })
    render(<ValidatorTotalCard networks={[rich]} metric="rewards" />)
    const card = screen.getByRole('article', { name: 'Cumulative rewards' })
    expect(card.querySelector('[data-slot="summary-value"]')?.textContent).toBe('1.73M')
    fireEvent.click(screen.getByRole('button', { name: /breakdown/ }))
    const dialog = screen.getByRole('dialog')
    expect(dialog.querySelector('[data-slot="validator-summary-rewards"] dd')?.textContent).toBe('1,730,000.5')
  })

  it('shows a cumulative block count complete rather than abbreviated', () => {
    const large = network('Large', { ...other, blocks: { ...other.blocks, knownSum: '18446744073709551614' } })
    render(<ValidatorTotalCard networks={[large]} metric="blocks" />)
    expect(screen.getByRole('article', { name: 'Cumulative blocks' })
      .querySelector('[data-slot="summary-value"]')?.textContent).toBe('18,446,744,073,709,551,614')
    fireEvent.click(screen.getByRole('button', { name: /breakdown/ }))
    expect(within(screen.getByRole('dialog')).getAllByText('18,446,744,073,709,551,614')).toHaveLength(2)
  })
})

describe('exact cross-Network addition', () => {
  it.each([
    [['9007199254740993', '1'], '9007199254740994'],
    [['18446744073709551614', '18446744073709551614'], '36893488147419103228'],
    [['11.750000000001', '2', '0.000000000009'], '13.750000000010'],
    [['999.999', '.001'], '1000.000'],
    [['0.000', null], '0.000'],
    [[null, undefined, 'invalid'], null],
  ])('adds %j without binary floating point', (values, expected) => {
    expect(sumKnownAmounts(values)).toBe(expected)
  })
})
