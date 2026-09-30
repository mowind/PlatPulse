import type { ReactNode } from 'react'
import { cn } from '../lib/utils'

/**
 * The one overview band Home and Node Detail both render: six summary metrics
 * beside the Peer map. Sharing the actual element removes the two near-duplicate
 * CSS sets that let the pages drift apart — card size, the 5:6 column split, the
 * map track (proportional 2:1 below xl, the fixed 22rem band from xl) and every
 * responsive breakpoint now come from this single definition.
 *
 * `items-end` matches Home's accepted behaviour: the metrics grid is its own
 * content height and sits on the band's floor, so the map band does not stretch
 * the tiles taller than a Home card.
 *
 * The DOM keeps the metrics before the map for assistive reading. `mapFirst`
 * restores Home's upstream visual order below lg, where the map sits above the
 * six statistics; Node Detail keeps its tiles first at every width.
 */
export function OverviewBand({ dataSlot, mapSlot, metricsLabel, metrics, map, className, mapFirst = false }: {
  /** `data-slot` of the band; each page keeps its own contract. */
  dataSlot: string
  /** `data-slot` of the map track; each page keeps its own contract. */
  mapSlot: string
  /** Accessible name of the metrics group. */
  metricsLabel: string
  metrics: ReactNode
  map: ReactNode
  /** Context spacing (page padding or top margin); the band owns the grid only. */
  className?: string
  /** Place the map above the metrics below lg (Home's order). */
  mapFirst?: boolean
}) {
  return (
    <div data-slot={dataSlot} className={cn('grid min-w-0 items-end gap-4 lg:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]', className)}>
      <div className="grid min-w-0 auto-rows-fr grid-cols-2 gap-2 sm:grid-cols-3" role="group" aria-label={metricsLabel}>
        {metrics}
      </div>
      {/* Below xl the track is proportional (2:1) so phones and tablets keep the
          compact map the mobile acceptance measured. From xl, where the band
          stops changing width, the track uses upstream's fixed 22rem band. The
          map fills whichever band it is given, so the world is never cropped. */}
      <div data-slot={mapSlot} className={cn('min-w-0 aspect-[2/1] xl:aspect-auto xl:h-88', mapFirst && 'order-first lg:order-none')}>
        {map}
      </div>
    </div>
  )
}
