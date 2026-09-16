import { useTheme } from '../theme/ThemeProvider'
import { EmeraldActionIcon } from './EmeraldActionIcon'
import { buttonVariants } from './ui/button'
import { cn } from '../lib/utils'

/**
 * One control for the whole site: Auto → Light → Dark → Auto. Its accessible
 * name states the current choice and the next action (webui.md §11.1), and it
 * is an Emerald ghost icon button, so it keeps the platform's 44x44 minimum.
 */
export default function ThemeToggle() {
  const { mode, toggleLabel, cycleTheme } = useTheme()

  return (
    <button
      type="button"
      data-slot="theme-toggle"
      className={cn(buttonVariants({ variant: 'ghost', size: 'icon-sm' }), 'compact-action border-[6px] border-transparent bg-clip-padding p-0')}
      onClick={cycleTheme}
      aria-label={toggleLabel}
      title={toggleLabel}
      data-theme-mode={mode}
    >
      <EmeraldActionIcon name={mode === 'auto' ? 'dark-mode' : mode === 'light' ? 'sun-one' : 'moon'} />
    </button>
  )
}
