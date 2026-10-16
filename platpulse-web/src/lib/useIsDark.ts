import { useEffect, useState } from 'react'

/**
 * Whether the themed `.dark` class is on the document element right now.
 *
 * The theme is applied by ThemeProvider and by the pre-paint `theme-init.js`
 * script, so the class — not a React context — is the only source that is
 * already correct on the first render. A MutationObserver keeps every chart in
 * step with a theme switch without remounting it.
 */
export function useIsDark(): boolean {
  const [dark, setDark] = useState(
    () => typeof document !== 'undefined' && document.documentElement.classList.contains('dark'),
  )
  useEffect(() => {
    const root = document.documentElement
    const observer = new MutationObserver(() => setDark(root.classList.contains('dark')))
    observer.observe(root, { attributes: true, attributeFilter: ['class'] })
    return () => observer.disconnect()
  }, [])
  return dark
}
