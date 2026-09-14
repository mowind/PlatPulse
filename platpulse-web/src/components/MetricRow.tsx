import type { CSSProperties, ReactNode } from 'react'

export type MetricRowProps = {
  label: string
  shortLabel?: string
  value: ReactNode
  detail?: ReactNode
  progress?: number | null
}

/**
 * One compact metric row (issue #140): the data item sits at the left and the
 * value sits flush right on the same line, and every row is a single column at
 * every breakpoint. A row with progress puts a full-width bar under the value
 * line and its explanation under the bar; a row without progress puts the
 * explanation directly under the value.
 */
export function MetricRow({ label, shortLabel, value, detail, progress }: MetricRowProps) {
  const boundedProgress = progress == null ? null : Math.max(0, Math.min(100, progress))
  const style = boundedProgress == null ? undefined : ({ '--metric-progress': `${boundedProgress}%` } as CSSProperties)
  return (
    <div className="metric-row" style={style}>
      <span className="metric-row-label">{shortLabel ? <>
        <span className="metric-label-full">{label}</span>
        <span className="metric-label-short" aria-label={label} title={label}>{shortLabel}</span>
      </> : label}</span>
      <strong className="metric-row-value">{value}</strong>
      {boundedProgress != null && <i className="metric-row-progress" aria-hidden="true" />}
      {detail != null && <small className="metric-row-detail">{detail}</small>}
    </div>
  )
}
