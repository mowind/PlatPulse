import { useEffect, useRef, type ReactNode, type RefObject } from 'react'

/** Non-modal, click/keyboard/touch disclosure. It never traps the Home filter. */
export default function MapInformation({ id, trigger, fallback, onClose, children }: {
  id: string
  trigger: RefObject<HTMLButtonElement | null>
  /** Used when the opening control unmounted while the panel stayed open. */
  fallback?: RefObject<HTMLButtonElement | null>
  onClose: () => void
  children: ReactNode
}) {
  const panel = useRef<HTMLDivElement>(null)
  const close = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    close.current?.focus()
    const element = panel.current
    const opener = trigger.current
    // The permanent information control always exists, so it is the safe
    // fallback when the opening control unmounted while the panel stayed open.
    const alternative = fallback?.current
    // Dismiss after the browser applies pointer focus. Closing on pointerdown
    // lets its later default action erase the focus restored during cleanup.
    function dismiss(event: MouseEvent) {
      if (event.target instanceof Node && !element?.contains(event.target) && !opener?.contains(event.target)) onClose()
    }
    function escape(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault()
        onClose()
      }
    }
    document.addEventListener('click', dismiss)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('click', dismiss)
      document.removeEventListener('keydown', escape)
      if (element?.contains(document.activeElement) || document.activeElement === document.body) {
        const target = opener?.isConnected ? opener : alternative
        target?.focus()
      }
    }
  }, [fallback, onClose, trigger])
  return (
    <div id={id} ref={panel} className="home-geo-information" role="dialog" aria-label="Map information">
      <header>
        <h3>Map information</h3>
        <button ref={close} type="button" className="home-geo-icon" aria-label="Close map information" onClick={onClose}>
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 6 12 12M6 18 18 6" /></svg>
        </button>
      </header>
      {children}
    </div>
  )
}
