import { useCallback, useSyncExternalStore } from 'react'

/**
 * Whether a CSS media query currently matches, following its `change`
 * events. Where `matchMedia` is unavailable (jsdom) the query reads as not
 * matching, so callers fall back to their narrow-screen behaviour.
 */
export function useMediaQuery(query: string): boolean {
  const subscribe = useCallback(
    (onChange: () => void) => {
      if (typeof window.matchMedia !== 'function') return () => {}
      const list = window.matchMedia(query)
      list.addEventListener('change', onChange)
      return () => list.removeEventListener('change', onChange)
    },
    [query],
  )
  return useSyncExternalStore(
    subscribe,
    () => typeof window.matchMedia === 'function' && window.matchMedia(query).matches,
    () => false,
  )
}
