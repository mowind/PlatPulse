import type { ReactNode } from 'react'
import { Suspense } from 'react'
import { useRouteError } from 'react-router'
import { Spinner } from './ui/spinner'

/**
 * Smooth, non-blocking fallback rendered while lazy route chunks resolve.
 * Preserves layout stability and screen-reader accessibility without blocking
 * the interactive shell.
 */
export function RouteLoadingFallback() {
  return (
    <div className="relative min-h-[16rem] w-full">
      <Spinner label="Loading">
        <span className="text-xs text-muted-foreground">Loading…</span>
      </Spinner>
    </div>
  )
}

/**
 * Boundary element displayed when dynamic chunk import or route execution fails
 * (e.g. stale hashed asset after deployment or network drop), conforming to
 * webui.md §9 (no leaked stack traces or internal paths; names failed task and
 * next safe action).
 */
export function RouteErrorFallback() {
  const error = useRouteError()
  if (import.meta.env.DEV) {
    console.error('Route error:', error)
  }

  return (
    <div
      role="alert"
      className="flex min-h-[16rem] w-full flex-col items-center justify-center gap-4 p-8 text-center"
    >
      <div className="flex max-w-md flex-col items-center gap-2">
        <h2 className="text-lg font-semibold tracking-tight text-foreground">
          Failed to load page
        </h2>
        <p className="text-sm text-muted-foreground">
          The requested page could not be loaded. This may occur if a new version was deployed or your network connection was interrupted.
        </p>
      </div>
      <button
        type="button"
        onClick={() => window.location.reload()}
        className="inline-flex min-h-11 items-center justify-center rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground shadow transition-colors hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
      >
        Reload page
      </button>
    </div>
  )
}

export function withSuspense(node: ReactNode) {
  return <Suspense fallback={<RouteLoadingFallback />}>{node}</Suspense>
}
