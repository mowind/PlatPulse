import type { ReactNode } from 'react'
import { cn } from '../lib/utils'

import { ProgressThin, type ProgressStatus } from './ui/progress-thin'

export type MetricRowProps = {
  label: string
  layout?: 'inline' | 'stacked'
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
export function MetricRow({ label, shortLabel, value, detail, progress, progressStatus, layout = 'inline' }: MetricRowProps) {
  return (
    <div
      data-slot="metric-row"
      data-kind={progress !== undefined ? "resource" : "information"}
      data-layout={layout}
      className={cn("grid min-w-0 items-baseline gap-x-2 gap-y-1 text-xs", layout === 'stacked' ? "grid-cols-1" : "grid-cols-[auto_minmax(0,1fr)]", layout === 'stacked' && typeof value === 'string' && value.length > 15 && "col-span-2")}
    >
      <span data-slot="metric-row-label" className={cn("metric-label flex items-baseline text-muted-foreground", layout === 'stacked' ? "whitespace-normal" : "whitespace-nowrap")}>
        {shortLabel ? (
          <>
            <span data-full-label>{label}</span>
            <span data-short-label aria-label={label} title={label}>{shortLabel}</span>
          </>
        ) : (
          label
        )}
      </span>
      <strong data-slot="metric-row-value" className={cn("min-w-0 font-medium tabular-nums text-foreground", layout === 'stacked' ? "whitespace-nowrap text-left text-sm" : "text-right [overflow-wrap:anywhere]")}>
        {value}
      </strong>
      {typeof progress === 'number' && (
        <ProgressThin className="col-span-2" percentage={progress} status={progressStatus} label={label} />
      )}
      {progress === null && <span data-slot="metric-unknown-space" className="col-span-2 h-1" aria-hidden="true" />}
      {detail != null && (
        <small data-slot="metric-row-detail" className={cn("text-[11px] text-muted-foreground", layout === 'stacked' ? "whitespace-normal" : "col-span-2 truncate")}>
          {detail}
        </small>
      )}
    </div>
  )
}
