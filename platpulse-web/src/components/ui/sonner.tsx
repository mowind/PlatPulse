import type * as React from 'react'
import { Toaster as Sonner, toast } from 'sonner'
import { useTheme } from '../../theme/ThemeProvider'

type ToasterProps = React.ComponentProps<typeof Sonner>

// Fallback for headless / unit-test environments without window.matchMedia
if (typeof window !== 'undefined' && typeof window.matchMedia !== 'function') {
  window.matchMedia = (query: string) =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }) as unknown as MediaQueryList
}

/**
 * Sonner Toaster, ported from komari-theme-emerald @ c2c5e88
 * src/components/ui/sonner/Sonner.vue.
 *
 * Binds Emerald color tokens (--popover, --popover-foreground, --border)
 * and maps ThemeProvider's 'auto' mode to Sonner's 'system'.
 */
export function Toaster({ ...props }: ToasterProps) {
  const { mode } = useTheme()

  return (
    <Sonner
      theme={mode === 'auto' ? 'system' : mode}
      className="toaster group"
      style={
        {
          '--normal-bg': 'var(--popover)',
          '--normal-text': 'var(--popover-foreground)',
          '--normal-border': 'var(--border)',
          '--border-radius': 'var(--radius)',
        } as React.CSSProperties
      }
      {...props}
    />
  )
}

export { toast }
