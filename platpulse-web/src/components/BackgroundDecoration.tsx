import { useId } from 'react'

type BackgroundDecorationProps = {
  /** Hide the texture while the light surface and layout are being tuned. */
  showGrid?: boolean
}

/**
 * Decorative-only Emerald background, ported from
 * Tokinx/komari-theme-emerald `src/components/Background.vue` at
 * c2c5e88ea19c7cbe18d14a50414e10deca3cc66e (MIT). Keep this copyright and
 * license notice with any adaptation.
 *
 * Faithful port: the page canvas already supplies the upstream
 * `bg-slate-50` / dark neutral base, then a 400px atmosphere carries the
 * emerald-to-lime wash under the upstream `farthest-side at top` radial mask
 * at `opacity: .4`, with the inclined 72x56 SVG grid and its four local
 * filled cells blended through `mix-blend-overlay`. Dark swaps in the
 * emerald/lime 30% wash, a vertical atmosphere mask, and the white
 * 2.5% / 5% grid.
 *
 * The one deliberate deviation from upstream is width: the atmosphere spans
 * the full viewport instead of the upstream fixed 1300px, so it never reads
 * as a centred partial band on a wide viewport (design §11.1).
 *
 * Deliberately isolated from the content tree: never receives pointer or
 * keyboard input and stays out of the accessibility tree.
 */
export default function BackgroundDecoration({ showGrid = true }: BackgroundDecorationProps) {
  const patternId = `platpulse-grid-${useId().replaceAll(':', '')}`

  return (
    <div className="background-decoration" aria-hidden="true">
      <div className="background-decoration-atmosphere">
        <div className="background-decoration-gradient">
          {showGrid && (
            <svg className="background-decoration-grid" focusable="false">
              <defs>
                <pattern id={patternId} width="72" height="56" patternUnits="userSpaceOnUse" x="-12" y="4">
                  <path d="M.5 56V.5H72" fill="none" />
                </pattern>
              </defs>
              <rect width="100%" height="100%" strokeWidth="0" fill={`url(#${patternId})`} />
              <svg className="background-decoration-cells" x="-12" y="4" overflow="visible">
                <rect strokeWidth="0" width="73" height="57" x="288" y="168" />
                <rect strokeWidth="0" width="73" height="57" x="144" y="56" />
                <rect strokeWidth="0" width="73" height="57" x="504" y="168" />
                <rect strokeWidth="0" width="73" height="57" x="720" y="336" />
              </svg>
            </svg>
          )}
        </div>
      </div>
    </div>
  )
}
