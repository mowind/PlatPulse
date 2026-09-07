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
 * b7baf4535939cfdda063d731943fc36e3ead4c51 (MIT; see attribution in the
 * design handoff). The texture remains inside the masked gradient layer so
 * both the color wash and its geometry fade as one surface.
 */
export default function BackgroundDecoration({ showGrid = true }: BackgroundDecorationProps) {
  const patternId = `platpulse-grid-${useId().replaceAll(':', '')}`

  return (
    <div className="background-decoration" aria-hidden="true">
      <div className="background-decoration-gradient">
        {showGrid && (
          <svg
            className="background-decoration-grid"
            viewBox="0 0 1000 700"
            preserveAspectRatio="none"
            focusable="false"
          >
            <defs>
              <pattern id={patternId} width="72" height="56" patternUnits="userSpaceOnUse" patternTransform="translate(-12 4)">
                <path d="M.5 56V.5H72" fill="none" />
              </pattern>
            </defs>
            <rect width="100%" height="100%" strokeWidth="0" fill={`url(#${patternId})`} />
            <g className="background-decoration-cells" transform="translate(-12 4)">
              <rect strokeWidth="0" width="73" height="57" x="288" y="168" />
              <rect strokeWidth="0" width="73" height="57" x="144" y="56" />
              <rect strokeWidth="0" width="73" height="57" x="504" y="168" />
              <rect strokeWidth="0" width="73" height="57" x="720" y="336" />
            </g>
          </svg>
        )}
      </div>
    </div>
  )
}
