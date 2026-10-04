/**
 * Emerald detail grid (design §15.4 and the Owner surfaces it backs; webui.md
 * §15): a label above its value, stacked on narrow viewports. One owner, so
 * every Owner surface renders the same detail rows and they cannot drift apart.
 */
import type { ReactNode } from 'react'

export function DetailList({ children }: { children: ReactNode }) {
  return <dl className="grid grid-cols-1 gap-3 sm:grid-cols-2">{children}</dl>
}

export function DetailItem({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs font-medium tracking-wider text-muted-foreground">{label}</dt>
      <dd className="mt-0.5 min-w-0 break-words text-sm">{children}</dd>
    </div>
  )
}
