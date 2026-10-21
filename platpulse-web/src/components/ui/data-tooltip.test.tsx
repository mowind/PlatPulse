import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { DataTooltip } from './data-tooltip'

afterEach(cleanup)

describe('DataTooltip', () => {
  it('renders children and opens tooltip on focus', () => {
    render(
      <DataTooltip content="Helper text" placement="bottom-end">
        <button type="button">Trigger</button>
      </DataTooltip>,
    )

    const button = screen.getByRole('button', { name: 'Trigger' })
    expect(screen.queryByRole('tooltip')).toBeNull()

    fireEvent.focus(button)
    const tooltip = screen.getByRole('tooltip')
    expect(tooltip).toBeDefined()
    expect(tooltip.textContent).toBe('Helper text')
    expect(tooltip.className).toContain('top-full right-0 mt-2')

    fireEvent.blur(button)
    expect(screen.queryByRole('tooltip')).toBeNull()
  })

  it('closes on Escape key press when open', () => {
    render(
      <DataTooltip content="Escape test">
        <button type="button">Trigger</button>
      </DataTooltip>,
    )

    const button = screen.getByRole('button', { name: 'Trigger' })
    fireEvent.focus(button)
    expect(screen.getByRole('tooltip')).toBeDefined()

    fireEvent.keyDown(document, { key: 'Escape' })
    expect(screen.queryByRole('tooltip')).toBeNull()
  })
})
