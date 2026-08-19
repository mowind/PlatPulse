import { useCallback, useEffect } from 'react'
import { useLocation, useNavigate } from 'react-router'

export type PrototypeVariant = {
  key: string
  label: string
}

type PrototypeSwitcherProps = {
  current: string
  variants: readonly PrototypeVariant[]
}

/** PROTOTYPE ONLY — shareable dev switcher for visual exploration. */
export default function PrototypeSwitcher({ current, variants }: PrototypeSwitcherProps) {
  const location = useLocation()
  const navigate = useNavigate()
  const currentIndex = Math.max(0, variants.findIndex((variant) => variant.key === current))

  const setVariant = useCallback((index: number) => {
    const next = variants[(index + variants.length) % variants.length]
    if (!next) return
    const search = new URLSearchParams(location.search)
    search.set('variant', next.key)
    void navigate({ pathname: location.pathname, search: '?' + search.toString() }, { replace: true })
  }, [location.pathname, location.search, navigate, variants])

  useEffect(() => {
    if (!import.meta.env.DEV) return
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target
      if (target instanceof HTMLElement && target.matches('input, textarea, select, [contenteditable="true"]')) return
      if (event.key === 'ArrowLeft') {
        event.preventDefault()
        setVariant(currentIndex - 1)
      }
      if (event.key === 'ArrowRight') {
        event.preventDefault()
        setVariant(currentIndex + 1)
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [currentIndex, setVariant])

  if (!import.meta.env.DEV) return null
  const active = variants[currentIndex]
  return (
    <nav className="prototype-switcher" aria-label="Prototype variants">
      <button type="button" aria-label="Previous prototype variant" onClick={() => setVariant(currentIndex - 1)}>←</button>
      <span><strong>{active?.key}</strong><span aria-hidden="true"> — </span>{active?.label}</span>
      <button type="button" aria-label="Next prototype variant" onClick={() => setVariant(currentIndex + 1)}>→</button>
    </nav>
  )
}
