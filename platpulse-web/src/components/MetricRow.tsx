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
 * a known progress value reserves unpainted space, never a zero-like track.
 */
export function MetricRow({ label, shortLabel, value, detail, progress, progressStatus }: MetricRowProps) {
  return (
    <div
      data-slot="metric-row"
      data-kind={progress !== undefined ? "resource" : "information"}
      className="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-2 gap-y-1 text-xs"
    >
      <span data-slot="metric-row-label" className="metric-label flex items-baseline whitespace-nowrap text-muted-foreground">
        {shortLabel ? (
          <>
            <span data-full-label>{label}</span>
            <span data-short-label aria-label={label} title={label}>{shortLabel}</span>
          </>
        ) : (
          label
        )}
      </span>
      <strong data-slot="metric-row-value" className="min-w-0 text-right font-medium tabular-nums text-foreground [overflow-wrap:anywhere]">
        {value}
      </strong>
      {typeof progress === 'number' && (
        <ProgressThin className="col-span-2" percentage={progress} status={progressStatus} label={label} />
      )}
      {progress === null && <span data-slot="metric-unknown-space" className="col-span-2 h-1" aria-hidden="true" />}
      {detail != null && (
        <small data-slot="metric-row-detail" className="col-span-2 truncate text-[11px] text-muted-foreground">
          {detail}
        </small>
      )}
    </div>
  )
}
