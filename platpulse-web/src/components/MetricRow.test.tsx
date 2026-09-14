import { cleanup, render } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { MetricRow } from './MetricRow'

afterEach(cleanup)

/**
 * The DOM contract shared by every Home, Node Detail, and Network metric row:
 * the data item, then the value, then an optional progress bar, then an
 * optional explanation. Slots are asserted by name so the row can be restyled
 * without changing the contract.
 */
const childSlots = (element: HTMLElement) => [...element.children].map((child) => child.getAttribute('data-slot'))

describe('MetricRow', () => {
  it('renders the data item then the value on one row', () => {
    const { container } = render(<MetricRow label="Head" value="120" />)
    const row = container.querySelector('[data-slot="metric-row"]') as HTMLElement
    expect(childSlots(row)).toEqual(['metric-row-label', 'metric-row-value'])
    expect(row.querySelector('[data-slot="progress-thin"]')).toBeNull()
    expect(row.querySelector('[data-slot="metric-row-detail"]')).toBeNull()
  })

  it('puts a full-width progress bar above the explanation', () => {
    const { container } = render(<MetricRow label="Node data" value="25.0%" detail="12.0 GiB / 48.0 GiB" progress={25} />)
    const row = container.querySelector('[data-slot="metric-row"]') as HTMLElement
    expect(childSlots(row)).toEqual(['metric-row-label', 'metric-row-value', 'progress-thin', 'metric-row-detail'])
    expect(row.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow')).toBe('25')
  })

  it('puts the explanation directly under the value when there is no progress', () => {
    const { container } = render(<MetricRow label="Peers" value="0" detail="Empty; authoritative zero" />)
    const row = container.querySelector('[data-slot="metric-row"]') as HTMLElement
    expect(childSlots(row)).toEqual(['metric-row-label', 'metric-row-value', 'metric-row-detail'])
  })

  it('clamps the progress bar and omits it when the value is absent', () => {
    const { container: over } = render(<MetricRow label="CPU" value="120%" progress={120} />)
    expect((over.querySelector('[role="progressbar"]') as HTMLElement).getAttribute('aria-valuenow')).toBe('100')
    // An unknown percentage must not render a track-only bar: that would read
    // as an authoritative zero.
    const { container: absent } = render(<MetricRow label="CPU" value="Unknown" progress={null} />)
    expect(absent.querySelector('[data-slot="progress-thin"]')).toBeNull()
    expect(absent.querySelector('[role="progressbar"]')).toBeNull()
  })
})
