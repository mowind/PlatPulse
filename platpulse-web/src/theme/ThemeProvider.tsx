import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react'
import {
  appliedThemeMode,
  applyTheme,
  nextThemeMode,
  readStoredThemeMode,
  storeThemeMode,
  systemThemeQuery,
  themeToggleLabel,
  type ThemeMode,
} from '../theme'

type ThemeContextValue = {
  /** Selected preference: Auto, Light, or Dark. */
  mode: ThemeMode
  /** Accessible name describing the current choice and the next action. */
  toggleLabel: string
  /** Advance Auto → Light → Dark → Auto. */
  cycleTheme: () => void
}

const ThemeContext = createContext<ThemeContextValue | null>(null)

function initialMode(): ThemeMode {
  // public/theme-init.js applies the stored, validated preference before this
  // module runs. Reconcile with that paint instead of re-reading storage, so
  // the first React render matches the already-painted document.
  return appliedThemeMode() ?? readStoredThemeMode()
}

/**
 * Owns the production theme lifecycle: applies the resolved theme to the
 * document element, follows system changes while Auto is selected, and
 * persists only the preference (webui.md §11.1 "Theme behavior").
 */
export function ThemeProvider({ children }: { children: ReactNode }) {
  const [mode, setMode] = useState<ThemeMode>(initialMode)

  // Apply before paint so a mode change never flashes the previous theme.
  useLayoutEffect(() => {
    applyTheme(mode)
  }, [mode])

  // Auto follows the system live; an explicit Light/Dark choice is never
  // overridden by a system change.
  useEffect(() => {
    if (mode !== 'auto') return
    const query = systemThemeQuery()
    if (!query) return
    const onChange = () => applyTheme('auto')
    query.addEventListener('change', onChange)
    return () => query.removeEventListener('change', onChange)
  }, [mode])

  useEffect(() => {
    storeThemeMode(mode)
  }, [mode])

  const cycleTheme = useCallback(() => {
    setMode((current) => nextThemeMode(current))
  }, [])

  const value = useMemo<ThemeContextValue>(
    () => ({ mode, toggleLabel: themeToggleLabel(mode), cycleTheme }),
    [mode, cycleTheme],
  )

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
}

export function useTheme(): ThemeContextValue {
  const context = useContext(ThemeContext)
  if (!context) throw new Error('useTheme must be used within a ThemeProvider')
  return context
}
