import { Component, type ErrorInfo, type ReactNode } from 'react'

type GeoMapBoundaryProps = { children: ReactNode }
type GeoMapBoundaryState = { failed: boolean }

/**
 * Home must survive a Peer country map that cannot render (issue #133). A
 * basemap, Geo, or geometry failure degrades inside the map slot: the four
 * global statistics, the Network filter, the sort control, and every Node
 * card keep working, and the failure is stated in plain text.
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
        <section className="home-geo home-geo-disabled" aria-label="Peer countries">
          <p className="home-geo-note" role="status">Peer country map is Unavailable; the Node list below is unaffected.</p>
        </section>
      )
    }
    return this.props.children
  }
}
