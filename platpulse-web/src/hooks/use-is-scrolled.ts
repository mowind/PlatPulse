import { useEffect, useState } from 'react'

/**
 * Emerald's header switches from transparent to backdrop-blur-xl once the page
 * has scrolled. Upstream drives this from BackTop with visibilityHeight 1, so
 * the threshold is one pixel.
 */
export function useIsScrolled(threshold = 1): boolean {
  const [scrolled, setScrolled] = useState(() =>
    typeof window === 'undefined' ? false : window.scrollY > threshold,
  )

  useEffect(() => {
    let frame = 0
    const read = () => {
      frame = 0
      setScrolled(window.scrollY > threshold)
    }
    const onScroll = () => {
      if (frame !== 0) return
      frame = window.requestAnimationFrame(read)
    }
    window.addEventListener('scroll', onScroll, { passive: true })
    read()
    return () => {
      window.removeEventListener('scroll', onScroll)
      if (frame !== 0) window.cancelAnimationFrame(frame)
    }
  }, [threshold])

  return scrolled
}
