import type { ComponentProps, ComponentType } from 'react'
import { lazy, useState } from 'react'

/**
 * Lazy route components (issue #275) that the app preloads once it has started.
 *
 * Admin pages, AdminLayout and LoginPage are split out so the public Home
 * screen does not download or parse them before its first paint. A plain
 * React.lazy boundary suspends on its first render even when the chunk is
 * already in the module cache, and React holds the Suspense fallback for at
 * least ~300ms, so every lazy page would flash "Loading…" on first visit.
 * Instead, the app preloads every route module when the browser is idle after
 * startup (schedulePreloadLazyRoutes); a route whose module has loaded renders
 * it directly, without a Suspense boundary. A route visited before its module
 * arrives still suspends and shares the same in-flight import.
 */

type RouteModule<T> = { default: T }

const preloaders: Array<() => Promise<unknown>> = []

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function lazyRoute<T extends ComponentType<any>>(factory: () => Promise<RouteModule<T>>) {
  let loaded: T | null = null
  let pending: Promise<RouteModule<T>> | null = null

  const load = () => {
    pending ??= factory().then(
      (mod) => {
        loaded = mod.default
        return mod
      },
      (error: unknown) => {
        // A failed chunk (dropped connection, stale hash after a deploy) is
        // retried by the next visit rather than cached as a failure.
        pending = null
        throw error
      },
    )
    return pending
  }
  preloaders.push(load)

  const Lazy = lazy(load)

  return function LazyRouteWrapper(props: ComponentProps<T>) {
    // Decided once per mount: a module that arrives while this route is
    // suspended keeps rendering through the lazy boundary instead of swapping
    // component types (which would remount the page and drop its state).
    const [Loaded] = useState<T | null>(() => loaded)
    if (Loaded) {
      return <Loaded {...props} />
    }
    return <Lazy {...props} />
  }
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function lazyNamed<T extends Record<string, any>, K extends keyof T>(
  factory: () => Promise<T>,
  name: K,
) {
  return lazyRoute(() => factory().then((mod) => ({ default: mod[name] })))
}

/** Load every registered route module; rejects if any import fails. */
export function preloadLazyRoutes(): Promise<void> {
  return Promise.all(preloaders.map((preload) => preload())).then(() => undefined)
}

/**
 * Preload the route modules once the page has loaded and the browser is idle,
 * so they never compete with the first screen. Failures are left to the route
 * that needs the module: its own import retries and its error boundary reports.
 */
export function schedulePreloadLazyRoutes(target: Window = window) {
  const preload = () => {
    void Promise.allSettled(preloaders.map((load) => load()))
  }
  const whenIdle = () => {
    if (typeof target.requestIdleCallback === 'function') {
      target.requestIdleCallback(preload, { timeout: 2_000 })
    } else {
      target.setTimeout(preload, 200)
    }
  }
  if (target.document.readyState === 'complete') {
    whenIdle()
  } else {
    target.addEventListener('load', whenIdle, { once: true })
  }
}
