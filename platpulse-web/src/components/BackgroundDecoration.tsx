import { useId } from 'react'

type BackgroundDecorationProps = {
  /** Hide the texture while the light surface and layout are being tuned. */
  showGrid?: boolean
}

/**
 * Decorative-only Emerald background geometry. It is deliberately isolated
 * from the content tree and never receives pointer or keyboard input.
 *
 * Adapted from Tokinx/komari-theme-emerald Background.vue at
 * c2c5e88ea19c7cbe18d14a50414e10deca3cc66e (MIT). Keep this copyright and
 * license notice with any adaptation. The geometry is the actual SVG grid —
 * a 72×56 user-space pattern at origin x=-12/y=4, skewY(-18deg), and four
 * local filled cells — not an approximate pair of CSS line gradients. The
 * texture lives inside the masked gradient layer so both the colour wash and
 * its geometry fade as one surface.
 */
export default function BackgroundDecoration({ showGrid = true }: BackgroundDecorationProps) {
  const patternId = `platpulse-grid-${useId().replaceAll(':', '')}`

  return (
    <div className="background-decoration" aria-hidden="true">
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
  )
}
