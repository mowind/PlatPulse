/**
 * Admin presentation primitives (webui.md §8.5.2). One small set of page,
 * toolbar, panel, table and key/value building blocks so every Admin page reads
 * as the same product: a 20px page title with its actions on the same row, one
 * toolbar rhythm, static panels, one table head/cell rhythm, and one key/value
 * grid for expanded details. These are presentation only — they own no data,
 * no query state and no routing, and they never add hover lift to a static
 * surface. Public Home and Node Detail do not use them.
 */
import type { ReactNode } from 'react'
import { InfoHint } from './InfoHint'
import { CardX } from './ui/card-x'
import { SURFACE_CARD_STATIC } from '../lib/surface'
import { cn } from '../lib/utils'

/** Page title: ~20px semibold on every Admin page. */
export const ADMIN_PAGE_TITLE = 'text-xl font-semibold tracking-tight break-words'
/** Section/panel title: 14px semibold. */
export const ADMIN_SECTION_TITLE = 'text-sm font-semibold'
/** Static Admin panel surface: no border, no hover, no lift. */
export const ADMIN_PANEL_SURFACE = cn('rounded-md border-none', SURFACE_CARD_STATIC)
/** Table head cell: 12px medium, muted, one padding rhythm. */
export const ADMIN_TH = 'px-3 py-2 text-left text-xs font-medium text-muted-foreground'
/** Table body cell. Inside a stacked (data-stack) table on phones the card
 * already provides the inset, so the cell drops its own horizontal padding. */
export const ADMIN_TD =
  'px-3 py-2.5 align-top max-md:in-[table[data-stack]]:px-0 max-md:in-[table[data-stack]]:py-1.5'
/** Table row separator. */
export const ADMIN_ROW = 'border-b border-border/60 align-top'
/** Expanded detail row and its body. */
export const ADMIN_DETAIL_ROW = 'border-b border-border/60 bg-muted/30'
export const ADMIN_DETAIL_BODY = 'space-y-3 px-3 py-3 sm:px-4'
/** Secondary line under a primary value (short ID, kind, reason). */
export const ADMIN_SECONDARY = 'text-xs text-muted-foreground'

export function AdminPageHeader({
  title,
  description,
  info,
  actions,
  className,
}: {
  title: ReactNode
  /** One short sentence; longer static domain notes go into `info`. */
  description?: ReactNode
  /** Full static explanation behind a click/tap/focus-reachable control. Never
   * an error, risk, Unknown reason or stale-data notice. */
  info?: { label: string; content: ReactNode }
  /** Primary action, refresh, or data time; sits right of the title. */
  actions?: ReactNode
  className?: string
}) {
  return (
    <header
      data-slot="admin-page-header"
      className={cn('flex flex-wrap items-start justify-between gap-x-4 gap-y-2', className)}
    >
      <div className="min-w-0 flex-[1_1_18rem] space-y-0.5">
        <h1 className={ADMIN_PAGE_TITLE}>{title}</h1>
        {(description || info) && (
          <div className="flex min-w-0 items-start gap-1 text-sm text-muted-foreground">
            {description && <p className="min-w-0">{description}</p>}
            {info && (
              <InfoHint label={info.label}>
                {info.content}
              </InfoHint>
            )}
          </div>
        )}
      </div>
      {actions && (
        <div data-slot="admin-page-actions" className="flex flex-wrap items-center gap-2">
          {actions}
        </div>
      )}
    </header>
  )
}

/** Toolbar row: natural flow on wide screens, ordered wrap, two columns on phones. */
export function AdminToolbar({ label, children, className }: { label: string; children: ReactNode; className?: string }) {
  return (
    <div
      role="group"
      aria-label={label}
      data-slot="admin-toolbar"
      className={cn('grid grid-cols-2 gap-x-3 gap-y-2 sm:flex sm:flex-wrap sm:items-end', className)}
    >
      {children}
    </div>
  )
}

const FIELD_WIDTH = {
  sm: 'sm:w-36',
  md: 'sm:w-44',
  lg: 'col-span-2 sm:w-64',
} as const

/** One labelled toolbar control. Without `htmlFor` the label wraps the
 * control; with it, the label points at the control's id. */
export function AdminToolbarField({
  label,
  htmlFor,
  size = 'sm',
  children,
}: {
  label: string
  htmlFor?: string
  size?: keyof typeof FIELD_WIDTH
  children: ReactNode
}) {
  const captionClass = 'text-xs font-medium text-muted-foreground'
  const className = cn('flex min-w-0 flex-col gap-1', FIELD_WIDTH[size])
  if (htmlFor) {
    return (
      <div className={className} data-slot="admin-toolbar-field">
        <label htmlFor={htmlFor} className={captionClass}>{label}</label>
        {children}
      </div>
    )
  }
  return (
    <label className={className} data-slot="admin-toolbar-field">
      <span className={captionClass}>{label}</span>
      {children}
    </label>
  )
}

/** Static panel with a one-line header: title, optional count, actions. */
export function AdminPanel({
  title,
  count,
  info,
  actions,
  description,
  flush = false,
  className,
  children,
  slot,
  footer,
}: {
  title: ReactNode
  count?: ReactNode
  info?: ReactNode
  actions?: ReactNode
  description?: ReactNode
  /** Panel-wide action row under the content (e.g. a bulk action and its
   * confirmation); omitted when undefined. */
  footer?: ReactNode
  /** Content runs edge to edge (tables). */
  flush?: boolean
  className?: string
  children: ReactNode
  /** Page-specific data-slot hook kept from before the shared panel existed;
   * defaults to `admin-panel`. */
  slot?: string
}) {
  return (
    <CardX
      size="medium"
      bordered={false}
      segmented
      data-slot={slot ?? 'admin-panel'}
      className={cn(ADMIN_PANEL_SURFACE, className)}
      headerClassName="flex-wrap gap-y-1"
      contentClassName={flush ? 'p-0' : 'p-4'}
      footer={footer}
      header={
        <>
          <div className="flex min-w-0 flex-1 items-center gap-2">
            <h2 className={cn('min-w-0 break-words', ADMIN_SECTION_TITLE)}>{title}</h2>
            {info}
            {count}
          </div>
          {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
          {description && <p className="basis-full text-xs text-muted-foreground">{description}</p>}
        </>
      }
    >
      {children}
    </CardX>
  )
}

/** Key/value grid for expanded details: two columns from tablets up, one on phones. */
export function AdminKeyValueList({
  className,
  children,
}: {
  className?: string
  children: ReactNode
}) {
  return (
    <dl
      data-slot="admin-key-values"
      className={cn('grid grid-cols-1 gap-x-6 gap-y-2.5 sm:grid-cols-2', className)}
    >
      {children}
    </dl>
  )
}

/** One label/value pair. `wide` gives long IDs, hashes, paths and error text
 * their own full line so they wrap naturally instead of squeezing a column. */
export function AdminKeyValue({
  label,
  wide = false,
  children,
}: {
  label: ReactNode
  wide?: boolean
  children: ReactNode
}) {
  return (
    <div className={cn('min-w-0', wide && 'sm:col-span-full')}>
      <dt className="text-xs font-medium text-muted-foreground">{label}</dt>
      <dd className="mt-0.5 min-w-0 text-sm [overflow-wrap:anywhere]">{children}</dd>
    </div>
  )
}
