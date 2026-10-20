import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import type { PublicRewardWallet } from '../api/generated'
import { RewardWalletCard, type RewardWalletReading } from './RewardWalletCard'

const ADDRESS = 'lat1zyctwgvw3ssagn722cneh6sz92y8j3dp8gqj5h'
const NOW = new Date('2026-10-20T03:10:00Z')

const wallet = (overrides: Partial<PublicRewardWallet> = {}): RewardWalletReading => ({
  status: 'ready',
  wallet: {
    address: ADDRESS,
    balance: '138179.543005858536',
    unit: 'LAT',
    state: 'current',
    lastGoodAt: '2026-10-20T03:09:30Z',
    lastAttemptAt: '2026-10-20T03:09:30Z',
    errorCategory: null,
    ...overrides,
  },
})

const card = () => screen.getByRole('article', { name: /^Balance/ })
const valueOf = () => card().querySelector('[data-slot="summary-value"]')?.textContent
const captionOf = () => card().querySelector('[data-slot="summary-caption"]')?.textContent
const titleOf = () => card().querySelector('[data-slot="summary-reading"]')?.getAttribute('title') ?? ''

afterEach(cleanup)

describe('RewardWalletCard', () => {
  it('reads a current balance with two grouped decimals and keeps the exact digits reachable', () => {
    render(<RewardWalletCard reading={wallet()} now={NOW} />)
    expect(valueOf()).toBe('138,179.54 LAT')
    expect(captionOf()).toBe('Mainnet · lat1zy…j5h')
    expect(titleOf()).toContain('138179.543005858536 LAT')
    expect(titleOf()).toContain(ADDRESS)
    expect(titleOf()).toContain('last updated')
    const name = card().getAttribute('aria-label') ?? ''
    expect(name).toContain('138179.543005858536 LAT')
    expect(name).toContain(ADDRESS)
    expect(name).toContain('last updated')
    expect(card().querySelector('[data-icon="Balance"]')).not.toBeNull()
    expect(card().getAttribute('data-tone')).toBe('green')
    // No link out and no Breakdown control.
    expect(card().querySelector('a, button')).toBeNull()
  })

  it('truncates rather than rounding up and pads short fractions', () => {
    render(<RewardWalletCard reading={wallet({ balance: '999.999' })} now={NOW} />)
    expect(valueOf()).toBe('999.99 LAT')
    cleanup()
    render(<RewardWalletCard reading={wallet({ balance: '1234567' })} now={NOW} />)
    expect(valueOf()).toBe('1,234,567.00 LAT')
  })

  it('keeps a stale last-good value and says how old it is and why', () => {
    render(<RewardWalletCard reading={wallet({ state: 'stale', lastGoodAt: '2026-10-20T03:05:00Z', errorCategory: 'upstream_unavailable' })} now={NOW} />)
    expect(valueOf()).toBe('138,179.54 LAT')
    expect(captionOf()).toBe('stale · 5m')
    expect(titleOf()).toContain('PlatScan unavailable')
    expect(titleOf()).toContain('last updated')
    cleanup()
    render(<RewardWalletCard reading={wallet({ state: 'stale', lastGoodAt: '2026-10-20T03:05:00Z', errorCategory: 'invalid_response' })} now={NOW} />)
    expect(titleOf()).toContain('PlatScan returned an invalid response')
  })

  it('never reads a balance it never had as zero', () => {
    render(<RewardWalletCard reading={wallet({ state: 'unavailable', balance: null, lastGoodAt: null, errorCategory: 'upstream_unavailable' })} now={NOW} />)
    expect(valueOf()).toBe('—')
    expect(captionOf()).toBe('Unavailable')
    expect(card().textContent).not.toMatch(/\b0\b/)
    expect(titleOf()).toContain('PlatScan unavailable')
  })

  it('distinguishes loading from a failed request', () => {
    render(<RewardWalletCard reading={{ status: 'loading' }} now={NOW} />)
    expect(valueOf()).toBe('—')
    expect(captionOf()).toBe('Loading…')
    cleanup()
    render(<RewardWalletCard reading={{ status: 'error' }} now={NOW} />)
    expect(valueOf()).toBe('—')
    expect(captionOf()).toBe('Unavailable')
  })
})
