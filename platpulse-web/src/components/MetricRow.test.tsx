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
  it('wraps noncompact captions naturally but retains compact truncation', () => {
    const { container, rerender } = render(<MetricRow label="Directory" value="Unknown" detail="A long explanation" />)
    const caption = () => container.querySelector('[data-slot="metric-row-detail"]')!
    expect(caption().className).toContain('whitespace-normal')
    expect(caption().className).not.toContain('truncate')
    expect(caption().className).not.toContain('line-clamp')
    rerender(<MetricRow layout="compact" label="Directory" value="Unknown" detail="A long explanation" />)
    expect(caption().className).toContain('truncate')
  })
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
    expect(absent.querySelector('[data-slot="metric-unknown-space"]')).not.toBeNull()
  })

  it('renders a compact key-value row without a two-line label reservation', () => {
    const { container } = render(<MetricRow layout="compact" label="Committed" value="159,317,988" />)
    const row = container.querySelector('[data-slot="metric-row"]') as HTMLElement
    expect(row.getAttribute('data-layout')).toBe('compact')
    expect(childSlots(row)).toEqual(['metric-row-label', 'metric-row-value'])
    expect(row.querySelector('[data-slot="metric-row-label"]')?.className).toContain('whitespace-nowrap')
    expect(row.querySelector('[data-slot="metric-row-value"]')?.className).toContain('whitespace-nowrap')
  })

  it('renders a wide value that absorbs the caption it supersedes', () => {
    const { container } = render(
      <MetricRow
        layout="compact"
        label="Node data"
        value="8.8%"
        wideValue="310 GiB / 3.43 TiB · 8.8%"
        detail="310 GiB / 3.43 TiB"
        progress={8.8}
      />,
    )
    const row = container.querySelector('[data-slot="metric-row"]') as HTMLElement
    expect(row.querySelector('[data-value-narrow]')?.textContent).toBe('8.8%')
    expect(row.querySelector('[data-value-wide]')?.textContent).toBe('310 GiB / 3.43 TiB · 8.8%')
    expect(row.querySelector('[data-slot="metric-row-detail"]')?.getAttribute('data-hide-wide')).toBe('true')
    expect(row.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow')).toBe('9')
  })
})
