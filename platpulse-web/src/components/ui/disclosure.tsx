import type { ReactNode } from 'react'
import { ChevronRight } from 'lucide-react'

import { cn } from '../../lib/utils'
import { SURFACE_CARD_DISCLOSURE } from '../../lib/surface'

/**
 * The one disclosure for every collapsible Node Detail region. Native
 * `details`/`summary` keeps keyboard operation, touch, in-page find and the
 * no-script state for free, so this primitive adds presentation only: the
 * chevron, the read-only disclosure surface, and the title/description tiers.
 *
 * `surface="none"` borrows that presentation without painting a second card
 * inside an existing one. The Validator diagnostics need it: they describe the
 * Linked Validator card they sit in, so they must read as the same disclosure
 * as the page-level ones without nesting a card in a card.
 *
 * Emerald ships no disclosure component; this is PlatPulse's own, built from
 * Emerald's own tokens and its existing summary anatomy (see
 * docs/adr/0002-webui-emerald-visual-authority.md).
 */
export function Disclosure({
  title,
  description,
  surface = 'card',
  className,
  children,
}: {
  title: ReactNode
  /** The short "what is in here" line under the title. */
  description?: ReactNode
  surface?: 'card' | 'none'
  className?: string
  children: ReactNode
}) {
  const card = surface === 'card'
  return <details
    data-slot="disclosure"
    // Marks which presentation was requested, so a caller (or a test) can tell
    // the page-level card from the one nested inside an existing card.
    data-surface={surface}
    className={cn(
      'group min-w-0 overflow-hidden',
      card && cn('mt-4 rounded-md border-none', SURFACE_CARD_DISCLOSURE),
      className,
    )}
  >
    <summary className={cn(
      'flex min-h-11 cursor-pointer list-none flex-wrap items-baseline gap-x-3 py-2 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring [&::-webkit-details-marker]:hidden',
      card && 'px-4',
    )}>
      <ChevronRight data-slot="disclosure-marker" size={16} aria-hidden="true" className="shrink-0 self-center text-muted-foreground transition-transform group-open:rotate-90" />
      <span className="font-semibold">{title}</span>
      {description && <span className="basis-full pl-7 text-[11px] leading-4 text-muted-foreground">{description}</span>}
    </summary>
    <div className={cn('grid min-w-0 gap-3', card ? 'px-4 pb-4' : 'pb-2')}>{children}</div>
  </details>
}
