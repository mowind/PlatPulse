import { act, cleanup, render, screen } from '@testing-library/react'
import { Suspense } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { lazyNamed, lazyRoute, preloadLazyRoutes, schedulePreloadLazyRoutes } from './lazyRoute'

function Page({ label }: { label: string }) {
  return <p>{label}</p>
}

function renderRoute(Route: (props: { label: string }) => React.ReactNode) {
  return render(
    <Suspense fallback={<p>Loading…</p>}>
      <Route label="Admin page" />
    </Suspense>,
  )
}

afterEach(cleanup)

describe('lazyRoute', () => {
  it('renders a preloaded route on its first render, without the Suspense fallback', async () => {
    const factory = vi.fn(() => Promise.resolve({ default: Page }))
    const Route = lazyRoute(factory)
    const Named = lazyNamed(() => Promise.resolve({ NamedPage: Page }), 'NamedPage')

    await preloadLazyRoutes()

    renderRoute(Route)
    expect(screen.getByText('Admin page')).toBeTruthy()
    expect(screen.queryByText('Loading…')).toBeNull()
    renderRoute(Named)
    expect(screen.getAllByText('Admin page')).toHaveLength(2)
    expect(factory).toHaveBeenCalledTimes(1)
  })

  it('suspends a route visited before its module arrives and shares the in-flight import', async () => {
    let resolve: (mod: { default: typeof Page }) => void = () => {}
    const factory = vi.fn(() => new Promise<{ default: typeof Page }>((done) => { resolve = done }))
    const Route = lazyRoute(factory)

    renderRoute(Route)
    expect(screen.getByText('Loading…')).toBeTruthy()
    const preloading = preloadLazyRoutes()
    await act(async () => {
      resolve({ default: Page })
      await preloading
    })

    expect(screen.getByText('Admin page')).toBeTruthy()
    expect(factory).toHaveBeenCalledTimes(1)
  })

  it('retries a failed preload instead of caching the failure', async () => {
    const factory = vi.fn()
      .mockRejectedValueOnce(new Error('chunk failed'))
      .mockResolvedValue({ default: Page })
    const Route = lazyRoute(factory)

    await expect(preloadLazyRoutes()).rejects.toThrow('chunk failed')
    await preloadLazyRoutes()

    renderRoute(Route)
    expect(screen.getByText('Admin page')).toBeTruthy()
    expect(factory).toHaveBeenCalledTimes(2)
  })
})

describe('schedulePreloadLazyRoutes', () => {
  function fakeWindow(readyState: DocumentReadyState) {
    const listeners: Record<string, () => void> = {}
    const idle: Array<() => void> = []
    const target = {
      document: { readyState },
      addEventListener: vi.fn((type: string, listener: () => void) => { listeners[type] = listener }),
      requestIdleCallback: vi.fn((callback: () => void) => { idle.push(callback); return idle.length }),
      setTimeout: vi.fn(),
    }
    return { target: target as unknown as Window, listeners, idle, raw: target }
  }

  it('waits for the page load and an idle period before importing', async () => {
    const factory = vi.fn(() => Promise.resolve({ default: Page }))
    lazyRoute(factory)
    const { target, listeners, idle, raw } = fakeWindow('loading')

    schedulePreloadLazyRoutes(target)
    expect(raw.requestIdleCallback).not.toHaveBeenCalled()
    listeners.load()
    expect(factory).not.toHaveBeenCalled()
    idle.forEach((callback) => callback())

    expect(factory).toHaveBeenCalledTimes(1)
  })

  it('schedules immediately when the page has already loaded', () => {
    const { target, raw } = fakeWindow('complete')

    schedulePreloadLazyRoutes(target)

    expect(raw.addEventListener).not.toHaveBeenCalled()
    expect(raw.requestIdleCallback).toHaveBeenCalledTimes(1)
  })
})
