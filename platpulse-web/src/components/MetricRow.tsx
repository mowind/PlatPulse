import type { ReactNode } from 'react'

import { ProgressThin, type ProgressStatus } from './ui/progress-thin'

export type MetricRowProps = {
  label: string
  shortLabel?: string
  value: ReactNode
  detail?: ReactNode
  progress?: number | null
  progressStatus?: ProgressStatus
}

/**
 * One compact metric row in Emerald's anatomy (NodeCard.vue): a text-xs line
 * with the muted label at the left and the value flush right, an optional
 * 4px ProgressThin bar underneath, and an 11px muted caption under the bar.
 *
 * The DOM keeps label and value adjacent so the value is the label's next
 * sibling; the progress bar and caption span both grid columns. A row without
 * a known progress value renders the track only — never a 0-width bar that
 * would read as a real zero.
 */
export function MetricRow({ label, shortLabel, value, detail, progress, progressStatus }: MetricRowProps) {
  return (
    <div
      data-slot="metric-row"
      className="grid grid-cols-[minmax(0,1fr)_auto] items-baseline gap-x-2 gap-y-1 text-xs"
    >
      <span data-slot="metric-row-label" className="min-w-0 truncate text-muted-foreground">
        {shortLabel ? (
          <>
            <span>{label}</span>
            <span className="hidden" aria-label={label} title={label}>{shortLabel}</span>
          </>
        ) : (
          label
        )}
      </span>
      <strong data-slot="metric-row-value" className="font-medium tabular-nums text-foreground">
        {value}
      </strong>
      {typeof progress === 'number' && (
        <ProgressThin className="col-span-2" percentage={progress} status={progressStatus} label={label} />
      )}
      {detail != null && (
        <small data-slot="metric-row-detail" className="col-span-2 truncate text-[11px] text-muted-foreground">
          {detail}
        </small>
      )}
    </div>
  )
}
