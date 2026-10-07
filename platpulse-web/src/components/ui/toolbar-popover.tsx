import { useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from 'react'

import { cn } from '../../lib/utils'

/** The clearance a floating surface keeps from the viewport's own edges. */
const VIEWPORT_MARGIN = 16

/**
 * The small surface a toolbar control opens, anchored under that control (#232).
 *
 * This is not a modal: it hangs off its trigger, it closes on a press outside
 * itself or on Escape, and it takes focus only when the caller asks (the search
 * field focuses itself because typing is the reason it opened, while a panel of
 * choices leaves focus on the control the reader just used). The surface is
 * kept inside the viewport's width: a control near the right edge, or a phone
 * narrower than the surface, would otherwise push the page sideways, which
 * refuses to scroll back and is therefore a real defect rather than a cosmetic
 * one. The correction is measured after layout, so the surface never paints
 * outside the viewport in the first place.
 */
export function ToolbarPopover({
  open,
  onClose,
  label,
  trigger,
  returnFocusRef,
  className,
  children,
}: {
  /** Whether the surface is showing. */
  open: boolean
  /** Close the surface: an outside press, Escape, or the trigger again. */
  onClose: () => void
  /** The surface's own name, which is how a reader reaches it by role. */
  label: string
  /** The control the surface hangs under; it owns its own ARIA state. */
  trigger: ReactNode
  /** Where Escape hands focus back: the control that opened the surface. */
  returnFocusRef?: RefObject<HTMLElement | null>
  className?: string
  children: ReactNode
}) {
  const wrapperRef = useRef<HTMLDivElement | null>(null)
  const surfaceRef = useRef<HTMLDivElement | null>(null)
  // The correction in force, kept beside the state so that a re-measure reads
  // the surface's unshifted position instead of its own last answer.
  const appliedShift = useRef(0)
  const [shift, setShift] = useState(0)

  useLayoutEffect(() => {
    if (!open) {
      appliedShift.current = 0
      setShift(0)
      return
    }
    const place = () => {
      const surface = surfaceRef.current
      if (surface === null) return
      const rect = surface.getBoundingClientRect()
      const viewport = document.documentElement.clientWidth
      const left = rect.left - appliedShift.current
      const next =
        left + rect.width > viewport - VIEWPORT_MARGIN
          ? viewport - VIEWPORT_MARGIN - rect.width - left
          : left < VIEWPORT_MARGIN
            ? VIEWPORT_MARGIN - left
            : 0
      appliedShift.current = next
      setShift(next)
    }
    place()
    window.addEventListener('resize', place)
    return () => window.removeEventListener('resize', place)
  }, [open])

  useEffect(() => {
    if (!open) return
    // A press outside closes the surface without moving focus: the reader is
    // already reaching for whatever they pressed. Escape is a keyboard retreat
    // from a surface that was opened from the keyboard, so it hands focus back
    // to the control that opened it.
    const onPointerDown = (event: PointerEvent) => {
      const wrapper = wrapperRef.current
      if (wrapper !== null && event.target instanceof Node && wrapper.contains(event.target)) return
      onClose()
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      onClose()
      returnFocusRef?.current?.focus()
    }
    document.addEventListener('pointerdown', onPointerDown, true)
    document.addEventListener('keydown', onKeyDown)
    return () => {
      document.removeEventListener('pointerdown', onPointerDown, true)
      document.removeEventListener('keydown', onKeyDown)
    }
  }, [onClose, open, returnFocusRef])

  return (
    <div ref={wrapperRef} className="relative flex-none">
      {trigger}
      {open && (
        <div
          ref={surfaceRef}
          role="dialog"
          aria-label={label}
          data-slot="toolbar-popover"
          className={cn(
            'absolute top-full z-30 mt-2 max-w-[calc(100vw-2rem)] rounded-md border bg-popover p-3 text-popover-foreground shadow-lg',
            className,
          )}
          style={{ left: shift }}
        >
          {children}
        </div>
      )}
    </div>
  )
}
