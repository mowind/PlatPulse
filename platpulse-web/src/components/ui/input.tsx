import type * as React from 'react'

import { cn } from '../../lib/utils'

/** Ported literally from emerald src/components/ui/input/Input.vue. */
export function Input({ className, ...props }: React.ComponentProps<'input'>) {
  return (
    <input
      data-slot="input"
      className={cn(
        'min-h-11 bg-transparent border-input focus-visible:border-ring focus-visible:ring-ring/50 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive dark:aria-invalid:border-destructive/50 h-9 rounded-md border px-3 py-1 text-base shadow-xs transition-colors file:h-7 file:text-sm file:font-medium focus-visible:ring-[3px] aria-invalid:ring-[3px] md:text-sm w-full min-w-0 outline-none file:inline-flex file:border-0 file:bg-transparent file:text-foreground placeholder:text-muted-foreground disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 dark:bg-input/30',
        className,
      )}
      {...props}
    />
  )
}

/** Same surface, for the multi-line admin forms. */
export function Textarea({ className, ...props }: React.ComponentProps<'textarea'>) {
  return (
    <textarea
      data-slot="textarea"
      className={cn(
        'min-h-11 bg-transparent border-input focus-visible:border-ring focus-visible:ring-ring/50 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive dark:aria-invalid:border-destructive/50 rounded-md border px-3 py-2 text-base shadow-xs transition-colors focus-visible:ring-[3px] aria-invalid:ring-[3px] md:text-sm w-full min-w-0 outline-none placeholder:text-muted-foreground disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 dark:bg-input/30',
        className,
      )}
      {...props}
    />
  )
}

/**
 * Native select. Upstream Emerald has no shadcn Select — its one dropdown is a
 * raw <select> (NodeGeneralCards finance popover) — so this keeps the native
 * element and only adopts the Emerald control surface.
 */
export function Select({ className, ...props }: React.ComponentProps<'select'>) {
  return (
    <select
      data-slot="select"
      className={cn(
        'border-input bg-background min-h-11 h-9 w-full min-w-0 rounded-md border px-2 text-sm shadow-xs transition-colors outline-none',
        'focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50',
        'disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 dark:bg-input/30',
        className,
      )}
      {...props}
    />
  )
}

type SelectionProps = Omit<React.ComponentProps<'input'>, 'type'>

/**
 * Keep the native input itself 44px, not just its label. The decorative 16px
 * indicator follows native CSS state, including uncontrolled/reset/fieldset
 * state, without replacing browser form, focus or keyboard behavior.
 */
function NativeSelection({ className, type, ...props }: SelectionProps & { type: 'checkbox' | 'radio' }) {
  return (
    <span className="relative inline-flex size-11 shrink-0 items-center justify-center align-middle">
      <input
        {...props}
        type={type}
        data-slot={type}
        className={cn(
          'peer m-0 size-11 min-h-11 min-w-11 shrink-0 cursor-pointer opacity-0 disabled:cursor-not-allowed',
          'forced-colors:appearance-auto forced-colors:opacity-100 forced-colors:accent-auto',
          className,
        )}
      />
      <span
        aria-hidden="true"
        data-slot="selection-indicator"
        className={cn(
          'pointer-events-none absolute flex size-4 items-center justify-center border border-input bg-transparent shadow-xs dark:bg-input/30',
          'peer-checked:border-primary peer-checked:bg-primary peer-checked:text-primary-foreground',
          'peer-focus-visible:border-ring peer-focus-visible:ring-[3px] peer-focus-visible:ring-ring/50',
          'peer-aria-invalid:border-destructive peer-aria-invalid:ring-[3px] peer-aria-invalid:ring-destructive/20 dark:peer-aria-invalid:ring-destructive/40',
          'peer-disabled:opacity-50 forced-colors:hidden',
          type === 'checkbox'
            ? 'rounded-[4px] peer-checked:[&>svg]:visible peer-indeterminate:border-primary peer-indeterminate:bg-primary peer-indeterminate:text-primary-foreground peer-indeterminate:[&>svg]:hidden peer-indeterminate:[&>span]:visible'
            : 'rounded-full peer-checked:[&>span]:visible',
        )}
      >
        {type === 'checkbox' ? (
          <>
            <svg className="invisible size-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3">
              <path d="m5 12 4 4L19 6" />
            </svg>
            <span className="invisible absolute h-0.5 w-2.5 rounded-full bg-current" />
          </>
        ) : (
          <span className="invisible size-1.5 rounded-full bg-current" />
        )}
      </span>
    </span>
  )
}

export function Checkbox(props: SelectionProps) {
  return <NativeSelection {...props} type="checkbox" />
}

export function Radio(props: SelectionProps) {
  return <NativeSelection {...props} type="radio" />
}
