import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { HomeToolbar, SEARCH_LABEL } from './HomeToolbar'
import type { PublicNetwork } from '../api/generated'
import type { HomeFilters } from '../homeFilters'

afterEach(cleanup)

const networks = [
  {
    networkKey: 'mainnet',
    displayName: 'Mainnet',
    nodes: [],
  },
  {
    networkKey: 'testnet',
    displayName: 'Testnet',
    nodes: [],
  },
] as unknown as PublicNetwork[]

const baseFilters: HomeFilters = {
  network: 'all',
  health: 'all',
  validator: 'all',
  sort: 'health',
  view: 'card',
  query: '',
}

describe('HomeToolbar', () => {
  it('renders network tabs and triggers updateFilters', () => {
    const updateFilters = vi.fn()
    render(
      <HomeToolbar
        networks={networks}
        filters={baseFilters}
        scope={{ network: 'all', rejected: null }}
        updateFilters={updateFilters}
      />,
    )

    const testnetTab = screen.getByRole('tab', { name: 'Testnet' })
    fireEvent.mouseDown(testnetTab)
    expect(updateFilters).toHaveBeenCalledWith({ network: 'testnet' }, false)
  })

  it('debounces search input by searchDebounceMs before calling updateFilters', () => {
    vi.useFakeTimers()
    try {
      const updateFilters = vi.fn()
      render(
        <HomeToolbar
          networks={networks}
          filters={baseFilters}
          scope={{ network: 'all', rejected: null }}
          updateFilters={updateFilters}
          searchDebounceMs={150}
        />,
      )

      const searchInput = screen.getByRole('searchbox', { name: SEARCH_LABEL }) as HTMLInputElement
      fireEvent.change(searchInput, { target: { value: 'Alpha' } })

      // Text updates immediately in draft
      expect(searchInput.value).toBe('Alpha')
      // But updateFilters has not been called yet
      expect(updateFilters).not.toHaveBeenCalled()

      // Advance 100ms (not yet 150ms)
      vi.advanceTimersByTime(100)
      expect(updateFilters).not.toHaveBeenCalled()

      // Advance remaining 50ms
      vi.advanceTimersByTime(50)
      expect(updateFilters).toHaveBeenCalledWith({ query: 'Alpha' }, true)
    } finally {
      vi.useRealTimers()
    }
  })

  it('immediately flushes pending draft query and cancels timer when switching another filter', () => {
    vi.useFakeTimers()
    try {
      const updateFilters = vi.fn()
      render(
        <HomeToolbar
          networks={networks}
          filters={baseFilters}
          scope={{ network: 'all', rejected: null }}
          updateFilters={updateFilters}
          searchDebounceMs={150}
        />,
      )

      const searchInput = screen.getByRole('searchbox', { name: SEARCH_LABEL }) as HTMLInputElement
      fireEvent.change(searchInput, { target: { value: 'Gamma' } })

      // Do NOT wait for 150ms. Immediately switch Network tab within the window:
      const testnetTab = screen.getByRole('tab', { name: 'Testnet' })
      fireEvent.mouseDown(testnetTab)

      // updateFilters must be called immediately with BOTH query and network
      expect(updateFilters).toHaveBeenCalledTimes(1)
      expect(updateFilters).toHaveBeenCalledWith({ query: 'Gamma', network: 'testnet' }, false)

      // Advancing timer should not trigger a second call
      vi.advanceTimersByTime(200)
      expect(updateFilters).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it('immediately clears search and cancels debounce when Clear search is clicked', () => {
    vi.useFakeTimers()
    try {
      const updateFilters = vi.fn()
      render(
        <HomeToolbar
          networks={networks}
          filters={{ ...baseFilters, query: 'Existing' }}
          scope={{ network: 'all', rejected: null }}
          updateFilters={updateFilters}
          searchDebounceMs={150}
        />,
      )

      const clearBtn = screen.getByRole('button', { name: 'Clear search' })
      fireEvent.click(clearBtn)

      expect(updateFilters).toHaveBeenCalledWith({ query: '' }, true)
      vi.advanceTimersByTime(200)
      expect(updateFilters).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })
})
