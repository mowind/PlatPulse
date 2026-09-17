import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { themeToggleLabel, type ThemeMode } from '../theme'
import ThemeToggle from './ThemeToggle'

const theme = vi.hoisted(() => ({ mode: 'auto' as ThemeMode, cycleTheme: vi.fn() }))
vi.mock('../theme/ThemeProvider', () => ({
  useTheme: () => ({ ...theme, toggleLabel: themeToggleLabel(theme.mode) }),
}))
afterEach(() => { cleanup(); vi.clearAllMocks() })

describe('Emerald-style theme toggle', () => {
  it.each(['auto', 'light', 'dark'] as const)('renders an icon-only accessible %s control', (mode) => {
    theme.mode = mode
    render(<ThemeToggle />)
    const button = screen.getByRole('button', { name: themeToggleLabel(mode) })
    expect(button.textContent).toBe('')
    expect(button.getAttribute('title')).toBe(themeToggleLabel(mode))
    const icon = button.querySelector('svg')
    expect(icon?.getAttribute('aria-hidden')).toBe('true')
    expect(icon?.getAttribute('width')).toBe('18')
    expect(icon?.getAttribute('height')).toBe('18')
    expect(icon?.getAttribute('viewBox')).toBe('0 0 24 24')
    expect(icon?.getAttribute('stroke-width')).toBe('2')
    expect(icon?.getAttribute('data-icon')).toBe(mode === 'auto' ? 'dark-mode' : mode === 'light' ? 'sun-one' : 'moon')
    fireEvent.click(button)
    expect(theme.cycleTheme).toHaveBeenCalledOnce()
  })
})
