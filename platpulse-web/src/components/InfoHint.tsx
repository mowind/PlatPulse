import { useCallback, useEffect, useId, useRef, useState, type FocusEvent, type ReactNode } from 'react'
import { Info } from 'lucide-react'
import { Button } from './ui/button'
import { ToolbarPopover } from './ui/toolbar-popover'
import { cn } from '../lib/utils'

/**
 * A static definition, kept off the card until the reader asks for it.
 *
 * Node Detail used to spend a permanent line of small print on what a label
 * means ("not chain sync", "not whole-Host disk usage"). The label now carries
 * the meaning and this control carries the full definition: a real button, so a
 * tap, a click, Enter or Space opens it; a press outside, the button again,
 * Escape, or focus moving on closes it, and Escape hands focus back. Hover alone
 * never opens it, and it is never a bare `title`, so touch and keyboard readers
 * reach the same text. Opening moves focus onto the definition itself (the
 * button names it through `aria-controls`), so a screen reader announces the
 * text rather than only "expanded"; Tab or Escape leave it.
 *
 * Only static definitions belong here. An actual condition — a failed
 * collection, a stale value, a missing direction — stays visible on the card.
 *
 * The button is the 44px target every visible control keeps; `inline` lets it
 * sit beside an 11–12px label by giving back its extra size through negative
 * margins, so the row keeps its height. The target therefore reaches a few
 * pixels into the neighbouring row's non-interactive label, which is the
 * accepted price of a full-size target beside dense rows.
 */
export function InfoHint({ label, children, align = 'start', inline = true }: {
  /** Accessible name of the button and of the surface it opens. */
  label: string
  children: ReactNode
  align?: 'start' | 'end'
  /** Beside a text label (the default) or filling a reserved 44px slot. */
  inline?: boolean
}) {
  const [open, setOpen] = useState(false)
  const trigger = useRef<HTMLButtonElement | null>(null)
  const content = useRef<HTMLDivElement | null>(null)
  const contentId = useId()
  const close = useCallback(() => setOpen(false), [])
  useEffect(() => {
    if (open) content.current?.focus()
  }, [open])
  // Focus leaving the button and its definition closes it, so a stale surface
  // never lingers (and a later Escape elsewhere never pulls focus back here).
  const onBlur = (event: FocusEvent<HTMLDivElement>) => {
    const next = event.relatedTarget
    if (next instanceof Node && event.currentTarget.contains(next)) return
    setOpen(false)
  }
  return (
    <div data-slot="info-hint-root" className="contents" onBlur={onBlur}>
      <ToolbarPopover
        open={open}
        onClose={close}
        label={label}
        align={align}
        returnFocusRef={trigger}
        className="w-64 whitespace-normal text-left text-xs font-normal normal-case leading-5 tracking-normal"
        trigger={(
          <Button
            ref={trigger}
            variant="ghost"
            size="icon"
            data-slot="info-hint"
            aria-label={label}
            aria-expanded={open}
            aria-haspopup="dialog"
            aria-controls={open ? contentId : undefined}
            onClick={() => setOpen((value) => !value)}
            className={cn('size-11 text-muted-foreground hover:text-foreground', inline && '-my-3.5 -ml-2.5 -mr-3.5')}
          >
            <Info className="size-3.5" aria-hidden="true" />
          </Button>
        )}
      >
        <div ref={content} id={contentId} tabIndex={-1} data-slot="info-hint-content" className="grid gap-1.5 outline-none">{children}</div>
      </ToolbarPopover>
    </div>
  )
}
