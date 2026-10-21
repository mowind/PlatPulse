import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { BackTop } from './back-top'

beforeEach(() => {
  window.scrollY = 0
  window.scrollTo = vi.fn()
  vi.stubGlobal('matchMedia', vi.fn((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })))
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('BackTop component', () => {
  it('is hidden and disabled when scrollY is below threshold', () => {
    window.scrollY = 100
    render(<BackTop visibilityHeight={200} />)

    const button = screen.getByRole('button', { hidden: true })
    expect(button.getAttribute('aria-label')).toBe('Back to top')
    expect(button.getAttribute('aria-hidden')).toBe('true')
    expect(button.getAttribute('tabindex')).toBe('-1')
    expect(button.hasAttribute('disabled')).toBe(true)
    expect(button.className).toContain('invisible')
    expect(button.className).toContain('pointer-events-none')
    expect(button.className).toContain('size-11')
  })

  it('becomes visible and enabled when scrollY exceeds threshold', () => {
    window.scrollY = 300
    render(<BackTop visibilityHeight={200} />)

    const button = screen.getByRole('button', { name: 'Back to top' })
    expect(button.getAttribute('aria-hidden')).toBe('false')
    expect(button.getAttribute('tabindex')).toBe('0')
    expect(button.hasAttribute('disabled')).toBe(false)
    expect(button.className).toContain('visible')
    expect(button.className).toContain('pointer-events-auto')
  })

  it('scrolls smoothly to top on click under normal motion preferences', () => {
    window.scrollY = 400
    render(<BackTop visibilityHeight={200} />)

    const button = screen.getByRole('button', { name: 'Back to top' })
    fireEvent.click(button)

    expect(window.scrollTo).toHaveBeenCalledWith({ top: 0, behavior: 'smooth' })
  })

  it('scrolls with auto behavior when prefers-reduced-motion is active', () => {
    vi.stubGlobal('matchMedia', vi.fn((query: string) => ({
      matches: query.includes('prefers-reduced-motion'),
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })))

    window.scrollY = 400
    render(<BackTop visibilityHeight={200} />)

    const button = screen.getByRole('button', { name: 'Back to top' })
    fireEvent.click(button)

    expect(window.scrollTo).toHaveBeenCalledWith({ top: 0, behavior: 'auto' })
  })
})
