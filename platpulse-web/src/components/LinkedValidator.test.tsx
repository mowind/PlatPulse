import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { PublicNode, PublicValidatorInsight } from '../api/generated'
import { LinkedValidatorSection, currentValidatorStatusLabel, validatorDataStatus, validatorStateLabel } from './LinkedValidator'

const node: PublicNode = {
  nodeId: 'node-1',
  displayName: 'Node One',
  peerCountries: { scope: 'unavailable', countries: [] },
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
const detailRegion = () => screen.getByRole('region', { name: 'Validator performance' })

/** The rendered value of a Linked Validator summary cell or Node detail metric. */
const metricValue = (label: string) => {
  const labelNode = screen.getByText(label, { exact: true })
  const cell = labelNode.closest('[data-slot="metric-row"], [data-slot="validator-summary-cell"]')
  return cell?.querySelector('strong')?.textContent ?? undefined
}

/** The Home summary cell carrying a full metric name, whether or not the compact
 *  cell currently renders its short label instead. */
const summaryCell = (label: string) => {
  const full = [...document.querySelectorAll('[data-slot="validator-summary-label"] [data-full-label]')]
    .find(node => node.textContent === label)
  return full?.closest('[data-slot="validator-summary-cell"]') as HTMLElement | undefined
}
const summaryValue = (label: string) => summaryCell(label)?.querySelector('strong')
const summaryTitle = (label: string) => summaryValue(label)?.querySelector('[title]')?.getAttribute('title')
/** The one short identity line the Home card prints when no three-metric summary
 *  is confirmed. */
const identityLine = () => screen.getByRole('status')

describe('LinkedValidatorSection Home card', () => {
  it('shows only the three summary metrics a confirmed Validator owns', () => {
    renderCard()
    const group = screen.getByRole('group', { name: 'Linked Validator metrics' })
    expect(group.getAttribute('data-slot')).toBe('linked-validator-metrics')
    const cells = [...group.querySelectorAll('[data-slot="validator-summary-cell"]')]
    expect(cells.map(cell => cell.getAttribute('data-metric'))).toEqual(['rank', 'blocks', 'rewards'])
    expect(metricValue('Network rank')).toBe('#7')
    expect(metricValue('Cumulative blocks')).toBe('4,321')
    expect(metricValue('Cumulative rewards')).toBe('1.23K')
    // The three metrics Home retired from the card stay on Node Detail.
    expect(screen.queryByText('Production rate')).toBeNull()
    expect(screen.queryByText('PlatScan 24h rate')).toBeNull()
    expect(screen.queryByText('Delegation reward share')).toBeNull()
  })

  it('keeps the short cell label under its full accessible metric name', () => {
    renderCard()
    const cells = [...document.querySelectorAll('[data-slot="validator-summary-cell"]')]
    expect(cells).toHaveLength(3)
    for (const cell of cells) {
      expect(cell.querySelector('[data-slot="validator-summary-value"]')?.className).toContain('tabular-nums')
      const full = cell.querySelector('[data-full-label]')!
      const short = cell.querySelector('[data-short-label]')!
      expect(short.textContent).toBeTruthy()
      expect(short.getAttribute('aria-label')).toBe(full.textContent)
      expect(short.getAttribute('title')).toBe(full.textContent)
      expect(cell.className).not.toMatch(/bg-|border-|break-all/)
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
    expect(metricValue('Cumulative blocks')).toBe('4,321')
  })

  it('keeps a source-reported zero distinct from a value the Server never attested', () => {
    renderCard({ validator: { ...insight, blockCount: 0 } })
    expect(metricValue('Cumulative blocks')).toBe('0')
    cleanup()
    renderCard({ validator: { ...insight, blockCount: null } })
    const value = summaryValue('Cumulative blocks')
    expect(value?.textContent).toMatch(/^Unknown:/)
    // The reason is announced rather than silently dropped.
    expect(value?.querySelector('span[aria-describedby]')).toBeTruthy()
  })

  it('abbreviates the gross lifetime rewards and keeps the exact digits reachable', () => {
    renderCard({ validator: { ...insight, rewardAmount: '1234567.89' } })
    expect(metricValue('Cumulative rewards')).toBe('1.23M')
    expect(summaryTitle('Cumulative rewards')).toBe('1,234,567.89')
    cleanup()
    renderCard({ validator: { ...insight, rewardAmount: '0' } })
    expect(metricValue('Cumulative rewards')).toBe('0')
    cleanup()
    renderCard({ validator: { ...insight, rewardAmount: null } })
    expect(summaryValue('Cumulative rewards')?.textContent).toMatch(/^Unknown:/)
  })

  it('shows the Network rank and keeps unranked distinct from failure or unknown', () => {
    renderCard()
    expect(metricValue('Network rank')).toBe('#7')
    cleanup()
    // Unranked is a ranking conclusion, not a value: the cell keeps it and says
    // which criterion produced it.
    renderCard({ validator: { ...insight, rank: null, rankState: 'unranked' } })
    expect(metricValue('Network rank')).toBe('Unranked')
    expect(summaryTitle('Network rank')).toMatch(/complete live-staking ALL cohort/)
    expect(screen.getByRole('group', { name: 'Linked Validator metrics' })).toBeTruthy()
    cleanup()
    // A ranking failure keeps the last-good rank when one exists…
    renderCard({ validator: { ...insight, rankState: 'error' } })
    expect(metricValue('Network rank')).toBe('#7')
    expect(summaryTitle('Network rank')).toMatch(/last successful rank is retained/)
    cleanup()
    // …and a failure with no last-good rank is Unknown, never a placeholder rank.
    renderCard({ validator: { ...insight, rank: null, rankState: 'error' } })
    expect(summaryValue('Network rank')?.textContent).toMatch(/^Unknown:/)
  })

  it('retains the last-good summary values after a Provider failure', () => {
    renderCard({ validator: { ...insight, state: 'error', freshness: 'stale' } })
    expect(screen.getByRole('group', { name: 'Linked Validator metrics' })).toBeTruthy()
    expect(metricValue('Network rank')).toBe('#7')
    expect(metricValue('Cumulative blocks')).toBe('4,321')
    expect(metricValue('Cumulative rewards')).toBe('1.23K')
  })

  it('replaces the summary with the one short word the confirmed status allows', () => {
    renderCard({ validator: { ...insight, currentValidatorStatus: 'not_validator', currentValidatorStatusState: 'current', activity: 'observing' } })
    expect(identityLine().textContent).toMatch(/^Non-validator/)
    expect(screen.queryByRole('group', { name: 'Linked Validator metrics' })).toBeNull()
    cleanup()
    renderCard({ validator: { ...insight, currentValidatorStatus: 'unknown' } })
    expect(identityLine().textContent).toMatch(/^Validator unknown/)
    expect(screen.queryByRole('group', { name: 'Linked Validator metrics' })).toBeNull()
    expect(screen.queryByText('Non-validator')).toBeNull()
  })

  it('never infers the identity from activity, consensus membership or a missing metric', () => {
    // Producing activity, a consensus flag and retained values are not a
    // confirmed staking verdict: an unconfirmed status stays Validator unknown.
    renderCard({ validator: { ...insight, currentValidatorStatus: 'unknown' } })
    expect(screen.queryByRole('group', { name: 'Linked Validator metrics' })).toBeNull()
    expect(identityLine().textContent).toMatch(/^Validator unknown/)
    cleanup()
    // …and a confirmed negative is not softened by activity or metrics either.
    renderCard({ validator: { ...insight, currentValidatorStatus: 'not_validator', currentValidatorStatusState: 'current', activity: 'producing' } })
    expect(identityLine().textContent).toMatch(/^Non-validator/)
    expect(screen.queryByRole('group', { name: 'Linked Validator metrics' })).toBeNull()
  })
})

describe('LinkedValidatorSection Home identity states', () => {
  const missing: PublicValidatorInsight = {
    ...insight, blockCount: null, rewardAmount: null, rank: null, rankState: 'unknown',
    blockRate: null, blockRateState: 'unknown', expectedBlockCount: null,
    genBlocksRate: null, delegationRewardPercentage: null,
  }
  const notValidator: PublicValidatorInsight = {
    ...missing, currentValidatorStatus: 'not_validator', currentValidatorStatusState: 'current',
  }

  it('states a confirmed non-Validator in one short word', () => {
    renderCard({ validator: { ...notValidator, state: 'empty', freshness: 'fresh' } })
    const line = identityLine()
    // One word on the card face (#238 round 2): the reason stops repeating
    // itself in the footer and moves to the line's own title, where it stays
    // reachable without crowding the identity row.
    expect(line.textContent).toBe('Non-validator')
    expect(line.getAttribute('title')).toBe('No current staking identity.')
    expect(line.querySelector('[data-slot="linked-validator-identity-reason"]')).toBeNull()
    expect(line.closest('[data-slot="linked-validator"]')?.getAttribute('data-identity')).toBe('not_validator')
    expect(screen.queryByRole('group', { name: 'Linked Validator metrics' })).toBeNull()
    // No six Unknown placeholders and no large empty state take the space back.
    expect(screen.queryByText('No validator metrics')).toBeNull()
    expect(screen.queryAllByText('Unknown')).toHaveLength(0)
  })

  it('states the one word a confirmed non-Validator has in the footer\'s own muted tone', () => {
    renderCard({ validator: { ...notValidator, state: 'empty', freshness: 'fresh' } })
    const label = identityLine().querySelector('[data-slot="linked-validator-identity-label"]') as HTMLElement
    // The card has no metric to weigh and no verdict to give, so the one word it
    // can say is the quietest reading in the footer rather than a value: it
    // takes the muted tone the reason beside it uses. An unconfirmed identity
    // keeps the foreground tone, because it is not a settled negative.
    expect(label.className).toContain('text-muted-foreground')
    cleanup()
    renderCard({ validator: { ...insight, currentValidatorStatus: 'unknown' } })
    const unconfirmed = identityLine().querySelector('[data-slot="linked-validator-identity-label"]') as HTMLElement
    expect(unconfirmed.className).toContain('text-foreground')
    expect(unconfirmed.className).not.toContain('text-muted-foreground')
  })

  it('marks a retained non-Validator verdict as retained', () => {
    renderCard({ validator: { ...notValidator, currentValidatorStatusState: 'stale' } })
    expect(identityLine().textContent).toBe('Validator unknown — Confirmed non-Validator status retained.')
  })

  it.each([
    ['loading', { state: 'loading', freshness: 'unknown' }],
    ['error', { state: 'error', freshness: 'stale' }],
    ['not_configured', { state: 'not_configured', freshness: 'unknown' }],
    ['unsupported', { state: 'unsupported', freshness: 'unknown' }],
    ['not_found', { state: 'not_found', freshness: 'unknown' }],
    ['empty', { state: 'empty', freshness: 'fresh' }],
  ] as const)('never reads the %s Provider state as a negative identity', (_state, override) => {
    renderCard({ validator: { ...missing, ...override, currentValidatorStatus: 'unknown', currentValidatorStatusState: 'unknown' } })
    expect(identityLine().textContent).toBe('Validator unknown — Staking validity is not established.')
    expect(screen.queryByText('Non-validator')).toBeNull()
    expect(screen.queryByText('No validator metrics')).toBeNull()
    expect(screen.queryByRole('group', { name: 'Linked Validator metrics' })).toBeNull()
  })

  it('explains a missing Link without claiming absent staking', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: null }} />)
    const line = identityLine()
    expect(line.textContent).toBe('Validator unknown — Identity has not been observed yet.')
    expect(line.getAttribute('aria-label')).toBe('Validator identity state: not observed')
    expect(screen.queryByText('Linked Validator')).toBeNull()
    expect(screen.queryByRole('group', { name: 'Linked Validator metrics' })).toBeNull()
  })

  it('keeps a long Server identity explanation on Node detail, not on the Home card', () => {
    const longReason = 'The observed Network Identity does not match the registered Network tuple, so no automatic correspondence was established.'
    render(<LinkedValidatorSection node={{ ...node, validator: null, validatorIdentityState: 'network_identity_mismatch', validatorIdentityReason: longReason }} />)
    expect(screen.queryByText(longReason)).toBeNull()
    expect(screen.getByText('Identity has not been established.')).toBeTruthy()
    cleanup()
    render(<LinkedValidatorSection node={{ ...node, validator: null, validatorIdentityState: 'network_identity_mismatch', validatorIdentityReason: longReason }} variant="detail" />)
    expect(screen.getByText(longReason)).toBeTruthy()
  })

  it('distinguishes identified data unavailability from unobserved discovery', () => {
    render(<LinkedValidatorSection node={{ ...node, validator: null, validatorIdentityState: 'identified' }} />)
    expect(identityLine().textContent).toBe('Validator unknown — Validator data is unavailable.')
    expect(identityLine().getAttribute('aria-label')).toBe('Validator identity state: identified')
  })

  it.each(['missing_public_key', 'invalid_public_key', 'network_identity_missing', 'network_identity_mismatch'])('preserves canonical identity state access: %s', validatorIdentityState => {
    render(<LinkedValidatorSection node={{ ...node, validator: null, validatorIdentityState }} />)
    expect(identityLine().textContent).toBe('Validator unknown — Identity has not been established.')
    expect(identityLine().getAttribute('aria-label')).toBe('Validator identity state: ' + validatorIdentityState)
  })
})

describe('LinkedValidatorSection Node detail', () => {
  it('keeps the identity, full identifier, copy control and all six metrics', async () => {
    const identifier = '0X' + 'AbCdEf0123456789'.repeat(8)
    const writeText = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('navigator', { clipboard: { writeText } })
    renderDetail({ validator: { ...insight, validatorNodeId: identifier } })
    const region = detailRegion()
    expect(within(region).getByRole('heading', { name: 'Validator performance' })).toBeTruthy()
    expect(within(region).getByText('Validator One').closest('details')).toBe(region.querySelector('details'))
    expect(within(region).queryByText(identifier, { exact: true })).toBeNull()
    fireEvent.click(within(region).getByRole('button', { name: 'Show full ID' }))
    expect(within(region).getByText(identifier, { exact: true })).toBeTruthy()
    expect(region.querySelector('details')?.open).toBe(false)
    expect(region.querySelectorAll('[data-slot="metric-row"]')).toHaveLength(6)
    fireEvent.click(within(region).getByRole('button', { name: 'Copy full Validator identifier' }))
    await waitFor(() => expect(within(region).getByRole('status', { name: 'Identifier copy status' }).textContent).toBe('Validator identifier copied.'))
    expect(writeText).toHaveBeenCalledExactlyOnceWith(identifier)
  })

  it.each(['unavailable', 'rejected'])('reveals the selectable full identifier when clipboard is %s', async failure => {
    const identifier = '0x' + 'aBcD'.repeat(32)
    vi.stubGlobal('navigator', failure === 'unavailable' ? {} : { clipboard: { writeText: vi.fn().mockRejectedValue(new Error('Denied')) } })
    renderDetail({ validator: { ...insight, validatorNodeId: identifier } })
    fireEvent.click(screen.getByRole('button', { name: 'Copy full Validator identifier' }))
    await waitFor(() => expect(screen.getByRole('status', { name: 'Identifier copy status' }).textContent).toMatch(/Copy failed.*manually/))
    expect(screen.getByText(identifier, { exact: true })).toBeTruthy()
  })

  it('leaves routine identity statuses to the page header and keeps one compact responsive grid', () => {
    renderDetail({ validator: { ...insight, currentValidatorStatusQualifier: 'locked', activity: 'verifying' } })
    const region = detailRegion()
    const header = region.querySelector('header')!
    expect(header.textContent).toBe('Validator performance')
    expect(within(region).queryByText('Validator', { exact: true })).toBeNull()
    expect(within(region).queryByText('Current', { exact: true })).toBeNull()
    expect(within(region).queryByText('Locked', { exact: true })).toBeNull()
    expect(within(region).queryByLabelText(/PlatScan status:/)).toBeNull()
    expect(region.className).not.toMatch(/hover:|transition/)
    const metrics = screen.getByRole('group', { name: 'Validator performance metrics' })
    expect(metrics.className).toContain('grid-cols-2')
    expect(metrics.className).toContain('lg:grid-cols-3')
    expect(metrics.querySelectorAll('[data-slot="metric-row"]')).toHaveLength(6)
    expect(region.querySelectorAll('[data-slot="card"]')).toHaveLength(0)
    const diagnostics = region.querySelector('details')!
    expect(within(diagnostics).getByText('Staking qualifier').nextElementSibling?.textContent).toBe('locked')
    expect(within(diagnostics).getByText('Activity').nextElementSibling?.textContent).toBe('verifying')
  })

  it('lets long numbers span the row without clipping or losing source precision', () => {
    const rewards = '123456789012345678901234567890.123456789'
    const rate = '123456789012345678901234567890.123456789'
    renderDetail({ validator: { ...insight, blockCount: Number.MAX_SAFE_INTEGER, rewardAmount: rewards, genBlocksRate: rate } })
    const row = (label: string) => screen.getByText(label, { exact: true }).closest('[data-slot="metric-row"]')!
    expect(row('Cumulative blocks').className).toContain('col-span-full sm:col-span-1')
    expect(row('Cumulative rewards').className).toContain('col-span-full')
    expect(row('Cumulative rewards').className).not.toContain('sm:col-span-1')
    expect(row('PlatScan 24h rate').className).toContain('col-span-full')
    expect(metricValue('Cumulative blocks')).toBe(Number.MAX_SAFE_INTEGER.toLocaleString())
    expect(metricValue('Cumulative rewards')).toBe('123,456,789,012,345,678,901,234,567,890.1234')
    expect(metricValue('PlatScan 24h rate')).toBe(rate + '%')
    expect(screen.getByText('Cumulative rewards (full precision)').nextElementSibling?.textContent).toBe('123,456,789,012,345,678,901,234,567,890.123456789')
  })

  it('defaults diagnostics closed and toggles the full ID independently', () => {
    renderDetail({ validator: { ...insight, validatorNodeId: '0x' + 'a'.repeat(128) } })
    const diagnostics = detailRegion().querySelector('details')!
    expect(diagnostics.open).toBe(false)
    expect(diagnostics.querySelector('summary')?.textContent).toContain('Validator diagnostics')
    expect(diagnostics.querySelector('summary')?.textContent).toContain('Provider, freshness, ranking and source details')
    expect(within(diagnostics).getByLabelText('Public Validator states')).toBeTruthy()
    expect(within(diagnostics).getByText('Last success')).toBeTruthy()
    const show = screen.getByRole('button', { name: 'Show full ID' })
    expect(show.getAttribute('aria-expanded')).toBe('false')
    fireEvent.click(show)
    const hide = screen.getByRole('button', { name: 'Hide full ID' })
    expect(hide.getAttribute('aria-expanded')).toBe('true')
    expect(document.getElementById(hide.getAttribute('aria-controls')!)?.className).toContain('select-text')
    expect(diagnostics.open).toBe(false)
    fireEvent.click(hide)
    expect(screen.queryByLabelText(/Validator identifier:/)).toBeNull()
  })

  it('keeps simultaneous Provider, staking, identity, ranking, rate and counter warnings outside diagnostics', () => {
    renderDetail({ validatorIdentityReason: 'Identity discovery failed.', validator: { ...insight,
      state: 'error', freshness: 'stale', currentValidatorStatus: 'unknown', currentValidatorStatusState: 'stale',
      rankState: 'error', rankFreshness: 'stale', blockRateState: 'unknown', blockRate: null, counterState: 'counter_reset' } })
    const reasons = [/source could not be read.*retained and stale/, /last established association/,
      /not a negative conclusion/, /last confirmed Validator status/, /last successful rank is retained/,
      /complete cumulative produced.*scheduled block pair/, /prior value was not treated as normal growth/]
    for (const reason of reasons) {
      const note = screen.getByText(reason)
      expect(note.closest('details')).toBeNull()
    }
    expect(screen.getByText(/last successful rank is retained/).closest('[data-slot=metric-row]')?.textContent).toContain('Network rank')
    expect(screen.getByText(/production rate is unknown/).closest('[data-slot=metric-row]')?.textContent).toContain('Production rate')
    expect(screen.getByText(/prior value was not treated as normal growth/).closest('[data-slot=metric-row]')?.textContent).toContain('Cumulative blocks')
    expect(detailRegion().querySelector('details')?.open).toBe(false)
  })

  it.each(['not_configured', 'unsupported', 'not_found', 'empty'])('labels retained metrics with unavailable Provider state %s outside diagnostics', state => {
    renderDetail({ validator: { ...insight, state, freshness: 'unknown' } })
    const note = screen.getByText(/Showing the last successful metrics; the current source state is unavailable/)
    expect(note.closest('details')).toBeNull()
    expect(metricValue('Cumulative blocks')).toBe('4,321')
    expect(metricValue('Cumulative rewards')).toBe('1,234.1234')
  })

  it('does not repeat the same unconfigured Provider reason for ranking', () => {
    renderDetail({ validator: { ...insight, state: 'not_configured', rankState: 'not_configured' } })
    expect(screen.getAllByText(/No Validator source is configured/)).toHaveLength(1)
  })

  it('limits overview rewards to four decimals and preserves exact source digits in the title', () => {
    renderDetail({ validator: { ...insight, rewardAmount: '1234567.890123456789', blockRate: '125.123456', genBlocksRate: '135.987654', delegationRewardPercentage: '20.123456' } })
    expect(metricValue('Cumulative rewards')).toBe('1,234,567.8901')
    expect(screen.getByTitle('1,234,567.890123456789')).toBeTruthy()
    const exactReward = screen.getByText('Cumulative rewards (full precision)').nextElementSibling
    expect(exactReward?.textContent).toBe('1,234,567.890123456789')
    expect(exactReward?.closest('details')).toBe(detailRegion().querySelector('details'))
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
    expect(screen.getByText(/source could not be read; the last successful values are retained/)).toBeTruthy()
  })

  it('keeps the locked qualifier and the unknown-staking explanation', () => {
    renderDetail({ validator: { ...insight, currentValidatorStatus: 'validator', currentValidatorStatusQualifier: 'locked' } })
    expect(screen.queryByText('Locked')).toBeNull()
    expect(screen.getByText(/confirmed-valid staking identity but is locked/).closest('details')).toBe(detailRegion().querySelector('details'))
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
