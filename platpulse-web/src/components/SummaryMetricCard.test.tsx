import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { Server } from 'lucide-react'
import { SummaryMetricCard } from './SummaryMetricCard'
import { SURFACE_CARD, SURFACE_CARD_SUMMARY } from '../lib/surface'

afterEach(cleanup)

describe('SummaryMetricCard', () => {
  it('enables group and muted-to-foreground icon transition when hoverable is true', () => {
    render(<SummaryMetricCard label="Active Nodes" value={5} icon={Server} hoverable />)
    const card = screen.getByRole('article', { name: 'Active Nodes' })
    expect(card.classList.contains('group')).toBe(true)
    for (const token of SURFACE_CARD.split(' ')) {
      expect(card.classList.contains(token)).toBe(true)
    }

    const icon = card.querySelector('[data-icon="Active Nodes"]')
    expect(icon).not.toBeNull()
    const iconClass = icon?.getAttribute('class') ?? ''
    expect(iconClass).toContain('text-muted-foreground/30')
    expect(iconClass).toContain('group-hover:text-foreground')
    expect(iconClass).toContain('transition-colors')
    expect(iconClass).toContain('motion-reduce:transition-none')
  })

  it('keeps static styling without group and without group-hover when hoverable is false', () => {
    render(<SummaryMetricCard label="Head" value="1,234,567" icon={Server} hoverable={false} dataSlot="node-summary-tile" />)
    const card = screen.getByRole('article', { name: 'Head' })
    expect(card.classList.contains('group')).toBe(false)
    for (const token of SURFACE_CARD_SUMMARY.split(' ')) {
      expect(card.classList.contains(token)).toBe(true)
    }

    const icon = card.querySelector('[data-icon="Head"]')
    expect(icon).not.toBeNull()
    const iconClass = icon?.getAttribute('class') ?? ''
    expect(iconClass).not.toContain('group-hover:text-foreground')
    expect(iconClass).toContain('text-muted-foreground')
  })

  it('mounts interactive action directly in summary-marker without icon dimming wrappers', () => {
    render(
      <SummaryMetricCard
        label="Breakdown Card"
        value={100}
        hoverable
        action={<button type="button" aria-label="Open breakdown">Breakdown</button>}
      />
    )
    const marker = screen.getByRole('article', { name: 'Breakdown Card' }).querySelector('[data-slot="summary-marker"]')
    expect(marker).not.toBeNull()
    const actionBtn = marker?.querySelector('button')
    expect(actionBtn).not.toBeNull()
    expect(actionBtn?.textContent).toBe('Breakdown')
    expect(marker?.querySelector('[data-icon]')).toBeNull()
  })

  it('renders value and unit with proper typographic hierarchy and accessible text', () => {
    render(<SummaryMetricCard label="Balance" value="138,179.54" unit="LAT" />)
    const card = screen.getByRole('article', { name: 'Balance' })
    const valueEl = card.querySelector('[data-slot="summary-value"]')
    expect(valueEl).not.toBeNull()
    expect(valueEl?.classList.contains('tabular-nums')).toBe(true)
    expect(valueEl?.classList.contains('font-bold')).toBe(true)
    expect(valueEl?.classList.contains('md:text-2xl')).toBe(true)

    const unitEl = card.querySelector('[data-slot="summary-unit"]')
    expect(unitEl).not.toBeNull()
    expect(unitEl?.textContent).toBe('LAT')
    expect(unitEl?.classList.contains('text-muted-foreground')).toBe(true)
    expect(unitEl?.classList.contains('font-normal')).toBe(true)
    expect(unitEl?.classList.contains('text-xs')).toBe(true)

    // Full text includes both value and unit with a space
    expect(valueEl?.textContent).toBe('138,179.54 LAT')
  })

  it('handles numeric zero unit without rendering stray falsy strings', () => {
    render(<SummaryMetricCard label="Count" value={42} unit={0} />)
    const card = screen.getByRole('article', { name: 'Count' })
    const unitEl = card.querySelector('[data-slot="summary-unit"]')
    expect(unitEl?.textContent).toBe('0')
    expect(card.querySelector('[data-slot="summary-value"]')?.textContent).toBe('42 0')
  })

  it('renders tone, caption, hint and accessibleName correctly', () => {
    render(
      <SummaryMetricCard
        label="Attention"
        accessibleName="Attention: 2 Unhealthy"
        value={2}
        tone="red"
        caption="Unhealthy or Unknown"
        hint="Attention hint"
      />
    )
    const card = screen.getByRole('article', { name: 'Attention: 2 Unhealthy' })
    const valueEl = card.querySelector('[data-slot="summary-value"]')
    expect(valueEl?.classList.contains('text-destructive')).toBe(true)
    const captionEl = card.querySelector('[data-slot="summary-caption"]')
    expect(captionEl?.textContent).toBe('Unhealthy or Unknown')
    const reading = card.querySelector('[data-slot="summary-reading"]')
    expect(reading?.getAttribute('title')).toBe('Attention hint')
  })
})
