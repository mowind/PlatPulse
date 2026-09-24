import { describe, expect, it } from 'vitest'
import { SURFACE_CARD, SURFACE_CARD_INTERACTIVE, SURFACE_CARD_STATIC } from './surface'

describe('card surface intent', () => {
  it('keeps read-only cards translucent with no hover opacity change', () => {
    expect(SURFACE_CARD_STATIC).toBe('bg-background/60')
    expect(SURFACE_CARD_STATIC).not.toContain('hover:')
  })
  it('retains the existing hover recipe for clickable cards and legacy pages', () => {
    expect(SURFACE_CARD_INTERACTIVE).toBe('bg-background/60 hover:bg-background')
    expect(SURFACE_CARD).toBe(SURFACE_CARD_INTERACTIVE)
  })
})
