import type * as React from 'react'

import { cn } from '../../lib/utils'

/** Ported from emerald src/components/ui/empty/Empty.vue. */
export function Empty({
  description,
  className,
  children,
}: {
  description?: React.ReactNode
  className?: string
  children?: React.ReactNode
}) {
  return (
    <div
      className={cn('flex flex-col items-center justify-center gap-3 p-8 text-muted-foreground', className)}
    >
      <svg
        aria-hidden="true"
        width="56"
        height="56"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        opacity="60"
      >
        <path d="M3 7.5 12 3l9 4.5v9L12 21l-9-4.5z" />
        <path d="M3 7.5 12 12l9-4.5" />
        <path d="M12 12v9" />
      </svg>
      {description && <p className="text-sm">{description}</p>}
      {children}
    </div>
  )
}
