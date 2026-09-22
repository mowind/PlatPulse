import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'

import type { PublicValidatorInsight } from '../api/generated'
import { ValidatorActivityBadge } from './ValidatorActivityBadge'

afterEach(cleanup)

/** Server-owned Activity snapshot; only the required fields plus the ones the
 *  badge reads. Never a locally derived status. */
const linked = (
  activity: string,
  activityState = 'current',
  overrides: Partial<PublicValidatorInsight> = {},
): PublicValidatorInsight => ({
  validatorId: 'validator-a',
  validatorNodeId: `0x${'a'.repeat(128)}`,
  state: 'fresh',
  freshness: 'fresh',
  source: 'platscan',
  activity,
  activityState,
  rankState: 'unknown',
  rankFreshness: 'unknown',
  blockRateState: 'unknown',
  counterState: 'normal',
  currentValidatorStatus: 'validator',
  currentValidatorStatusState: 'current',
  currentValidatorStatusQualifier: null,
  receivedAt: '2026-08-25T00:00:00Z',
  ...overrides,
})

const renderBadge = (validator: PublicValidatorInsight | null | undefined, identityReason?: string | null) => {
  render(<ValidatorActivityBadge validator={validator} identityReason={identityReason} />)
  return document.querySelector('[data-slot="validator-activity"]') as HTMLElement
}

const labelOf = (badge: HTMLElement) =>
  badge.querySelector('[data-slot="validator-activity-label"]')?.textContent

const sizersOf = (badge: HTMLElement) =>
  [...badge.querySelectorAll('[data-slot="validator-activity-sizer"]')]
    .map((node) => node.getAttribute('data-sizer') ?? '')
    .sort()

const iconClassOf = (badge: HTMLElement) => badge.querySelector('svg')?.getAttribute('class') ?? ''

const svgOf = (badge: HTMLElement): SVGElement => {
  const svg = badge.querySelector('svg')
  if (!svg) throw new Error('the activity badge has no icon')
  return svg
}

/** The layout-only part of the badge's class list. Colour utilities vary by
 *  tone; size, spacing, radius and focus ring must not. */
const geometryOf = (badge: HTMLElement) =>
  [...badge.classList].filter((name) => !/(?:emerald|teal|muted|border-border)/.test(name)).sort()

describe('ValidatorActivityBadge', () => {
  it.each([
    ['verifying', 'Verifying', 'verifying', 'lucide-shield-check'],
    ['producing', 'Producing', 'producing', 'lucide-box'],
    ['active', 'Active', 'active', 'lucide-activity'],
    ['observing', 'Observing', 'muted', 'lucide-eye'],
  ])('renders %s as %s with the %s tone', (activity, label, tone, icon) => {
    const badge = renderBadge(linked(activity))
    expect(labelOf(badge)).toBe(label)
    expect(badge.getAttribute('data-activity')).toBe(activity)
    expect(badge.getAttribute('data-tone')).toBe(tone)
    expect(iconClassOf(badge)).toContain(icon)
    expect(badge.getAttribute('aria-label')).toContain(`PlatScan status: ${label}`)
  })

  it('treats unavailable evidence and a missing Link as Observing, never as a named state', () => {
    expect(labelOf(renderBadge(null))).toBe('Observing')
    cleanup()
    expect(labelOf(renderBadge(undefined))).toBe('Observing')
    cleanup()
    expect(labelOf(renderBadge(linked('unknown', 'unknown', { state: 'error', receivedAt: null })))).toBe('Observing')
  })

  it('keeps out-of-scheme canonical statuses as their real names with the neutral tone', () => {
    for (const [activity, label] of [['exiting', 'Exiting'], ['exited', 'Exited'], ['locked', 'Locked']] as const) {
      const badge = renderBadge(linked(activity))
      expect(labelOf(badge)).toBe(label)
      expect(badge.getAttribute('data-tone')).toBe('muted')
      cleanup()
    }
  })

  it('preserves an unlisted status verbatim instead of forcing it into the four named states', () => {
    const badge = renderBadge(linked('slashing'))
    expect(labelOf(badge)).toBe('Slashing')
    expect(badge.getAttribute('data-tone')).toBe('muted')
    expect(badge.getAttribute('aria-label')).toContain('PlatScan status: Slashing')
    fireEvent.focus(badge)
    expect(within(screen.getByRole('tooltip')).getByText('Status: Slashing')).toBeTruthy()
  })

  it('holds one width across every status by sizing against the same labels', () => {
    const reference = sizersOf(renderBadge(linked('verifying')))
    expect(reference.length).toBeGreaterThan(0)
    for (const activity of ['producing', 'active', 'observing', 'exiting', 'exited', 'locked', 'unknown']) {
      const badge = renderBadge(linked(activity))
      expect(sizersOf(badge)).toEqual(reference)
      cleanup()
    }
  })

  it('keeps Active legible in both themes with a full-strength Emerald foreground', () => {
    const classes = renderBadge(linked('active')).className
    // Light surfaces take the darker Emerald that clears AA for 11px text; the
    // bright dark-surface shade stays scoped to `dark:` and never leaks into light.
    expect(classes).toContain('text-emerald-700')
    expect(classes).toContain('dark:text-emerald-400')
    // The previously dimmed 70% wash is gone, and no pale Emerald replaces it in light.
    expect(classes).not.toMatch(/text-emerald-\d+\/70/)
    expect(classes).not.toMatch(/(?:^|\s)text-emerald-[1-5]00(?:\s|$)/)
  })

  it('keeps Observing neutral so the status name, not colour, carries the meaning', () => {
    const classes = renderBadge(linked('observing')).className
    expect(classes).toContain('text-muted-foreground')
    expect(classes).not.toMatch(/emerald|teal/)
  })

  it('keeps one badge geometry and one 14px currentColor glyph across the four states', () => {
    const reference = geometryOf(renderBadge(linked('verifying')))
    expect(reference.length).toBeGreaterThan(0)
    cleanup()
    for (const activity of ['producing', 'active', 'observing'] as const) {
      const badge = renderBadge(linked(activity))
      expect(geometryOf(badge)).toEqual(reference)
      const svg = svgOf(badge)
      expect(svg.getAttribute('width')).toBe('14')
      expect(svg.getAttribute('height')).toBe('14')
      expect(svg.getAttribute('stroke-width')).toBe('2')
      expect(svg.getAttribute('stroke')).toBe('currentColor')
      cleanup()
    }
  })

  it('marks a retained last-good value stale without adding a word to the badge', () => {
    const badge = renderBadge(linked('locked', 'stale', { state: 'error' }))
    expect(badge.getAttribute('data-stale')).toBe('true')
    expect(badge.querySelector('[data-slot="validator-activity-stale-mark"]')).toBeTruthy()
    expect(labelOf(badge)).toBe('Locked')
    expect(badge.getAttribute('aria-label')).toContain('stale')
    expect(badge.getAttribute('aria-label')).toContain('Showing the last successful value')
  })

  it('is keyboard focusable, names itself, and never relies on a native title', () => {
    const badge = renderBadge(linked('verifying'))
    expect(badge.getAttribute('tabindex')).toBe('0')
    expect(badge.getAttribute('title')).toBeNull()
    expect(badge.querySelector('svg')?.getAttribute('aria-hidden')).toBe('true')
    fireEvent.focus(badge)
    expect(screen.getByRole('tooltip')).toBeTruthy()
  })

  it('explains the PlatScan source, the full status, and the real update time', () => {
    const badge = renderBadge(linked('verifying'))
    fireEvent.focus(badge)
    const tooltip = screen.getByRole('tooltip')
    expect(within(tooltip).getByText('Source: PlatScan')).toBeTruthy()
    expect(within(tooltip).getByText('Status: Verifying')).toBeTruthy()
    expect(within(tooltip).getByText('Updated: 2026-08-25 00:00:00 UTC')).toBeTruthy()
  })

  it('never fabricates an update time and explains the actual reason', () => {
    const badge = renderBadge(undefined, 'Observed chain identity does not match the registered Network identity.')
    fireEvent.focus(badge)
    const tooltip = screen.getByRole('tooltip')
    expect(within(tooltip).getByText('Updated: Never observed')).toBeTruthy()
    expect(
      within(tooltip).getByText('Observed chain identity does not match the registered Network identity.'),
    ).toBeTruthy()
    expect(badge.getAttribute('aria-label')).toContain('registered Network')
  })

  it('names the bound-deployment reason for an unconfigured Network', () => {
    const badge = renderBadge(linked('unknown', 'unknown', { state: 'not_configured', receivedAt: null }))
    fireEvent.focus(badge)
    expect(within(screen.getByRole('tooltip')).getByText('No PlatScan deployment is bound to this Network.')).toBeTruthy()
  })
})
