/**
 * Production theme lifecycle (design webui.md §11.1 "Theme behavior").
 *
 * One preference, three modes: Auto (follow the system) → Light → Dark → Auto.
 * The preference is stored under a production-owned key and is deliberately
 * isolated from the throwaway prototype key
 * (`platpulse.emerald-prototype.themeMode`). The resolved mode is applied to
 * the document element as a `dark` class, `color-scheme`, and
 * `data-theme-mode` attribute. `public/theme-init.js` applies this before the
 * application entry, so the first paint never depends on React.
 */

export type ThemeMode = 'auto' | 'light' | 'dark'
export type ResolvedTheme = 'light' | 'dark'

/** Production preference key. Never reuse the prototype key. */
export const THEME_STORAGE_KEY = 'platpulse.themeMode'
/** Document-element attribute carrying the selected mode. */
export const THEME_MODE_ATTRIBUTE = 'data-theme-mode'
/** Media query that drives Auto. */
export const THEME_MEDIA_QUERY = '(prefers-color-scheme: dark)'

const THEME_MODE_LABELS: Record<ThemeMode, string> = {
  auto: 'Auto',
  light: 'Light',
  dark: 'Dark',
}

export function isThemeMode(value: unknown): value is ThemeMode {
  return value === 'auto' || value === 'light' || value === 'dark'
}

/** Auto → Light → Dark → Auto (webui.md §11.1). */
export function nextThemeMode(mode: ThemeMode): ThemeMode {
  switch (mode) {
    case 'auto':
      return 'light'
    case 'light':
      return 'dark'
    case 'dark':
      return 'auto'
  }
}

export function themeModeLabel(mode: ThemeMode): string {
  return THEME_MODE_LABELS[mode]
}

/** Accessible name: the current choice and the action the control performs. */
export function themeToggleLabel(mode: ThemeMode): string {
  return `Theme: ${themeModeLabel(mode)}. Switch to ${themeModeLabel(nextThemeMode(mode))}`
}

function defaultStorage(): Storage | null {
  try {
    return window.localStorage
  } catch {
    // Storage can be unavailable (private mode, blocked origin). Auto is safe.
    return null
  }
}

/**
 * Read the persisted preference. A missing, invalid, or unreadable value
 * resolves to Auto rather than throwing, so unavailable storage still allows
 * in-session switching.
 */
export function readStoredThemeMode(storage: Storage | null = defaultStorage()): ThemeMode {
  if (!storage) return 'auto'
  try {
    const stored = storage.getItem(THEME_STORAGE_KEY)
    return isThemeMode(stored) ? stored : 'auto'
  } catch {
    return 'auto'
  }
}

/** Persist the preference; theme switching must survive a write failure. */
export function storeThemeMode(mode: ThemeMode, storage: Storage | null = defaultStorage()): void {
  if (!storage) return
  try {
    storage.setItem(THEME_STORAGE_KEY, mode)
  } catch {
    // A blocked write only loses persistence; the session keeps its choice.
  }
}

export function resolveTheme(mode: ThemeMode, systemPrefersDark: boolean): ResolvedTheme {
  if (mode === 'auto') return systemPrefersDark ? 'dark' : 'light'
  return mode
}

/** The system preference query, or null where matchMedia is unavailable. */
export function systemThemeQuery(): MediaQueryList | null {
  return typeof window.matchMedia === 'function' ? window.matchMedia(THEME_MEDIA_QUERY) : null
}

/**
 * Apply the resolved theme to the document before/without React. Returns the
 * resolved mode so callers can reconcile their own state with the paint.
 */
export function applyTheme(mode: ThemeMode, doc: Document = document): ResolvedTheme {
  const resolved = resolveTheme(mode, systemThemeQuery()?.matches ?? false)
  const root = doc.documentElement
  root.classList.toggle('dark', resolved === 'dark')
  root.style.colorScheme = resolved
  root.setAttribute(THEME_MODE_ATTRIBUTE, mode)
  return resolved
}

/** The mode already applied to the document, if a pre-mount script ran. */
export function appliedThemeMode(doc: Document = document): ThemeMode | null {
  const applied = doc.documentElement.getAttribute(THEME_MODE_ATTRIBUTE)
  return isThemeMode(applied) ? applied : null
}
