import { describe, expect, it } from 'vitest'
import { SURFACE_CARD, SURFACE_CARD_DISCLOSURE, SURFACE_CARD_INTERACTIVE, SURFACE_CARD_STATIC, SURFACE_CARD_SUMMARY } from './surface'

const opacity = (recipe: string) => Number(recipe.slice('bg-background/'.length))

describe('card surface intent', () => {
  it('keeps every read-only card translucent with no hover opacity change', () => {
    for (const recipe of [SURFACE_CARD_SUMMARY, SURFACE_CARD_STATIC, SURFACE_CARD_DISCLOSURE]) {
      expect(recipe).toMatch(/^bg-background\/\d+$/)
      expect(recipe).not.toContain('hover:')
    }
  })
  it('orders the read-only tiers from the most to the least authoritative', () => {
    expect(opacity(SURFACE_CARD_SUMMARY)).toBeGreaterThan(opacity(SURFACE_CARD_STATIC))
    expect(opacity(SURFACE_CARD_STATIC)).toBeGreaterThan(opacity(SURFACE_CARD_DISCLOSURE))
  })
  it('retains the existing hover recipe for clickable cards and legacy pages', () => {
    expect(SURFACE_CARD_INTERACTIVE).toBe('bg-background/60 hover:bg-background')
    expect(SURFACE_CARD).toBe(SURFACE_CARD_INTERACTIVE)
  })
})
