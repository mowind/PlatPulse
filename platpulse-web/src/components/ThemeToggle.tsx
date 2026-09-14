import { useTheme } from '../theme/ThemeProvider'
import { EmeraldActionIcon } from './EmeraldActionIcon'

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
      <EmeraldActionIcon name={mode === 'auto' ? 'dark-mode' : mode === 'light' ? 'sun-one' : 'moon'} />
    </button>
  )
}
