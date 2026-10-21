import { useEffect, useState } from 'react'
import { ArrowUp } from 'lucide-react'
import { cn } from '../../lib/utils'

export interface BackTopProps {
  /**
   * The scroll height threshold in pixels above which the button becomes visible.
   * Defaults to 200.
   */
  visibilityHeight?: number
  /** Additional CSS class names for the button. */
  className?: string
}

/**
 * BackTop component, ported from komari-theme-emerald @ c2c5e88
 * src/components/ui/back-top/BackTop.vue.
 *
 * Meets PlatPulse 44x44px touch target constraints (Deviation 10) and smooth-scrolls
 * to the top, respecting prefers-reduced-motion. When hidden, it does not intercept
 * pointer events or keyboard focus.
 */
export function BackTop({
  visibilityHeight = 200,
  className,
}: BackTopProps) {
  const [visible, setVisible] = useState(() =>
    typeof window !== 'undefined' ? window.scrollY > visibilityHeight : false,
  )

  useEffect(() => {
    let frame = 0
    const handleScroll = () => {
      frame = 0
      setVisible(window.scrollY > visibilityHeight)
    }

    const onScroll = () => {
      if (frame !== 0) return
      frame = window.requestAnimationFrame(handleScroll)
    }

    window.addEventListener('scroll', onScroll, { passive: true })
    handleScroll()

    return () => {
      window.removeEventListener('scroll', onScroll)
      if (frame !== 0) window.cancelAnimationFrame(frame)
    }
  }, [visibilityHeight])

  const scrollToTop = () => {
    const prefersReducedMotion =
      typeof window !== 'undefined' &&
      typeof window.matchMedia === 'function' &&
      window.matchMedia('(prefers-reduced-motion: reduce)').matches

    window.scrollTo({
      top: 0,
      behavior: prefersReducedMotion ? 'auto' : 'smooth',
    })
  }

  return (
    <button
      type="button"
      data-slot="back-top"
      aria-label="Back to top"
      aria-hidden={!visible}
      tabIndex={visible ? 0 : -1}
      disabled={!visible}
      onClick={scrollToTop}
      className={cn(
        'fixed bottom-6 right-6 sm:bottom-8 sm:right-8 z-30 flex size-11 items-center justify-center rounded-full border border-border bg-background shadow-sm text-foreground transition-all duration-200 hover:bg-accent focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 motion-reduce:transition-none',
        visible
          ? 'opacity-100 translate-y-0 visible pointer-events-auto'
          : 'opacity-0 translate-y-2 invisible pointer-events-none',
        className,
      )}
    >
      <ArrowUp className="size-5 shrink-0" strokeWidth={2} aria-hidden="true" />
    </button>
  )
}
