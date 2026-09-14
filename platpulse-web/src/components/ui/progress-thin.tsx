import { cn } from '../../lib/utils'

export type ProgressStatus = 'success' | 'warning' | 'error' | 'info' | 'default'

const FILL: Record<ProgressStatus, string> = {
  success: 'bg-success',
  warning: 'bg-warning',
  error: 'bg-destructive',
  info: 'bg-info',
  default: 'bg-primary',
}

/**
 * ProgressThin, ported from emerald src/components/ui/progress-thin/ProgressThin.vue.
 *
 * PlatPulse rule: an unknown percentage must never render as a zero-width or
 * empty-looking bar, so a null percentage draws only the track and exposes no
 * aria-valuenow at all.
 */
export function ProgressThin({
  percentage,
  status = 'default',
  height = 4,
  className,
  label,
}: {
  percentage: number | null | undefined
  status?: ProgressStatus
  height?: number
  className?: string
  label?: string
}) {
  const known = typeof percentage === 'number' && Number.isFinite(percentage)
  const clamped = known ? Math.max(0, Math.min(100, percentage)) : 0
  return (
    <div
      data-slot="progress-thin"
      role="progressbar"
      aria-label={label}
      aria-valuemin={known ? 0 : undefined}
      aria-valuemax={known ? 100 : undefined}
      aria-valuenow={known ? Math.round(clamped) : undefined}
      className={cn('relative w-full overflow-hidden rounded-full bg-muted', className)}
      style={{ height }}
    >
      {known && (
        <div
          data-slot="progress-thin-fill"
          className={cn('h-full rounded-full transition-[width] duration-300 ease-out', FILL[status])}
          style={{ width: clamped + '%' }}
        />
      )}
    </div>
  )
}
