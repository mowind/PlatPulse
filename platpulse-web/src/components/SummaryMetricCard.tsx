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
 *
 * The trailing slot is one box in every card (#240). A metric icon and the
 * Breakdown control are different sizes — 18px glyph beside a 44px target — so
 * the glyph used to sit on a different line in the two tile kinds: the plain
 * icon hung at the title's line and the control sat a dozen pixels below it.
 * Both now render inside the same `summary-marker` box, whose 44px square is
 * lifted into the card's own top padding: the six headers therefore share one
 * title line, one marker position and one value baseline by construction, and
 * the box is the target the interactive tiles already needed.
 */
export function SummaryMetricCard({ label, value, unit, icon: Icon, tone = 'green', action, caption, hint, accessibleName, hoverable = true, dataSlot = 'summary-card' }: {
  label: string
  /** The tile's full accessible name when the face abbreviates what it reads
   *  (the Balance tile's exact amount and full address); defaults to the label. */
  accessibleName?: string
  value: ReactNode
  /** Unit of measurement (e.g. LAT), displayed in a smaller secondary tier. */
  unit?: ReactNode
  /** Trailing metric icon; omit it for a read-only tile that carries none. */
  icon?: LucideIcon
  tone?: 'green' | 'red'
  action?: ReactNode
  /** Directly visible stat line under the value. Every tile reserves the same
   *  slot so all six keep one height and one value baseline. */
  caption?: ReactNode
  /** The longer reading of the same figure, exposed as the `title` of the
   *  value-and-caption reading so the visible caption can stay short and the
   *  explanation stays reachable without adding copy the compact shell would
   *  have to grow for. It deliberately does not travel through CardX's own
   *  `title` prop: that one is the card's *visible* header. */
  hint?: string
  /** Home's tiles carry the interactive hover ring; a read-only tile (Node
   *  Detail) keeps the same resting surface without it. */
  hoverable?: boolean
  /** `data-slot` of the tile; Node Detail keeps `node-summary-tile`. */
  dataSlot?: string
}) {
  return <CardX bordered={false} role="article" aria-label={accessibleName ?? label} size="small"
    data-slot={dataSlot} data-tone={tone}
    className={cn('min-w-0 rounded-md transition-all', hoverable && 'group', hoverable ? SURFACE_CARD : SURFACE_CARD_SUMMARY)}
    contentClassName="flex h-full min-w-0 flex-col gap-1 !p-3">
    <div className="flex min-h-11 items-start justify-between gap-1">
      <span className="min-w-0 text-xs font-medium tracking-wider text-muted-foreground">{label}</span>
      <span data-slot="summary-marker" className="-mt-3 -mr-2 grid size-11 shrink-0 place-items-center">
        {action ?? (Icon ? <Icon size={18} strokeWidth={2} className={cn('shrink-0', hoverable ? 'text-muted-foreground/30 transition-colors motion-reduce:transition-none group-hover:text-foreground' : 'text-muted-foreground')} aria-hidden="true" data-icon={label} /> : null)}
      </span>
    </div>
    <div data-slot="summary-reading" title={hint} className="flex min-w-0 flex-col gap-0.5">
      <strong data-slot="summary-value" className={cn('text-base font-bold leading-none tracking-tight tabular-nums md:text-2xl [overflow-wrap:anywhere]', tone === 'red' && 'text-destructive')}>
        {value}
        {unit != null && (
          <>
            {' '}
            <span data-slot="summary-unit" className="text-xs font-normal text-muted-foreground md:text-sm">
              {unit}
            </span>
          </>
        )}
      </strong>
      <span data-slot="summary-caption" className="min-h-4 text-[11px] leading-4 text-muted-foreground [overflow-wrap:anywhere]">{caption}</span>
    </div>
  </CardX>
}
