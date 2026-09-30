import type { ReactNode } from 'react'
import type { LucideIcon } from 'lucide-react'
import { CardX } from './ui/card-x'
import { SURFACE_CARD, SURFACE_CARD_SUMMARY } from '../lib/surface'
import { cn } from '../lib/utils'

/**
 * One shared Emerald summary surface, used by Home's six overview metrics and
 * by Node Detail's six key metrics, so a card is the same size on both pages: a
 * 44px header row holding the title and the trailing icon (or the tile's own
 * detail control), then the value and its directly visible stat line. The value
 * sits immediately under the uniform header — not pinned to the card bottom —
 * so a taller caption in one tile cannot shift its value off the shared
 * baseline; `auto-rows-fr` keeps all six the same height.
 */
export function SummaryMetricCard({ label, value, icon: Icon, tone = 'green', action, caption, hoverable = true, dataSlot = 'summary-card' }: {
  label: string
  value: ReactNode
  /** Trailing metric icon; omit it for a read-only tile that carries none. */
  icon?: LucideIcon
  tone?: 'green' | 'red'
  action?: ReactNode
  /** Directly visible stat line under the value. Every tile reserves the same
   *  slot so all six keep one height and one value baseline. */
  caption?: ReactNode
  /** Home's tiles carry the interactive hover ring; a read-only tile (Node
   *  Detail) keeps the same resting surface without it. */
  hoverable?: boolean
  /** `data-slot` of the tile; Node Detail keeps `node-summary-tile`. */
  dataSlot?: string
}) {
  return <CardX hoverable={hoverable} bordered={false} role="article" aria-label={label} size="small"
    data-slot={dataSlot} data-tone={tone}
    className={cn('min-w-0 rounded-md transition-all', hoverable ? SURFACE_CARD : SURFACE_CARD_SUMMARY)}
    contentClassName="flex h-full min-w-0 flex-col gap-1 !p-3">
    <div className="flex min-h-11 items-start justify-between gap-1">
      <span className="min-w-0 text-xs font-medium tracking-wider text-muted-foreground">{label}</span>
      {action ?? (Icon ? <Icon size={18} strokeWidth={2} className="shrink-0" aria-hidden="true" data-icon={label} /> : null)}
    </div>
    <div className="flex min-w-0 flex-col gap-0.5">
      <strong data-slot="summary-value" className={cn('text-base font-bold leading-none tracking-tight tabular-nums md:text-2xl [overflow-wrap:anywhere]', tone === 'red' && 'text-destructive')}>{value}</strong>
      <span data-slot="summary-caption" className="min-h-4 text-[11px] leading-4 text-muted-foreground [overflow-wrap:anywhere]">{caption}</span>
    </div>
  </CardX>
}
