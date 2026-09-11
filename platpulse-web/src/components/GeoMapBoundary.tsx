import { Component, type ErrorInfo, type ReactNode } from 'react'
import { PEER_COUNTRIES_HEADING } from './geoPresentation'

type GeoMapBoundaryProps = { children: ReactNode }
type GeoMapBoundaryState = { failed: boolean }

/**
 * Home must survive a Peer country map that cannot render (issue #133). A
 * basemap, Geo, or geometry failure degrades inside the map slot: the four
 * global statistics, the Network filter, the sort control, and every Node
 * card keep working.
 *
 * The map carries no visible explanation any more, so this fallback paints
 * nothing: it keeps the slot stable and announces the failure to assistive
 * technology instead of writing a glyph or a sentence onto the map.
 */
export default class GeoMapBoundary extends Component<GeoMapBoundaryProps, GeoMapBoundaryState> {
  state: GeoMapBoundaryState = { failed: false }

  static getDerivedStateFromError(): GeoMapBoundaryState {
    return { failed: true }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // Keep the diagnosis in the console; the UI stays a local, non-leaking
    // statement instead of an internal error surface.
    console.error('Peer country map failed to render', error, info.componentStack)
  }

  render() {
    if (this.state.failed) {
      return (
        <section className="home-geo home-geo-disabled" aria-label={PEER_COUNTRIES_HEADING}>
          <span className="sr-only" role="status">Map unavailable</span>
        </section>
      )
    }
    return this.props.children
  }
}
