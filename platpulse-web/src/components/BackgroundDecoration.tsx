import { useId } from 'react'

/**
 * Decorative-only Emerald background.
 *
 * Ported from Tokinx/komari-theme-emerald @
 * c2c5e88ea19c7cbe18d14a50414e10deca3cc66e src/components/Background.vue (MIT,
 * Copyright (c) 2026 Tokinx). The layer geometry is upstream's, class for
 * class: a fixed z-index:-1 canvas, the slate base, a 1300x400px emerald-to-lime
 * wash at -ml-152 under a "farthest-side at top" radial mask at opacity .4, and
 * the inclined 72x56 grid with its four filled cells blended through
 * mix-blend-overlay. Dark swaps to the 30% wash, a vertical mask and the
 * white 2.5%/5% grid.
 *
 * An earlier PlatPulse port stretched the wash across the whole viewport; that
 * adaptation is removed so the rendered result is upstream's.
 *
 * Deliberately isolated from the content tree: never receives pointer or
 * keyboard input and stays out of the accessibility tree.
 */
export default function BackgroundDecoration() {
  const patternId = `platpulse-grid-${useId().replaceAll(':', '')}`

  return (
    <div data-slot="background-decoration" className="fixed inset-0 -z-1 overflow-hidden" aria-hidden="true">
      <div className="absolute inset-0 mx-0 max-w-none overflow-hidden bg-slate-50 dark:bg-slate-900/50">
        <div data-slot="background-decoration-atmosphere" className="absolute top-0 left-1/2 -ml-152 h-100 w-325 dark:mask-[linear-gradient(white,transparent)]">
          <div data-slot="background-decoration-gradient" className="absolute inset-0 bg-linear-to-r from-emerald-500 to-lime-300 mask-[radial-gradient(farthest-side_at_top,white,transparent)] opacity-40 dark:from-emerald-500/30 dark:to-lime-300/30 dark:opacity-100">
            <svg
              aria-hidden="true"
              focusable="false"
              data-slot="background-decoration-grid"
              className="absolute inset-x-0 inset-y-[-50%] h-[200%] w-full skew-y-[-18deg] fill-black/40 stroke-black/50 mix-blend-overlay dark:fill-white/2.5 dark:stroke-white/5"
            >
              <defs>
                <pattern id={patternId} width="72" height="56" patternUnits="userSpaceOnUse" x="-12" y="4">
                  <path d="M.5 56V.5H72" fill="none" />
                </pattern>
              </defs>
              <rect width="100%" height="100%" strokeWidth="0" fill={`url(#${patternId})`} />
              <svg x="-12" y="4" className="overflow-visible">
                <rect strokeWidth="0" width="73" height="57" x="288" y="168" />
                <rect strokeWidth="0" width="73" height="57" x="144" y="56" />
                <rect strokeWidth="0" width="73" height="57" x="504" y="168" />
                <rect strokeWidth="0" width="73" height="57" x="720" y="336" />
              </svg>
            </svg>
          </div>
        </div>
      </div>
    </div>
  )
}
