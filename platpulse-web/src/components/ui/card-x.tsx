import type * as React from 'react'

import { cn } from '../../lib/utils'

type CardSize = 'small' | 'medium' | 'large'

/**
 * CardX, ported from emerald src/components/ui/card-x/CardX.vue.
 *
 * Emerald's shell is the app-wide card: the same component backs the Home
 * statistics tiles, the map panel and the node cards. Padding sizes, the
 * optional header/footer separators and the hoverable rule are the upstream
 * literals.
 */
export type CardXProps = React.ComponentProps<'div'> & {
  size?: CardSize
  hoverable?: boolean
  bordered?: boolean
  segmented?: boolean
  title?: React.ReactNode
  header?: React.ReactNode
  footer?: React.ReactNode
  headerClassName?: string
  contentClassName?: string
  footerClassName?: string
}

const PADDING: Record<CardSize, string> = {
  small: 'p-3',
  medium: 'p-4',
  large: 'p-6',
}

const HEADER_PADDING: Record<CardSize, string> = {
  small: 'px-3 py-2',
  medium: 'px-4 py-3',
  large: 'px-6 py-4',
}

export function CardX({
  className,
  size = 'medium',
  hoverable = false,
  bordered = true,
  segmented = false,
  title,
  header,
  footer,
  headerClassName,
  contentClassName,
  footerClassName,
  children,
  ...props
}: CardXProps) {
  const hasHeader = Boolean(header || title)
  return (
    <div
      data-slot="card-x"
      className={cn(
        'bg-card text-card-foreground flex flex-col rounded-lg',
        bordered ? 'border' : 'border-none',
        hoverable && 'transition-colors hover:border-foreground/30',
        className,
      )}
      {...props}
    >
      {hasHeader && (
        <div
          data-slot="card-x-header"
          className={cn(
            'flex items-center gap-2',
            HEADER_PADDING[size],
            segmented && 'border-b',
            headerClassName,
          )}
        >
          {header ?? <div className="min-w-0 flex-1 truncate text-sm font-medium">{title}</div>}
        </div>
      )}
      <div
        data-slot="card-x-content"
        className={cn(PADDING[size], hasHeader && 'pt-0', contentClassName)}
      >
        {children}
      </div>
      {footer && (
        <div
          data-slot="card-x-footer"
          className={cn(
            HEADER_PADDING[size],
            segmented && 'border-t bg-muted/40',
            footerClassName,
          )}
        >
          {footer}
        </div>
      )}
    </div>
  )
}
