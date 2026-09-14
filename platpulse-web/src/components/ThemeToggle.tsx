import { themeModeLabel, type ThemeMode } from '../theme'
import { useTheme } from '../theme/ThemeProvider'

/** Decorative glyphs; the accessible name carries the real meaning. */
const THEME_ICONS: Record<ThemeMode, string> = { auto: '◐', light: '☀', dark: '☾' }

/**
 * One control for the whole site: Auto → Light → Dark → Auto. Its accessible
 * name states the current choice and the next action (webui.md §11.1), and it
 * is a 44×44 touch target wherever it appears.
 */
export default function ThemeToggle() {
  const { mode, toggleLabel, cycleTheme } = useTheme()

  return (
    <button
      type="button"
      className="theme-toggle"
      onClick={cycleTheme}
      aria-label={toggleLabel}
      title={toggleLabel}
      data-theme-mode={mode}
    >
      <span className="theme-toggle-icon" aria-hidden="true">
        {THEME_ICONS[mode]}
      </span>
      <span className="theme-toggle-label" aria-hidden="true">
        {themeModeLabel(mode)}
      </span>
    </button>
  )
}
