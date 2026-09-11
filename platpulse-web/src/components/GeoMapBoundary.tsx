import { Component, useCallback, useId, useRef, useState, type ErrorInfo, type ReactNode } from 'react'
import MapInformation from './MapInformation'

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
      return <MapRenderFailure onRetry={() => this.setState({ failed: false })} />
    }
    return this.props.children
  }
}

function MapRenderFailure({ onRetry }: { onRetry: () => void }) {
  const [open, setOpen] = useState(false)
  const trigger = useRef<HTMLButtonElement>(null)
  const id = useId()
  const close = useCallback(() => setOpen(false), [])
  return <section className="home-geo home-geo-disabled" aria-label="Peer countries">
    <span className="sr-only" role="status">Map unavailable</span>
    <div className="home-geo-heading">
      <button ref={trigger} type="button" className="home-geo-icon" aria-label="Map status: Map unavailable" aria-haspopup="dialog" aria-expanded={open} aria-controls={id} onClick={() => setOpen(true)}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="8" /><path d="M12 7v6M12 16v1" /></svg>
      </button>
    </div>
    {open && <MapInformation id={id} trigger={trigger} onClose={close}>
      <p>Peer country map is Unavailable; the Node list below is unaffected.</p>
      <p>The map could not be rendered. Retry to load it again.</p>
      <button type="button" onClick={onRetry}>Retry map</button>
    </MapInformation>}
  </section>
}
