import { cva, type VariantProps } from 'class-variance-authority'
import type * as React from 'react'

import { cn } from '../../lib/utils'

/** Ported literally from emerald src/components/ui/alert/index.ts. */
export const alertVariants = cva(
  'grid gap-0.5 rounded-lg border px-4 py-3 text-left text-sm has-data-[slot=alert-action]:relative has-data-[slot=alert-action]:pr-18 has-[>svg]:grid-cols-[auto_1fr] has-[>svg]:gap-x-2.5 *:[svg]:row-span-2 *:[svg]:translate-y-0.5 *:[svg]:text-current *:[svg:not([class*=size-])]:size-4 group/alert relative w-full',
  {
    variants: {
      variant: {
        default: 'bg-background text-foreground',
        destructive:
          'text-destructive bg-background *:data-[slot=alert-description]:text-destructive/90 *:[svg]:text-current',
      },
    },
    defaultVariants: { variant: 'default' },
  },
)

export type AlertProps = React.ComponentProps<'div'> & VariantProps<typeof alertVariants>

export function Alert({ className, variant, ...props }: AlertProps) {
  return (
    <div
      data-slot="alert"
      role="alert"
      className={cn(alertVariants({ variant }), className)}
      {...props}
    />
  )
}

export function AlertTitle({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="alert-title"
      className={cn(
        'font-medium group-has-[>svg]/alert:col-start-2 [&_a]:underline [&_a]:underline-offset-3 [&_a]:hover:text-foreground',
        className,
      )}
      {...props}
    />
  )
}

export function AlertDescription({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="alert-description"
      className={cn(
        'text-muted-foreground text-sm text-balance md:text-pretty [&_p:not(:last-child)]:mb-4 [&_a]:underline [&_a]:underline-offset-3 [&_a]:hover:text-foreground',
        className,
      )}
      {...props}
    />
  )
}

export function AlertAction({ className, ...props }: React.ComponentProps<'div'>) {
  return <div data-slot="alert-action" className={cn('absolute top-2.5 right-3', className)} {...props} />
}
