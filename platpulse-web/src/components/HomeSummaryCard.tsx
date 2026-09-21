import type { ReactNode } from 'react'
import type { LucideIcon } from 'lucide-react'
import { CardX } from './ui/card-x'
import { SURFACE_CARD } from '../lib/surface'
import { cn } from '../lib/utils'

/**
 * One shared Emerald surface for all six Home overview metrics, restored to the
 * compact height of the original four-card 2x2 grid: a 44px header row holding
 * the title and the trailing icon (or the tile's own detail control), then the
 * value pinned to the bottom. `auto-rows-fr` keeps all six equal.
 */
export function HomeSummaryCard({ label, value, icon: Icon, tone = 'green', action }: {
  label: string
  value: ReactNode
  icon: LucideIcon
  tone?: 'green' | 'red'
  action?: ReactNode
}) {
  return <CardX hoverable bordered={false} role="article" aria-label={label} size="small"
    data-slot="summary-card" data-tone={tone}
    className={cn('min-w-0 rounded-md transition-all', SURFACE_CARD)}
    contentClassName="flex h-full min-w-0 flex-col justify-between gap-1 !p-3">
    <div className="flex min-h-11 items-start justify-between gap-1">
      <span className="min-w-0 text-xs font-medium tracking-wider text-muted-foreground">{label}</span>
      {action ?? <Icon size={18} strokeWidth={2} className="shrink-0" aria-hidden="true" data-icon={label} />}
    </div>
    <strong data-slot="summary-value" className={cn('text-base font-bold leading-none tracking-tight tabular-nums md:text-2xl [overflow-wrap:anywhere]', tone === 'red' && 'text-destructive')}>{value}</strong>
  </CardX>
}
