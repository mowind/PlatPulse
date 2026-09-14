import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  appliedThemeMode,
  applyTheme,
  isThemeMode,
  nextThemeMode,
  readStoredThemeMode,
  resolveTheme,
  storeThemeMode,
  themeModeLabel,
  themeToggleLabel,
  THEME_MODE_ATTRIBUTE,
  THEME_STORAGE_KEY,
} from './theme'

function memoryStorage(initial: Record<string, string> = {}): Storage {
  const map = new Map(Object.entries(initial))
  return {
    get length() {
      return map.size
    },
    clear: () => map.clear(),
    getItem: (key) => (map.has(key) ? map.get(key)! : null),
    key: (index) => Array.from(map.keys())[index] ?? null,
    removeItem: (key) => {
      map.delete(key)
    },
    setItem: (key, value) => {
      map.set(key, value)
    },
  } as Storage
}

function stubSystemTheme(matches: boolean) {
  vi.stubGlobal('matchMedia', () => ({ matches }) as MediaQueryList)
}

beforeEach(() => {
  document.documentElement.classList.remove('dark')
  document.documentElement.removeAttribute(THEME_MODE_ATTRIBUTE)
  document.documentElement.style.colorScheme = ''
  vi.unstubAllGlobals()
})

describe('theme mode cycle', () => {
  it('cycles Auto → Light → Dark → Auto without gaps', () => {
    expect(nextThemeMode('auto')).toBe('light')
    expect(nextThemeMode('light')).toBe('dark')
    expect(nextThemeMode('dark')).toBe('auto')
  })

  it('names the current choice and the next action', () => {
    expect(themeModeLabel('auto')).toBe('Auto')
    expect(themeToggleLabel('auto')).toBe('Theme: Auto. Switch to Light')
    expect(themeToggleLabel('light')).toBe('Theme: Light. Switch to Dark')
    expect(themeToggleLabel('dark')).toBe('Theme: Dark. Switch to Auto')
  })

  it('validates only the three supported modes', () => {
    expect(isThemeMode('auto')).toBe(true)
    expect(isThemeMode('light')).toBe(true)
    expect(isThemeMode('dark')).toBe(true)
    expect(isThemeMode('sepia')).toBe(false)
    expect(isThemeMode(null)).toBe(false)
  })
})

describe('theme preference persistence', () => {
  it('reads a valid production preference', () => {
    expect(readStoredThemeMode(memoryStorage({ [THEME_STORAGE_KEY]: 'dark' }))).toBe('dark')
  })

  it('falls back to Auto for missing, invalid, or unreadable values', () => {
    expect(readStoredThemeMode(memoryStorage())).toBe('auto')
    expect(readStoredThemeMode(memoryStorage({ [THEME_STORAGE_KEY]: 'sepia' }))).toBe('auto')
    const unavailable = memoryStorage()
    unavailable.getItem = () => {
      throw new Error('storage unavailable')
    }
    expect(readStoredThemeMode(unavailable)).toBe('auto')
  })

  it('isolates the production key from the throwaway prototype key', () => {
    const storage = memoryStorage({ 'platpulse.emerald-prototype.themeMode': 'dark' })
    expect(readStoredThemeMode(storage)).toBe('auto')
    storeThemeMode('light', storage)
    expect(storage.getItem(THEME_STORAGE_KEY)).toBe('light')
    // The prototype preference is untouched.
    expect(storage.getItem('platpulse.emerald-prototype.themeMode')).toBe('dark')
  })

  it('does not throw when storage is unavailable', () => {
    const unavailable = memoryStorage()
    unavailable.setItem = () => {
      throw new Error('storage unavailable')
    }
    expect(() => storeThemeMode('dark', unavailable)).not.toThrow()
  })
})

describe('theme resolution and application', () => {
  it('resolves Auto against the system and explicit modes against themselves', () => {
    expect(resolveTheme('auto', true)).toBe('dark')
    expect(resolveTheme('auto', false)).toBe('light')
    expect(resolveTheme('light', true)).toBe('light')
    expect(resolveTheme('dark', false)).toBe('dark')
  })

  it('applies the class, color-scheme, and attribute to the document', () => {
    stubSystemTheme(false)
    expect(applyTheme('dark')).toBe('dark')
    const root = document.documentElement
    expect(root.classList.contains('dark')).toBe(true)
    expect(root.style.colorScheme).toBe('dark')
    expect(appliedThemeMode()).toBe('dark')

    stubSystemTheme(false)
    expect(applyTheme('auto')).toBe('light')
    expect(root.classList.contains('dark')).toBe(false)
    expect(root.style.colorScheme).toBe('light')
    expect(appliedThemeMode()).toBe('auto')
  })

  it('resolves Auto to the dark system preference', () => {
    stubSystemTheme(true)
    expect(applyTheme('auto')).toBe('dark')
    expect(document.documentElement.classList.contains('dark')).toBe(true)
  })
})
