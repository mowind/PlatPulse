import { useEffect, useRef, useState } from 'react'
import type * as React from 'react'

import { cn } from '../../lib/utils'

type Placement = 'top' | 'bottom' | 'left' | 'right'

const PLACEMENT: Record<Placement, string> = {
  top: 'bottom-full left-1/2 mb-2 -translate-x-1/2',
  bottom: 'top-full left-1/2 mt-2 -translate-x-1/2',
  left: 'top-1/2 right-full mr-2 -translate-y-1/2',
  right: 'top-1/2 left-full ml-2 -translate-y-1/2',
}

/**
 * DataTooltip, ported from komari-theme-emerald @ c2c5e88
 * src/components/ui/data-tooltip/DataTooltip.vue.
 *
 * Upstream implements its own tooltip rather than a shadcn one, and it is
 * deliberately touch-capable: a coarse pointer toggles the bubble on
 * pointerdown, the following synthetic click is swallowed so the trigger's own
 * action does not fire, an outside pointerdown or Escape closes it, and
 * keyboard focus opens it too. That behaviour is ported unchanged — a
 * hover-only tooltip would fail the PlatPulse accessibility requirement.
 */
export function DataTooltip({
  content,
  contentNode,
  placement = 'top',
  width,
  height,
  as: As = 'div',
  className,
  contentClassName,
  children,
}: {
  content?: string
  contentNode?: React.ReactNode
  placement?: Placement
  width?: number | string
  height?: number | string
  as?: React.ElementType
  className?: string
  contentClassName?: string
  children?: React.ReactNode
}) {
  const rootRef = useRef<HTMLElement | null>(null)
  const [open, setOpen] = useState(false)
  const [hoverOpen, setHoverOpen] = useState(false)
  const lastTouchOpenAt = useRef(0)
  const stopNextClick = useRef(false)

  const hasTooltip = Boolean(content || contentNode)

  useEffect(() => {
    if (!open) return
    const onPointerDown = (event: PointerEvent) => {
      const root = rootRef.current
      if (!root || !event.target || root.contains(event.target as Node)) return
      setOpen(false)
      setHoverOpen(false)
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      setOpen(false)
      setHoverOpen(false)
    }
    document.addEventListener('pointerdown', onPointerDown, true)
    document.addEventListener('keydown', onKeyDown)
    return () => {
      document.removeEventListener('pointerdown', onPointerDown, true)
      document.removeEventListener('keydown', onKeyDown)
    }
  }, [open])

  useEffect(() => {
    if (hasTooltip) return
    setOpen(false)
    setHoverOpen(false)
  }, [hasTooltip])

  const sizeStyle: React.CSSProperties = {}
  if (width != null) sizeStyle.width = typeof width === 'number' ? width + 'px' : width
  if (height != null) sizeStyle.height = typeof height === 'number' ? height + 'px' : height

  return (
    <As
      ref={rootRef}
      data-slot="data-tooltip"
      data-state={open ? 'open' : 'closed'}
      className={cn('group/data-tooltip relative inline-block', className)}
      onPointerDownCapture={(event: React.PointerEvent) => {
        if (!hasTooltip) return
        const coarse =
          typeof window !== 'undefined' &&
          typeof window.matchMedia === 'function' &&
          window.matchMedia('(hover: none), (pointer: coarse)').matches
        if (event.pointerType === 'mouse' && !coarse) return
        lastTouchOpenAt.current = Date.now()
        stopNextClick.current = true
        setOpen((value) => !value)
      }}
      onPointerEnter={() => hasTooltip && setHoverOpen(true)}
      onPointerLeave={() => setHoverOpen(false)}
      onFocus={() => hasTooltip && setHoverOpen(true)}
      onBlur={() => setHoverOpen(false)}
      onClick={(event: React.MouseEvent) => {
        if (stopNextClick.current && Date.now() - lastTouchOpenAt.current < 800) {
          event.stopPropagation()
        }
        stopNextClick.current = false
      }}
    >
      {children}
      {hasTooltip && (open || hoverOpen) && (
        <span
          role="tooltip"
          className={cn(
            'pointer-events-none absolute z-20 hidden rounded bg-foreground/80 p-1 text-[10px] leading-none text-background shadow-lg group-hover/data-tooltip:block group-focus-within/data-tooltip:block whitespace-normal break-words',
            open && 'block',
            PLACEMENT[placement],
            contentClassName,
          )}
          style={sizeStyle}
        >
          {contentNode ?? content}
        </span>
      )}
    </As>
  )
}
