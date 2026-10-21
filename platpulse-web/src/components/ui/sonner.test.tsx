import { act, cleanup, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { type ThemeMode } from '../../theme'
import { Toaster, toast } from './sonner'

const theme = vi.hoisted(() => ({ mode: 'auto' as ThemeMode }))
vi.mock('../../theme/ThemeProvider', () => ({
  useTheme: () => ({ mode: theme.mode }),
}))

beforeEach(() => {
  vi.stubGlobal(
    'matchMedia',
    vi.fn((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  )
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('Toaster (sonner)', () => {
  it.each([
    ['auto', 'light'],
    ['light', 'light'],
    ['dark', 'dark'],
  ] as const)('maps theme mode %s and renders toast', async (appMode, expectedActualTheme) => {
    theme.mode = appMode
    const { container } = render(<Toaster richColors closeButton position="top-center" />)
    
    act(() => {
      toast.success('Test message ' + appMode)
    })

    const toastText = await screen.findByText('Test message ' + appMode)
    expect(toastText).toBeDefined()

    const toasterEl = container.querySelector('[data-sonner-toaster]') as HTMLElement
    expect(toasterEl).not.toBeNull()
    expect(toasterEl.getAttribute('data-sonner-theme')).toBe(expectedActualTheme)
    expect(toasterEl.style.getPropertyValue('--normal-bg')).toBe('var(--popover)')
    expect(toasterEl.style.getPropertyValue('--normal-text')).toBe('var(--popover-foreground)')
    expect(toasterEl.style.getPropertyValue('--normal-border')).toBe('var(--border)')
    expect(toasterEl.style.getPropertyValue('--border-radius')).toBe('var(--radius)')
  })
})
