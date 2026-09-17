import * as TabsPrimitive from '@radix-ui/react-tabs'
import { cva, type VariantProps } from 'class-variance-authority'
import type * as React from 'react'

import { cn } from '../../lib/utils'

/**
 * Tabs, ported from komari-theme-emerald @ c2c5e88 src/components/ui/tabs/*
 * (upstream builds these on reka-ui, the Vue port of Radix, so Radix is the
 * same lineage rather than a substitute).
 *
 * State and orientation selectors target Radix's actual data-state and
 * data-orientation attributes; no synthetic Reka attributes are added.
 */
export const tabsListVariants = cva(
  'min-h-11 rounded-lg p-[3px] group-data-[orientation=horizontal]/tabs:h-9 group-data-[orientation=vertical]/tabs:rounded-lg data-[variant=line]:rounded-none group/tabs-list inline-flex w-fit items-center justify-center text-muted-foreground group-data-[orientation=vertical]/tabs:h-fit group-data-[orientation=vertical]/tabs:flex-col',
  {
    variants: {
      variant: {
        default: 'bg-muted',
        line: 'gap-1 bg-transparent',
      },
    },
    defaultVariants: { variant: 'default' },
  },
)

export function Tabs({ className, ...props }: React.ComponentProps<typeof TabsPrimitive.Root>) {
  return (
    <TabsPrimitive.Root
      data-slot="tabs"
      className={cn('group/tabs flex flex-col gap-2', className)}
      {...props}
    />
  )
}

export type TabsListProps = React.ComponentProps<typeof TabsPrimitive.List> &
  VariantProps<typeof tabsListVariants>

export function TabsList({ className, variant, ...props }: TabsListProps) {
  return (
    <TabsPrimitive.List
      data-slot="tabs-list"
      data-variant={variant ?? 'default'}
      className={cn(tabsListVariants({ variant }), className)}
      {...props}
    />
  )
}

const triggerClass =
  'min-h-11 min-w-11 gap-1.5 rounded-md border border-transparent px-2 py-1 text-sm font-medium group-data-[orientation=vertical]/tabs:px-2.5 group-data-[orientation=vertical]/tabs:py-1.5 [&_svg:not([class*=size-])]:size-4 has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 relative inline-flex h-[calc(100%-1px)] flex-1 items-center justify-center whitespace-nowrap text-foreground/60 transition-all group-data-[orientation=vertical]/tabs:w-full group-data-[orientation=vertical]/tabs:justify-start hover:text-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:outline-1 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-50 dark:text-muted-foreground dark:hover:text-foreground [&_svg]:pointer-events-none [&_svg]:shrink-0 group-data-[variant=line]/tabs-list:bg-transparent group-data-[variant=line]/tabs-list:data-[state=active]:bg-transparent dark:group-data-[variant=line]/tabs-list:data-[state=active]:border-transparent dark:group-data-[variant=line]/tabs-list:data-[state=active]:bg-transparent data-[state=active]:bg-background data-[state=active]:text-foreground dark:data-[state=active]:border-input dark:data-[state=active]:bg-input/30 dark:data-[state=active]:text-foreground after:absolute after:bg-foreground after:opacity-0 after:transition-opacity group-data-[orientation=horizontal]/tabs:after:inset-x-0 group-data-[orientation=horizontal]/tabs:after:bottom-[-5px] group-data-[orientation=horizontal]/tabs:after:h-0.5 group-data-[orientation=vertical]/tabs:after:inset-y-0 group-data-[orientation=vertical]/tabs:after:-right-1 group-data-[orientation=vertical]/tabs:after:w-0.5 group-data-[variant=line]/tabs-list:data-[state=active]:after:opacity-100'

export function TabsTrigger({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Trigger>) {
  return (
    <TabsPrimitive.Trigger
      data-slot="tabs-trigger"
      className={cn(triggerClass, className)}
      {...props}
    />
  )
}

export function TabsContent({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Content>) {
  return (
    <TabsPrimitive.Content
      data-slot="tabs-content"
      className={cn('flex-1 outline-none', className)}
      {...props}
    />
  )
}
