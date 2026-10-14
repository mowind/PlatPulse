import type { ReactNode } from 'react'
import { cn } from '../lib/utils'

import { ProgressThin, type ProgressStatus } from './ui/progress-thin'

export type MetricRowProps = {
  label: string
  /** `compact` is the ordinary left-label/right-value key-value line used on
   *  the Home Node card; `inline` is the Node-detail form. */
  layout?: 'inline' | 'compact'
  shortLabel?: string
  value: ReactNode
  detail?: ReactNode
  progress?: number | null
  progressStatus?: ProgressStatus
  /** Extra classes for the row itself. The Linked Validator grid uses this to
   *  let one over-long exact value span the whole row. */
  className?: string
}

/**
 * One compact metric row in Emerald's anatomy (NodeCard.vue): the muted label
 * at the left and the value flush right, an optional 4px ProgressThin bar
 * underneath, and an 11px muted caption under the bar. `compact` is the
 * ordinary key-value line (13px value, no wrap); `inline` adds the wider
 * Node-detail wrap behaviour.
 *
 * The DOM keeps label and value adjacent so the value is the label's next
 * sibling; the progress bar and caption span both grid columns. A row without
 * a known progress value reserves unpainted space, never a zero-like track.
 *
 */
export function MetricRow({ label, shortLabel, value, detail, progress, progressStatus, className, layout = 'inline' }: MetricRowProps) {
  const compact = layout === 'compact'
  return (
    <div
      data-slot="metric-row"
      data-kind={progress !== undefined ? 'resource' : 'information'}
      data-layout={layout}
      className={cn('grid min-w-0 grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-2 gap-y-1 text-xs', className)}
    >
      <span
        data-slot="metric-row-label"
        className={cn(
          'flex items-baseline whitespace-nowrap text-muted-foreground',
          compact && 'font-normal leading-[1.375rem]',
        )}
      >
        {shortLabel ? (
          <>
            <span data-full-label>{label}</span>
            <span data-short-label aria-label={label} title={label}>{shortLabel}</span>
          </>
        ) : (
          label
        )}
      </span>
      <strong
        data-slot="metric-row-value"
        className={cn(
          'min-w-0 tabular-nums text-foreground',
          compact
            ? 'whitespace-nowrap text-right text-[13px] font-medium leading-[1.375rem]'
            : 'text-right font-medium [overflow-wrap:anywhere]',
        )}
      >
        {value}
      </strong>
      {typeof progress === 'number' && (
        <ProgressThin className="col-span-2" percentage={progress} status={progressStatus} label={label} />
      )}
      {progress === null && <span data-slot="metric-unknown-space" className="col-span-2 h-1" aria-hidden="true" />}
      {detail != null && (
        <small
          data-slot="metric-row-detail"
          className={cn('col-span-2 text-[11px] text-muted-foreground', compact ? 'truncate' : 'whitespace-normal [overflow-wrap:anywhere]')}
        >
          {detail}
        </small>
      )}
    </div>
  )
}
