import { useRef, type KeyboardEvent } from 'react'
import { cn } from '../lib/utils'
import { SURFACE_TOOLBAR } from '../lib/surface'
import { METRIC_WINDOW_LABELS, METRIC_WINDOWS, type MetricWindow } from '../metricWindow'

/**
 * The Node Detail Metric Window selector (docs/design/webui.md §15.38):
 * `Live · 1h · 6h · 24h · 7d` as one segmented control.
 *
 * It is one tab stop with arrow-key movement, like the Home Network pills: the
 * selected segment is the only one in the tab order, and Left/Right (Up/Down,
 * Home, End) move the selection and the focus together. It is a radio group
 * rather than a tab list because it selects what the six charts show instead
 * of switching between panels. Every segment is at least 44px tall and wide,
 * and the five fit one row at 360px.
 */
export function MetricWindowSelector({ value, onChange, className }: {
  value: MetricWindow
  onChange: (window: MetricWindow) => void
  className?: string
}) {
  const buttons = useRef<Array<HTMLButtonElement | null>>([])
  const move = (event: KeyboardEvent<HTMLDivElement>) => {
    const current = METRIC_WINDOWS.indexOf(value)
    const last = METRIC_WINDOWS.length - 1
    const next = event.key === 'ArrowRight' || event.key === 'ArrowDown'
      ? (current + 1) % METRIC_WINDOWS.length
      : event.key === 'ArrowLeft' || event.key === 'ArrowUp'
        ? (current - 1 + METRIC_WINDOWS.length) % METRIC_WINDOWS.length
        : event.key === 'Home'
          ? 0
          : event.key === 'End'
            ? last
            : undefined
    if (next === undefined) return
    event.preventDefault()
    const window = METRIC_WINDOWS[next]
    if (window === undefined) return
    buttons.current[next]?.focus()
    if (window !== value) onChange(window)
  }
  return (
    <div
      role="radiogroup"
      aria-label="Metric window"
      data-slot="metric-window-selector"
      onKeyDown={move}
      className={cn('inline-flex min-w-0 items-center gap-0.5 rounded-md p-0.5', SURFACE_TOOLBAR, className)}
    >
      {METRIC_WINDOWS.map((window, index) => {
        const selected = window === value
        return (
          <button
            key={window}
            ref={(element) => { buttons.current[index] = element }}
            type="button"
            role="radio"
            aria-checked={selected}
            tabIndex={selected ? 0 : -1}
            data-window={window}
            data-state={selected ? 'active' : 'inactive'}
            onClick={() => { if (!selected) onChange(window) }}
            className={cn(
              'inline-flex min-h-11 min-w-11 flex-1 items-center justify-center rounded-sm px-2 text-xs font-medium tabular-nums transition-colors',
              'focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-ring',
              selected
                ? 'bg-background text-emerald-600 dark:bg-input/30 dark:text-emerald-500'
                : 'text-muted-foreground hover:text-foreground',
            )}
          >
            {METRIC_WINDOW_LABELS[window]}
          </button>
        )
      })}
    </div>
  )
}
