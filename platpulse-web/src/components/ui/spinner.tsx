import type * as React from 'react'

import { cn } from '../../lib/utils'

/** Ported from emerald src/components/ui/spinner/Spinner.vue. */
export function Spinner({
  show = true,
  size = 24,
  stroke = 2,
  className,
  contentClassName,
  children,
  label = 'Loading',
}: {
  show?: boolean
  size?: number
  stroke?: number
  className?: string
  contentClassName?: string
  children?: React.ReactNode
  label?: string
}) {
  if (!show) return <>{children}</>
  return (
    <div
      className={cn(
        'absolute inset-0 z-10 flex flex-col items-center justify-center gap-3 bg-card/60 backdrop-blur-sm',
        className,
      )}
      role="status"
      aria-live="polite"
    >
      <span className={cn('flex flex-col items-center justify-center gap-3', contentClassName)}>
        <span
          aria-hidden="true"
          className="block animate-spin rounded-full"
          style={{
            width: size,
            height: size,
            borderWidth: stroke,
            borderStyle: 'solid',
            borderColor: 'color-mix(in srgb, currentColor 18%, transparent)',
            borderTopColor: 'currentColor',
          }}
        />
        <span className="sr-only">{label}</span>
      </span>
      {children}
    </div>
  )
}
