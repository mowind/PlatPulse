/**
 * Admin detail sections (webui.md §8.7). Admin Node and Agent detail pages
 * keep a compact, always-visible header and split everything else into four
 * sections — Overview, History, Diagnostics, Manage — built on the existing
 * Radix Tabs. The chosen section is presentation state in the URL
 * (`?section=history`), so a reload, a shared link and back/forward reopen
 * it, and an older in-page anchor (`#audit`, `#credentials`) still opens
 * the section that holds its target and scrolls to it. No route is added.
 *
 * A section mounts on its first visit and then stays mounted but `hidden`:
 * an unsaved draft, an open confirmation or a one-time result survives a
 * section switch, while the hidden controls leave the keyboard order and the
 * accessibility tree. A section holding a draft or an open confirmation says
 * so on its tab, so leaving it is never a silent loss.
 */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from 'react'
import { ArrowLeft } from 'lucide-react'
import { Link, useLocation, useSearchParams } from 'react-router'
import { cn } from '../lib/utils'
import { Button } from './ui/button'
import { StatusBadge } from './StatusBadge'
import { Tabs, TabsContent, TabsList, TabsTrigger } from './ui/tabs'

export const DETAIL_SECTIONS = ['overview', 'history', 'diagnostics', 'manage'] as const
export type DetailSection = (typeof DETAIL_SECTIONS)[number]

function isDetailSection(value: string | null): value is DetailSection {
  return value !== null && (DETAIL_SECTIONS as readonly string[]).includes(value)
}

const NO_ANCHORS: Readonly<Record<string, DetailSection>> = {}

/** The open section: `?section=` first, then a legacy in-page anchor mapped
 *  to the section that holds it, then Overview. Returns the section, a
 *  selector for the tab list, and `open`, which also moves focus to the
 *  opened section so a jump button that disappears never drops focus. */
export function useDetailSection(
  anchors: Readonly<Record<string, DetailSection>> = NO_ANCHORS,
): [DetailSection, (next: DetailSection) => void, (next: DetailSection) => void] {
  const [search, setSearch] = useSearchParams()
  const location = useLocation()
  const requested = search.get('section')
  const anchor = location.hash.replace(/^#/, '')
  const section: DetailSection = isDetailSection(requested)
    ? requested
    : (anchors[anchor] ?? 'overview')

  // An anchor link opens its section and then reaches its target, which only
  // exists once the section has mounted.
  useEffect(() => {
    if (!anchor || anchors[anchor] !== section) return
    const frame = requestAnimationFrame(() => {
      const target = document.getElementById(anchor)
      if (target && typeof target.scrollIntoView === 'function') target.scrollIntoView({ block: 'start' })
    })
    return () => cancelAnimationFrame(frame)
  }, [anchor, anchors, section])

  const select = useCallback(
    (next: DetailSection) => {
      const params = new URLSearchParams(search)
      if (next === 'overview') params.delete('section')
      else params.set('section', next)
      // Replace, not push: switching sections is not a navigation step, but
      // the section still survives reload and a return from another page.
      setSearch(params, { replace: true })
    },
    [search, setSearch],
  )
  const open = useCallback(
    (next: DetailSection) => {
      select(next)
      requestAnimationFrame(() => {
        document
          .querySelector<HTMLElement>('[data-slot="admin-detail-section"][data-section="' + next + '"]')
          ?.focus()
      })
    },
    [select],
  )
  return [section, select, open]
}

/** The tab marker for a Manage section holding a draft or open confirmation. */
export function draftMarker(pending: ReadonlySet<string>): Pick<DetailSectionEntry, 'marker' | 'markerLabel'> {
  return pending.size > 0
    ? { marker: 'Unsaved', markerLabel: 'unsaved change or open confirmation' }
    : {}
}

/** The return link above a detail header. */
export function DetailBackLink({ to, children }: { to: string; children: ReactNode }) {
  return (
    <Link
      className="inline-flex min-h-11 min-w-11 items-center text-sm font-medium text-muted-foreground underline-offset-4 hover:underline"
      to={to}
    >
      <ArrowLeft size={16} aria-hidden="true" className="mr-1" /> {children}
    </Link>
  )
}

export const DETAIL_LINK =
  'inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline'

type DraftReporter = (key: string, pending: boolean) => void
const DraftContext = createContext<DraftReporter | null>(null)

/** Tracks which controls inside a section hold an unsaved draft or an open
 *  confirmation, so the section's tab can say so. */
export function useDetailDrafts(): { pending: ReadonlySet<string>; report: DraftReporter } {
  const [pending, setPending] = useState<ReadonlySet<string>>(() => new Set())
  const report = useCallback<DraftReporter>((key, active) => {
    setPending((current) => {
      if (current.has(key) === active) return current
      const next = new Set(current)
      if (active) next.add(key)
      else next.delete(key)
      return next
    })
  }, [])
  return { pending, report }
}

export const DetailDraftProvider = DraftContext.Provider

/** Report one control's unsaved draft or open confirmation to its section. */
export function useReportDraft(key: string, active: boolean) {
  const report = useContext(DraftContext)
  useEffect(() => {
    report?.(key, active)
  }, [report, key, active])
  useEffect(() => () => report?.(key, false), [report, key])
}

export type DetailSectionEntry = {
  key: DetailSection
  label: string
  /** Short visible marker on the tab (an issue count, "Unsaved"). */
  marker?: ReactNode
  /** Accessible wording for the marker, read after the label. */
  markerLabel?: string
  content: ReactNode
}

export function AdminDetailSections({
  label,
  value,
  onValueChange,
  sections,
}: {
  label: string
  value: DetailSection
  onValueChange: (next: DetailSection) => void
  sections: DetailSectionEntry[]
}) {
  const [visited, setVisited] = useState<ReadonlySet<DetailSection>>(() => new Set([value]))
  if (!visited.has(value)) setVisited(new Set(visited).add(value))
  return (
    <Tabs
      value={value}
      onValueChange={(next) => {
        if (isDetailSection(next)) onValueChange(next)
      }}
      data-slot="admin-detail-sections"
      className="min-w-0 gap-4"
    >
      <div className="min-w-0 overflow-x-auto border-b border-border/60">
        <TabsList variant="line" aria-label={label} className="justify-start">
          {sections.map((section) => (
            <TabsTrigger key={section.key} value={section.key} className="flex-none px-3">
              {section.label}
              {section.marker && (
                <span
                  aria-hidden="true"
                  className="rounded-full bg-warning/15 px-1.5 text-xs font-medium tabular-nums text-foreground"
                >
                  {section.marker}
                </span>
              )}
              {section.markerLabel && <span className="sr-only">{', ' + section.markerLabel}</span>}
            </TabsTrigger>
          ))}
        </TabsList>
      </div>
      {sections.map((section) => (
        <TabsContent
          key={section.key}
          value={section.key}
          forceMount
          // Mounted after the first visit so drafts survive a switch; hidden
          // so its controls leave the tab order and the accessibility tree.
          hidden={section.key !== value}
          data-slot="admin-detail-section"
          data-section={section.key}
          // One shrinkable track: a wide history table scrolls inside its own
          // card instead of widening the section past the page.
          className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-4 rounded-md focus-visible:outline-2 focus-visible:outline-offset-4 focus-visible:outline-ring"
        >
          {visited.has(section.key) ? section.content : null}
        </TabsContent>
      ))}
    </Tabs>
  )
}

export type DetailNotice = {
  key: string
  tone: 'error' | 'warning' | 'neutral'
  status: string
  message: ReactNode
  /** The section holding the complete evidence. */
  section?: DetailSection
  sectionLabel?: string
}

/** Current conditions that stay visible above every section, each with the
 *  section that holds its complete evidence. */
export function DetailNotices({
  notices,
  current,
  onOpen,
  label,
}: {
  notices: DetailNotice[]
  current: DetailSection
  onOpen: (section: DetailSection) => void
  label: string
}) {
  if (notices.length === 0) return null
  return (
    <ul role="list" aria-label={label} data-slot="detail-notices" className="grid min-w-0 gap-1.5">
      {notices.map((notice) => (
        <li
          key={notice.key}
          data-tone={notice.tone}
          className={cn(
            'flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 rounded-md border border-l-[3px] border-border/60 bg-background/60 px-3 py-1.5 text-sm',
            notice.tone === 'error' && 'border-l-destructive',
            notice.tone === 'warning' && 'border-l-warning',
            notice.tone === 'neutral' && 'border-l-muted-foreground',
          )}
        >
          <StatusBadge status={notice.status} tone={notice.tone} />
          <span className="min-w-0 flex-1 basis-48 [overflow-wrap:anywhere]">{notice.message}</span>
          {notice.section && notice.section !== current && (
            <Button variant="link" size="sm" className="h-auto min-h-11 px-0" onClick={() => onOpen(notice.section!)}>
              {'Open ' + (notice.sectionLabel ?? notice.section)}
            </Button>
          )}
        </li>
      ))}
    </ul>
  )
}

/** One labelled, independent dimension in a detail header. */
export function DetailDimension({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="m-0 min-w-0 text-sm [overflow-wrap:anywhere]">{children}</dd>
    </div>
  )
}

export function DetailDimensions({ label, children }: { label: string; children: ReactNode }) {
  return (
    <dl aria-label={label} data-slot="detail-dimensions" className="flex min-w-0 flex-wrap gap-x-6 gap-y-2">
      {children}
    </dl>
  )
}
