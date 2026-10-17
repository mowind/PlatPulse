/**
 * Admin long-identifier disclosure (webui.md §8.5.3, §11.1). A non-secret
 * identifier — Agent ID, Genesis hash, observed P2P key — stays short in a list
 * row; one explicit, click/tap/keyboard-reachable toggle opens the complete
 * value as selectable, wrapping text with a copy action. Copy reports success
 * only after the Clipboard API resolves; when it is missing or rejects, the
 * complete value stays open and selected for manual copy and the failure is
 * announced. A missing value renders its real Unknown state with no controls.
 *
 * Never use this for one-time Enrollment/Recovery Tokens or credentials: those
 * keep their own single-view secret handling.
 */
import { ChevronDown, ChevronUp } from 'lucide-react'
import { useId, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import { formatIdentifier } from '../formatBytes'
import { cn } from '../lib/utils'
import { Button } from './ui/button'

export function AdminIdentifier({
  value,
  label,
  heading,
  lead,
  missing = 'Unknown',
  fullSlot = 'admin-identifier-full',
  className,
}: {
  /** The complete identifier; null/blank renders `missing` without controls. */
  value: string | null | undefined
  /** Accessible noun, e.g. "Genesis hash": names the toggle and copy action. */
  label: string
  /** Optional line above the toggle line (e.g. a display-name link). */
  heading?: ReactNode
  /** Leading element on the toggle's line; defaults to the short value. */
  lead?: ReactNode
  missing?: ReactNode
  /** data-slot of the complete value, for page-specific test hooks. */
  fullSlot?: string
  className?: string
}) {
  const full = value?.trim() ?? ''
  const [open, setOpen] = useState(false)
  const panelId = useId()
  const toggleRef = useRef<HTMLButtonElement>(null)

  if (!full) {
    // Same cell shape as the short value, with no title and no controls.
    return <code className={cn('font-mono text-xs text-muted-foreground', className)}>{missing}</code>
  }

  const collapseOnEscape = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== 'Escape' || !open) return
    event.stopPropagation()
    setOpen(false)
    toggleRef.current?.focus()
  }

  return (
    <div data-slot="admin-identifier" className={cn('flex min-w-0 max-w-full flex-col items-start', className)}>
      {heading}
      <div className="flex min-w-0 max-w-full flex-wrap items-center gap-x-1">
        {lead ?? (
          <code
            aria-hidden="true"
            className="min-w-0 font-mono text-xs text-muted-foreground [overflow-wrap:anywhere]"
            title={full}
          >
            {formatIdentifier(full)}
          </code>
        )}
        {/* §8.6: the complete value stays in the DOM while collapsed. */}
        {!open && <span className="sr-only">{label + ' ' + full}</span>}
        <Button
          ref={toggleRef}
          variant="link"
          size="xs"
          className="px-1 text-xs"
          aria-expanded={open}
          aria-controls={panelId}
          onClick={() => setOpen((value) => !value)}
          onKeyDown={collapseOnEscape}
        >
          {/* Named from content, not aria-label, so page labels such as a
              form's "Genesis hash" field stay unambiguous. */}
          {open ? 'Hide full ' : 'Show full '}
          <span className="sr-only">{label}</span>
          {open ? <ChevronUp aria-hidden="true" /> : <ChevronDown aria-hidden="true" />}
        </Button>
      </div>
      {open && (
        // inline-size containment keeps the opened value out of the table's
        // intrinsic column sizing: it fills the existing cell and wraps
        // instead of widening the table.
        <div
          id={panelId}
          onKeyDown={collapseOnEscape}
          className="mt-1 flex w-full min-w-0 max-w-88 flex-col items-start gap-1 [contain:inline-size]"
        >
          <IdentifierValue value={full} label={label} slot={fullSlot} />
        </div>
      )}
    </div>
  )
}

function IdentifierValue({ value, label, slot }: { value: string; label: string; slot: string }) {
  const [copyState, setCopyState] = useState<'idle' | 'copied' | 'failed'>('idle')
  const valueRef = useRef<HTMLElement>(null)

  function selectValue() {
    const node = valueRef.current
    const selection = typeof window === 'undefined' ? null : window.getSelection()
    if (!node || !selection) return
    const range = document.createRange()
    range.selectNodeContents(node)
    selection.removeAllRanges()
    selection.addRange(range)
  }

  async function copy() {
    setCopyState('idle')
    if (!navigator.clipboard?.writeText) {
      setCopyState('failed')
      selectValue()
      return
    }
    try {
      await navigator.clipboard.writeText(value)
      setCopyState('copied')
    } catch {
      setCopyState('failed')
      selectValue()
    }
  }

  return (
    <>
      <code
        ref={valueRef}
        data-slot={slot}
        className="block w-full min-w-0 select-all rounded-md border border-border/60 bg-background px-2 py-1.5 font-mono text-xs [overflow-wrap:anywhere]"
      >
        {value}
      </code>
      <div className="flex min-w-0 flex-wrap items-center gap-x-2">
        <Button variant="link" size="xs" className="px-0 text-xs" onClick={() => void copy()}>
          {'Copy ' + label}
        </Button>
        <span
          role="status"
          className={cn('text-xs', copyState === 'failed' ? 'text-destructive' : 'text-success')}
        >
          {copyState === 'copied' && 'Copied to clipboard.'}
          {copyState === 'failed' && 'Copy unavailable; select the full ' + label + ' shown here.'}
        </span>
      </div>
    </>
  )
}
