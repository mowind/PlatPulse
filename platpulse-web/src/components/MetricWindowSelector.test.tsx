import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { useState } from 'react'
import { afterEach, describe, expect, it } from 'vitest'
import type { MetricWindow } from '../metricWindow'
import { MetricWindowSelector } from './MetricWindowSelector'

afterEach(cleanup)

function Harness({ initial = 'live' }: { initial?: MetricWindow }) {
  const [value, setValue] = useState<MetricWindow>(initial)
  return <MetricWindowSelector value={value} onChange={setValue} />
}

describe('MetricWindowSelector', () => {
  it('is one tab stop holding the five presets in order', () => {
    render(<Harness />)
    const group = screen.getByRole('radiogroup', { name: 'Metric window' })
    const radios = screen.getAllByRole('radio')
    expect(group).toBeTruthy()
    expect(radios.map((radio) => radio.textContent)).toEqual(['Live', '1h', '6h', '24h', '7d'])
    expect(radios.map((radio) => radio.tabIndex)).toEqual([0, -1, -1, -1, -1])
    expect(radios.map((radio) => radio.getAttribute('aria-checked'))).toEqual(['true', 'false', 'false', 'false', 'false'])
    // Every segment is a 44px target.
    for (const radio of radios) expect(radio.className).toMatch(/min-h-11 min-w-11/)
  })

  it('moves the selection and the focus together with the arrow keys, wrapping at both ends', () => {
    render(<Harness />)
    const radio = (name: string) => screen.getByRole('radio', { name })
    fireEvent.keyDown(radio('Live'), { key: 'ArrowRight' })
    expect(radio('1h').getAttribute('aria-checked')).toBe('true')
    expect(document.activeElement).toBe(radio('1h'))
    expect(radio('1h').tabIndex).toBe(0)
    expect(radio('Live').tabIndex).toBe(-1)
    fireEvent.keyDown(radio('1h'), { key: 'ArrowDown' })
    expect(radio('6h').getAttribute('aria-checked')).toBe('true')
    fireEvent.keyDown(radio('6h'), { key: 'ArrowLeft' })
    fireEvent.keyDown(radio('1h'), { key: 'ArrowUp' })
    expect(radio('Live').getAttribute('aria-checked')).toBe('true')
    fireEvent.keyDown(radio('Live'), { key: 'ArrowLeft' })
    expect(radio('7d').getAttribute('aria-checked')).toBe('true')
    expect(document.activeElement).toBe(radio('7d'))
    fireEvent.keyDown(radio('7d'), { key: 'ArrowRight' })
    expect(radio('Live').getAttribute('aria-checked')).toBe('true')
    // Other keys do nothing.
    fireEvent.keyDown(radio('Live'), { key: 'a' })
    expect(radio('Live').getAttribute('aria-checked')).toBe('true')
  })

  it('selects a segment by pointer', () => {
    render(<Harness initial="6h" />)
    fireEvent.click(screen.getByRole('radio', { name: '24h' }))
    expect(screen.getByRole('radio', { name: '24h' }).getAttribute('aria-checked')).toBe('true')
    expect(screen.getByRole('radio', { name: '6h' }).getAttribute('aria-checked')).toBe('false')
  })
})
