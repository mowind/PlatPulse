import type { ReactNode, Ref } from 'react'
import { StatusBadge } from './StatusBadge'
import { Button } from './ui/button'

/** The notice an editor shows when its write succeeded but reading the
 *  entity's detail back failed. It is a warning beside the write's own result,
 *  never a second copy of the page's refresh alert (no live role: that alert
 *  already announces the failed read), and its only action reads again. */
export function ReadBackNotice({
  children,
  readAgainLabel,
  readAgainRef,
  onReadAgain,
  reading,
  className,
}: {
  /** What the failed read-back means for the value just written. */
  children: ReactNode
  readAgainLabel: string
  readAgainRef?: Ref<HTMLButtonElement>
  onReadAgain: () => void
  /** While the read runs: the action stays in place but answers nothing. */
  reading?: boolean
  className?: string
}) {
  return (
    <div
      data-slot="read-back-failed"
      className={
        'flex flex-wrap items-center gap-x-2 gap-y-1 rounded-md border border-warning/40 bg-warning/5 p-3 text-sm ' +
        (className ?? '')
      }
    >
      <StatusBadge status="Not read back" tone="warning" />
      <span className="min-w-0 break-words">{children}</span>
      <Button
        ref={readAgainRef}
        variant="link"
        size="sm"
        aria-disabled={reading || undefined}
        className="aria-disabled:cursor-progress aria-disabled:opacity-70"
        onClick={() => {
          if (!reading) onReadAgain()
        }}
      >
        {readAgainLabel}
      </Button>
      {reading !== undefined && (
        <span className="text-xs text-muted-foreground" role="status">
          {reading ? 'Reading…' : ''}
        </span>
      )}
    </div>
  )
}
